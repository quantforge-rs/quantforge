//! The reader a host opens to read candles and nothing else
//! (`quantforge::CandleReader`, `StoreFactory::open_reader`), on each
//! backend: what it reads of a store a command wrote, the sentence it has
//! for a range that held nothing, and what it refuses and leaves as it was:
//! a file that is not there, a database without a store, a store at another
//! schema, and one that holds an unfinished copy. PostgreSQL runs on a
//! database of its own on the server `QUANTFORGE_TEST_POSTGRES_URL` names,
//! and skips without it.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[cfg(feature = "postgres")]
use quantforge::ConnectionString;
use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{
    AnyStore, Candle, CandleReader, CandleStore, ENGINE_VERSION, ExchangeId, Interval, MarketId,
    StorageBackend, StorageConfigError, StorageError, StorageRequest, StorageSettings,
    StoreFactory, StoreOpenError, StoreOwner, Supplied, Symbol,
};
use rust_decimal::Decimal;

use crate::conformance::Raw;
use crate::conformance::cases::fixture;
use crate::sqlite::SqliteRaw;

/// One database of a backend: where a store lives, or does not yet.
struct Place {
    database: Database,
    /// Whatever keeps the database there, dropped last.
    _guard: Box<dyn Any>,
}

enum Database {
    Sqlite(PathBuf),
    #[cfg(feature = "postgres")]
    Postgres(String),
}

impl Place {
    /// A path in a directory of its own, with no file at it yet.
    fn sqlite() -> Self {
        let tempdir = tempfile::tempdir().expect("tempdir");
        Self {
            database: Database::Sqlite(tempdir.path().join("market.sqlite")),
            _guard: Box::new(tempdir),
        }
    }

    /// A database of its own on the test server, with no table in it yet,
    /// or `None` to skip.
    #[cfg(feature = "postgres")]
    fn postgres() -> Option<Self> {
        let Some(server) = crate::postgres::server_url() else {
            crate::postgres::skip(&format!("{} is unset", crate::postgres::VARIABLE));
            return None;
        };
        let database = crate::postgres::IsolatedDatabase::create(&server);
        Some(Self {
            database: Database::Postgres(database.url.clone()),
            _guard: Box::new(database),
        })
    }

    /// What a host resolves for this place, as `owner`.
    fn settings(&self, owner: StoreOwner) -> StorageSettings {
        let mut request = StorageRequest::default();
        match &self.database {
            Database::Sqlite(path) => {
                request.sqlite_path = Some(Supplied::new(path.clone(), "db"));
            }
            #[cfg(feature = "postgres")]
            Database::Postgres(url) => {
                request.backend = Some(Supplied::new(StorageBackend::Postgres, "storage"));
                request.postgres_url =
                    Some(Supplied::new(ConnectionString::new(url), "postgres_url"));
            }
        }
        StorageSettings::resolve(request, owner).expect("settings")
    }

    /// The file of a SQLite place.
    fn path(&self) -> &Path {
        match &self.database {
            Database::Sqlite(path) => path,
            #[cfg(feature = "postgres")]
            Database::Postgres(_) => panic!("not a sqlite place"),
        }
    }

    /// The store as a command opens it: created if missing, and brought to
    /// this build's schema.
    fn store(&self) -> AnyStore {
        StoreFactory::builtin()
            .open(&self.settings(StoreOwner::Command))
            .expect("a store")
    }

    /// The store as a reader opens it.
    fn open_reader(&self) -> Result<CandleReader, StoreOpenError> {
        StoreFactory::builtin().open_reader(&self.settings(StoreOwner::Reader))
    }

    fn reader(&self) -> CandleReader {
        self.open_reader().expect("a reader")
    }

    /// Every row of a query, each value as its text.
    fn select(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        match &self.database {
            Database::Sqlite(path) => SqliteRaw(path.clone()).select(sql),
            #[cfg(feature = "postgres")]
            Database::Postgres(url) => crate::postgres::query(url, sql),
        }
    }

    /// A statement run behind the store's back.
    fn execute(&self, sql: &str) {
        match &self.database {
            Database::Sqlite(path) => SqliteRaw(path.clone()).execute(sql),
            #[cfg(feature = "postgres")]
            Database::Postgres(url) => drop(crate::postgres::query(url, sql)),
        }
    }

    /// The one value a query selects.
    fn value(&self, sql: &str) -> String {
        let rows = self.select(sql);
        assert_eq!(rows.len(), 1, "{sql}: {rows:?}");
        rows[0][0].clone().expect("a value")
    }
}

/// Runs a scenario on a SQLite place and, with the feature and a server, on
/// a PostgreSQL one.
macro_rules! on_each_backend {
    ($($scenario:ident),* $(,)?) => {
        $(mod $scenario {
            use super::Place;

            #[test]
            fn sqlite() {
                super::$scenario(&Place::sqlite());
            }

            #[cfg(feature = "postgres")]
            #[test]
            fn postgres() {
                if let Some(place) = Place::postgres() {
                    super::$scenario(&place);
                }
            }
        })*
    };
}

fn market(exchange: ExchangeId, symbol: &str) -> MarketId {
    MarketId::new(
        exchange,
        Symbol::new(symbol).expect("a symbol"),
        Interval::M1,
    )
}

fn spot() -> MarketId {
    market(ExchangeId::BinanceSpot, "BTCUSDT")
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).expect("a decimal")
}

const MINUTE: i64 = 60_000;
/// 2026-01-01T12:00:00Z.
const NOON: i64 = 1_767_268_800_000;

/// The candle of minute `index` after noon, every price `close`.
fn minute(index: i64, close: &str, trades: Option<u64>) -> Candle {
    Candle {
        open_time_ms: NOON + index * MINUTE,
        close_time_ms: NOON + index * MINUTE + MINUTE - 1,
        open: decimal(close),
        high: decimal(close),
        low: decimal(close),
        close: decimal(close),
        volume: decimal("0.17235000"),
        trades,
    }
}

/// Five minutes of candles: prices written with different numbers of
/// decimal places, one count unknown, one zero.
fn five_minutes() -> Vec<Candle> {
    vec![
        minute(0, "81940.01000000", Some(412)),
        minute(1, "81955.25", None),
        minute(2, "81900", Some(0)),
        minute(3, "81900.10", Some(7)),
        minute(4, "0.00000001", Some(1)),
    ]
}

/// Every decimal of a candle as its exact text, with its times and count.
fn written(candle: &Candle) -> String {
    format!(
        "{} {} {} {} {} {} {} {:?}",
        candle.open_time_ms,
        candle.close_time_ms,
        candle.open,
        candle.high,
        candle.low,
        candle.close,
        candle.volume,
        candle.trades
    )
}

fn as_written(candles: &[Candle]) -> Vec<String> {
    candles.iter().map(written).collect()
}

fn refusal(result: Result<CandleReader, StoreOpenError>) -> String {
    match result {
        Err(StoreOpenError::Storage(StorageError::InvalidArgument(message))) => message,
        other => panic!("not a refusal: {other:?}"),
    }
}

fn reads_what_a_command_wrote_oldest_first_and_digit_for_digit(place: &Place) {
    let candles = five_minutes();
    let store = place.store();
    // Written out of order, in two batches, as a backfill after a sync is.
    store
        .upsert_candles(&spot(), &candles[3..])
        .expect("the later candles");
    store
        .upsert_candles(&spot(), &candles[..3])
        .expect("the earlier candles");

    let reader = place.reader();
    assert_eq!(reader.backend().as_str(), reader_backend(place));
    let all = reader.load(&spot(), None, None).expect("candles");
    assert_eq!(all, candles);
    // Equal decimals are not yet the same text: every digit is.
    assert_eq!(as_written(&all), as_written(&candles));

    // Both ends of a range are part of it.
    let load = |start: Option<i64>, end: Option<i64>| {
        reader
            .load(&spot(), start.map(at), end.map(at))
            .expect("a range")
    };
    assert_eq!(load(Some(1), Some(3)), candles[1..=3]);
    assert_eq!(load(Some(1), Some(1)), candles[1..=1]);
    assert_eq!(load(Some(2), None), candles[2..]);
    assert_eq!(load(None, Some(2)), candles[..=2]);
    // A bound between two candles takes neither side's neighbour.
    let between = reader
        .load(&spot(), Some(at(1) + 1), Some(at(3) - 1))
        .expect("a range");
    assert_eq!(between, candles[2..=2]);
    assert_eq!(
        reader
            .load(&spot(), Some(i64::MIN), Some(i64::MAX))
            .expect("a range"),
        candles
    );
}

/// The open time of minute `index`.
fn at(index: i64) -> i64 {
    NOON + index * MINUTE
}

fn reader_backend(place: &Place) -> &'static str {
    match place.database {
        Database::Sqlite(_) => "sqlite",
        #[cfg(feature = "postgres")]
        Database::Postgres(_) => "postgres",
    }
}

fn keeps_every_product_line_and_every_venue_apart(place: &Place) {
    let store = place.store();
    // One symbol and one interval under every product line, each with a
    // candle of its own at the same time.
    let lines: Vec<ExchangeId> = ExchangeId::all().collect();
    assert_eq!(lines.len(), 6, "{lines:?}");
    for (index, line) in lines.iter().enumerate() {
        let price = format!("{}.5", 100 + index);
        store
            .upsert_candles(
                &market(*line, "BTCUSDT"),
                &[minute(0, &price, Some(index as u64))],
            )
            .expect("a candle");
    }
    let reader = place.reader();
    for (index, line) in lines.iter().enumerate() {
        let found = reader
            .load(&market(*line, "BTCUSDT"), None, None)
            .expect("candles");
        let price = format!("{}.5", 100 + index);
        assert_eq!(found, [minute(0, &price, Some(index as u64))], "{line:?}");
    }
    // And another symbol, or another interval, of a line holds nothing.
    assert!(
        reader
            .load(&market(ExchangeId::BinanceSpot, "ETHUSDT"), None, None)
            .expect("candles")
            .is_empty()
    );
    let hourly = MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::H1,
    );
    assert!(
        reader
            .load(&hourly, None, None)
            .expect("candles")
            .is_empty()
    );
}

fn says_why_a_range_held_nothing(place: &Place) {
    let candles = five_minutes();
    let store = place.store();
    // Minutes 0, 1 and 4: a hole where 2 and 3 would be.
    store
        .upsert_candles(&spot(), &[candles[0].clone(), candles[1].clone()])
        .expect("candles");
    store
        .upsert_candles(&spot(), &candles[4..])
        .expect("candles");
    let reader = place.reader();
    let location = reader.location().to_string();
    let found = |market: &MarketId, start: Option<i64>, end: Option<i64>| {
        assert_eq!(reader.load(market, start, end).expect("candles"), []);
        reader
            .nothing_found(market, start, end)
            .strip_prefix(&format!("{location} "))
            .expect("the store's location first")
            .to_string()
    };
    let held = "it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z";

    // A market the store holds nothing of, whatever the range.
    let other = market(ExchangeId::BybitSpot, "BTCUSDT");
    for (start, end) in [
        (None, None),
        (Some(at(0)), Some(at(4))),
        (Some(at(9)), None),
    ] {
        assert_eq!(
            found(&other, start, end),
            "holds no candles of bybit_spot BTCUSDT 1m"
        );
    }
    // A range after the last candle, before the first, and in the hole.
    assert_eq!(
        found(&spot(), Some(at(5)), Some(at(9))),
        format!(
            "holds no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:05:00Z to \
             2026-01-01T12:09:00Z; {held}"
        )
    );
    assert_eq!(
        found(&spot(), Some(at(2)), Some(at(3))),
        format!(
            "holds no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:02:00Z to \
             2026-01-01T12:03:00Z; {held}"
        )
    );
    // A range of one instant, which is a range, and holds no candle.
    assert_eq!(
        found(&spot(), Some(at(3)), Some(at(3))),
        format!(
            "holds no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:03:00Z to \
             2026-01-01T12:03:00Z; {held}"
        )
    );
    // One bound alone.
    assert_eq!(
        found(&spot(), Some(at(4) + 1), None),
        format!(
            "holds no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:04:00.001Z on; {held}"
        )
    );
    assert_eq!(
        found(&spot(), None, Some(at(0) - 1)),
        format!(
            "holds no candles of binance_spot BTCUSDT 1m up to 2026-01-01T11:59:59.999Z; {held}"
        )
    );
    // A range that ends before it starts holds no time at all, and the
    // store is not asked what it holds.
    for market in [spot(), other] {
        let line = market.exchange.as_str();
        assert_eq!(
            found(&market, Some(at(4)), Some(at(0))),
            format!(
                "holds no candles of {line} BTCUSDT 1m from 2026-01-01T12:04:00Z to \
                 2026-01-01T12:00:00Z: the range ends before it starts"
            )
        );
    }
    // A time no date holds is written as the number it is.
    assert_eq!(
        found(&spot(), Some(i64::MAX), None),
        format!(
            "holds no candles of binance_spot BTCUSDT 1m from invalid-ms({}) on; {held}",
            i64::MAX
        )
    );
    // Two bounds inside one millisecond, which a host whose times can fall
    // between two says so: the range it loads is from the millisecond after
    // the first to the one before the second, and holds no candle.
    assert_eq!(
        reader
            .load(&spot(), Some(at(2) + 1), Some(at(2)))
            .expect("candles"),
        []
    );
    assert_eq!(
        reader.nothing_inside(&spot(), at(2)),
        format!(
            "{location} holds no candles of binance_spot BTCUSDT 1m between two times inside \
             the millisecond that begins at 2026-01-01T12:02:00Z: a candle opens on a whole \
             millisecond"
        )
    );
}

fn sees_what_a_writer_stored_since_it_opened(place: &Place) {
    let candles = five_minutes();
    let store = place.store();
    store
        .upsert_candles(&spot(), &candles[..2])
        .expect("candles");
    let reader = place.reader();
    assert_eq!(
        reader.load(&spot(), None, None).expect("candles"),
        candles[..2]
    );
    // The writer goes on beside the reader, and corrects a candle too.
    let mut corrected = candles.clone();
    corrected[1].close = decimal("81955.26");
    store
        .upsert_candles(&spot(), &corrected[1..])
        .expect("more candles");
    assert_eq!(
        reader.load(&spot(), None, None).expect("candles"),
        corrected
    );
    // With nothing in a range's way, a sentence asked for late says so.
    assert_eq!(
        reader.nothing_found(&spot(), None, None),
        format!(
            "{} held no candles of binance_spot BTCUSDT 1m as they were read; it holds them \
             from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z now",
            reader.location()
        )
    );
    // And so it does of a range a writer filled between the load that
    // found it empty and the sentence: it never says the store holds no
    // candles where it holds some.
    let later = Some(at(5));
    assert_eq!(reader.load(&spot(), later, None).expect("candles"), []);
    store
        .upsert_candles(&spot(), &[minute(5, "81920.00", Some(4))])
        .expect("a later candle");
    assert_eq!(
        reader.nothing_found(&spot(), later, None),
        format!(
            "{} held no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:05:00Z on as they \
             were read; it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:05:00Z now",
            reader.location()
        )
    );
    // A hole in what the store holds is said to be one, now as before.
    store
        .upsert_candles(&spot(), &[minute(9, "81920.00", Some(4))])
        .expect("a candle after a hole");
    let hole = (Some(at(6)), Some(at(8)));
    assert_eq!(reader.load(&spot(), hole.0, hole.1).expect("candles"), []);
    assert_eq!(
        reader.nothing_found(&spot(), hole.0, hole.1),
        format!(
            "{} holds no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:06:00Z to \
             2026-01-01T12:08:00Z; it holds them from 2026-01-01T12:00:00Z to \
             2026-01-01T12:09:00Z",
            reader.location()
        )
    );
}

/// The tables a database holds, by the backend's own catalog.
fn tables(place: &Place) -> usize {
    let sql = match place.database {
        Database::Sqlite(_) => "SELECT CAST(count(*) AS TEXT) FROM sqlite_master",
        #[cfg(feature = "postgres")]
        Database::Postgres(_) => {
            "SELECT CAST(count(*) AS TEXT) FROM pg_tables WHERE schemaname = current_schema()"
        }
    };
    place.value(sql).parse().expect("a count")
}

fn refuses_a_database_without_a_store_and_creates_none(place: &Place) {
    // SQLite's file has to be there to hold nothing: a database of
    // something else.
    match &place.database {
        Database::Sqlite(path) => rusqlite::Connection::open(path)
            .and_then(|connection| connection.execute_batch("CREATE TABLE notes (text TEXT)"))
            .expect("another database"),
        #[cfg(feature = "postgres")]
        Database::Postgres(_) => {}
    }
    let before = tables(place);
    let location = place.settings(StoreOwner::Reader).location();
    assert_eq!(
        refusal(place.open_reader()),
        format!(
            "{location} holds no QuantForge store; a reader creates none, and `quantforge data \
             sync` does"
        )
    );
    assert_eq!(tables(place), before);
}

const VERSION: &str = "SELECT value FROM meta WHERE key = 'schema_version'";
const MIGRATIONS: &str = "SELECT CAST(count(*) AS TEXT) FROM schema_migrations";

fn refuses_a_store_at_another_schema_and_migrates_nothing(place: &Place) {
    let store = place.store();
    store
        .upsert_candles(&spot(), &five_minutes())
        .expect("candles");
    drop(store);
    let steps = place.value(MIGRATIONS);
    let settings = place.settings(StoreOwner::Reader);
    let location = settings.location();
    let forward = match &place.database {
        Database::Sqlite(path) => format!("`quantforge --db {} store status`", path.display()),
        #[cfg(feature = "postgres")]
        Database::Postgres(_) => {
            "`quantforge --storage postgres store status` with the same URL".to_string()
        }
    };

    // As an earlier build left it.
    let earlier = SCHEMA_VERSION - 1;
    place.execute(&format!(
        "UPDATE meta SET value = '{earlier}' WHERE key = 'schema_version'"
    ));
    assert_eq!(
        refusal(place.open_reader()),
        format!(
            "{location} is at schema {earlier}, and this build reads schema {SCHEMA_VERSION}; a \
             reader migrates nothing: bring the store forward with the quantforge command line \
             of this version ({ENGINE_VERSION}), where any command does it in place, such as \
             {forward}"
        )
    );
    assert_eq!(place.value(VERSION), earlier.to_string());
    assert_eq!(place.value(MIGRATIONS), steps);

    // As a later build left it.
    let later = SCHEMA_VERSION + 1;
    place.execute(&format!(
        "UPDATE meta SET value = '{later}' WHERE key = 'schema_version'"
    ));
    match place.open_reader() {
        Err(StoreOpenError::Storage(StorageError::SchemaUnsupported { found, supported })) => {
            assert_eq!((found, supported), (later, SCHEMA_VERSION));
        }
        other => panic!("not a newer schema: {other:?}"),
    }
    assert_eq!(place.value(VERSION), later.to_string());
    assert_eq!(place.value(MIGRATIONS), steps);

    // At this build's schema again, it reads.
    place.execute(&format!(
        "UPDATE meta SET value = '{SCHEMA_VERSION}' WHERE key = 'schema_version'"
    ));
    assert_eq!(
        place
            .reader()
            .load(&spot(), None, None)
            .expect("candles")
            .len(),
        5
    );
}

on_each_backend!(
    reads_what_a_command_wrote_oldest_first_and_digit_for_digit,
    keeps_every_product_line_and_every_venue_apart,
    says_why_a_range_held_nothing,
    sees_what_a_writer_stored_since_it_opened,
    refuses_a_database_without_a_store_and_creates_none,
    refuses_a_store_at_another_schema_and_migrates_nothing,
);

#[test]
fn a_sqlite_file_that_is_not_there_is_refused_and_not_created() {
    let place = Place::sqlite();
    let path = place.path();
    match place.open_reader() {
        Err(StoreOpenError::NoSuchFile {
            path: missing,
            resolved: None,
        }) => assert_eq!(&missing, path),
        other => panic!("not a missing file: {other:?}"),
    }
    assert_eq!(
        place.open_reader().expect_err("no file").to_string(),
        format!(
            "there is no SQLite database at {}; a reader creates none, and `quantforge data \
             sync` does",
            path.display()
        )
    );
    assert!(!path.exists());
    let directory = path.parent().expect("a directory");
    assert_eq!(std::fs::read_dir(directory).expect("entries").count(), 0);

    // A directory is there, and is no file to read.
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(directory.to_path_buf(), "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    assert_eq!(
        refusal(StoreFactory::builtin().open_reader(&settings)),
        format!(
            "{} is a directory, and a SQLite database is a file",
            directory.display()
        )
    );
    assert_eq!(std::fs::read_dir(directory).expect("entries").count(), 0);

    // What holds an @ or an =, as a URL or a connection string with a
    // password does where it is given in a path's place, is never shown:
    // not where no file is, and not where the system refuses the name, as
    // Windows refuses a URL's `:`.
    for given in [
        "postgres://SOMEONE:WORD@db.example.com/market",
        "host=db.example.com user=SOMEONE password=WORD",
    ] {
        let mut request = StorageRequest::default();
        request.sqlite_path = Some(Supplied::new(directory.join(given), "db"));
        let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
        let refused = StoreFactory::builtin()
            .open_reader(&settings)
            .expect_err("no file")
            .to_string();
        assert!(
            !refused.contains("WORD") && !refused.contains("SOMEONE"),
            "{refused}"
        );
        if cfg!(unix) {
            assert_eq!(
                refused,
                "there is no SQLite database at the path given, which is not shown: it holds an \
                 @ or an =, as a URL or a connection string with a password does; a reader \
                 creates none, and `quantforge data sync` does"
            );
        }
    }
    // Nor where what is there is no file.
    let odd = directory.join("user=SOMEONE password=WORD");
    std::fs::create_dir(&odd).expect("a directory");
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(odd.clone(), "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    assert_eq!(
        refusal(StoreFactory::builtin().open_reader(&settings)),
        "the path given is a directory, and a SQLite database is a file"
    );
    std::fs::remove_dir(&odd).expect("the directory removed");
    assert_eq!(std::fs::read_dir(directory).expect("entries").count(), 0);
}

/// Where the store cannot be read again to say what it holds, the sentence
/// says so, and why: the load that read nothing was an answer all the same.
#[test]
fn the_sentence_for_a_range_that_held_nothing_says_what_could_not_be_read() {
    let place = Place::sqlite();
    place
        .store()
        .upsert_candles(&spot(), &five_minutes())
        .expect("candles");
    let reader = place.reader();
    let location = reader.location().to_string();
    assert_eq!(
        reader.load(&spot(), Some(at(9)), None).expect("candles"),
        []
    );
    // The file goes before the sentence is asked for.
    std::fs::remove_file(place.path()).expect("the file removed");
    let said = reader.nothing_found(&spot(), Some(at(9)), None);
    let lead = format!(
        "{location} held no candles of binance_spot BTCUSDT 1m from 2026-01-01T12:09:00Z on as \
         they were read, and what it holds besides could not be read: invalid argument: cannot \
         open "
    );
    assert!(said.starts_with(&lead), "{said}");
}

/// A file at a path that may be a secret, which holds no SQLite database, is
/// refused without the path: no SQLite database is found at it.
#[test]
fn a_file_that_is_no_database_at_a_path_that_may_be_a_secret_is_named_without_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("user=SOMEONE password=WORD");
    std::fs::write(
        &file,
        "not a database, and longer than a header\n".repeat(20),
    )
    .expect("a file");
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(file, "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    assert_eq!(
        refusal(StoreFactory::builtin().open_reader(&settings)),
        "the path given is not a SQLite database"
    );
}

/// A missing file whose path does not start at the root says where it was
/// looked for, the full path it was settled to where the process ran as the
/// reader opened; and its `Debug`, as its `Display`, shows no path that may
/// be a secret, the full path included.
#[test]
fn a_missing_file_says_where_it_was_looked_for_and_debug_shows_no_secret() {
    let relative = PathBuf::from("no-such-directory").join("market.sqlite");
    let here = std::env::current_dir().expect("the directory");
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(relative.clone(), "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    match StoreFactory::builtin().open_reader(&settings) {
        Err(StoreOpenError::NoSuchFile { path, resolved }) => {
            assert_eq!(path, relative);
            assert_eq!(resolved, Some(here.join(&relative)));
        }
        other => panic!("not a missing file: {other:?}"),
    }
    assert!(!relative.exists() && !Path::new("no-such-directory").exists());

    let shown = StoreOpenError::NoSuchFile {
        path: relative.clone(),
        resolved: Some(here.join(&relative)),
    };
    let debug = format!("{shown:?}");
    assert!(
        debug.contains("market.sqlite") && debug.contains("no-such-directory"),
        "{debug}"
    );
    let url = "postgres://SOMEONE:WORD@db.example.com/market";
    let url_shaped = StoreOpenError::NoSuchFile {
        path: PathBuf::from(url),
        resolved: Some(here.join(url)),
    };
    let told = format!("{url_shaped:?} {url_shaped}");
    assert!(
        !told.contains("WORD") && !told.contains("SOMEONE"),
        "redaction failed: credentials were exposed"
    );
}

/// On Windows a path with a drive and no root, such as `D:market.sqlite`,
/// names a file in that drive's own current directory. A reader settles it
/// to that full path as it opens, and keeps no path the system would settle
/// again, elsewhere, once the process moves.
#[cfg(windows)]
#[test]
fn a_path_with_a_drive_and_no_root_is_settled_to_a_full_path() {
    use std::path::{Component, Prefix};

    let here = std::env::current_dir().expect("the directory");
    let Some(Component::Prefix(prefix)) = here.components().next() else {
        panic!("no drive in {}", here.display());
    };
    let Prefix::Disk(letter) = prefix.kind() else {
        panic!("not on a lettered drive: {}", here.display());
    };
    // The drive the process runs on, whose own directory is where it runs.
    let tail = PathBuf::from("no-such-directory").join("market.sqlite");
    let given = PathBuf::from(format!("{}:{}", char::from(letter), tail.display()));
    assert!(
        !given.is_absolute() && !given.has_root(),
        "{}",
        given.display()
    );
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(given.clone(), "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    match StoreFactory::builtin().open_reader(&settings) {
        Err(StoreOpenError::NoSuchFile { path, resolved }) => {
            assert_eq!(path, given);
            assert_eq!(resolved, Some(here.join(&tail)));
        }
        other => panic!("not a missing file: {other:?}"),
    }
}

/// A path that may be a secret is not shown where it cannot be reached
/// either: a file inside a directory that may not be entered may well be
/// there, and is refused with the path left out.
#[cfg(unix)]
#[test]
fn a_path_that_may_be_a_secret_is_not_shown_where_it_cannot_be_reached() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let closed = dir.path().join("user=SOMEONE password=WORD");
    std::fs::create_dir(&closed).expect("a directory");
    let mode = |mode| {
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(mode)).expect("chmod")
    };
    mode(0o000);
    // A user who enters it anyway, such as root, cannot see the refusal.
    let entered = std::fs::metadata(closed.join("market.sqlite")).is_ok()
        || std::fs::read_dir(&closed).is_ok();
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(closed.join("market.sqlite"), "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    let opened = StoreFactory::builtin().open_reader(&settings);
    mode(0o755);
    if entered {
        eprintln!("skipping: this user enters directories it may not");
        return;
    }
    let refused = refusal(opened);
    assert!(
        refused.starts_with("cannot reach the path given: "),
        "{refused}"
    );
    assert!(
        !refused.contains("WORD") && !refused.contains("SOMEONE"),
        "{refused}"
    );

    // Nor where a file is there that SQLite cannot open.
    let unreadable = dir.path().join("user=SOMEONE password=WORD.sqlite");
    std::fs::write(&unreadable, b"").expect("a file");
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(unreadable, "db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    let refused = refusal(StoreFactory::builtin().open_reader(&settings));
    assert!(
        refused.starts_with("cannot open the path given: "),
        "{refused}"
    );
    assert!(
        !refused.contains("WORD") && !refused.contains("SOMEONE"),
        "{refused}"
    );
}

/// A store an earlier build's command line wrote, as `tests/fixtures/sqlite`
/// keeps one: refused as it is, byte for byte unchanged, and read once a
/// command of this build brought it forward.
#[test]
fn a_store_an_earlier_build_wrote_is_read_once_a_command_brought_it_forward() {
    let place = Place::sqlite();
    let path = place.path();
    let sql = std::fs::read_to_string(fixture("tests/fixtures/sqlite/schema-9.sql"))
        .expect("the fixture");
    rusqlite::Connection::open(path)
        .and_then(|connection| connection.execute_batch(&sql))
        .expect("the fixture loaded");
    let before = std::fs::read(path).expect("the file");

    let refused = refusal(place.open_reader());
    assert!(
        refused.starts_with(&format!(
            "{} is at schema 9, and this build reads schema {SCHEMA_VERSION}; a reader migrates \
             nothing: ",
            path.display()
        )),
        "{refused}"
    );
    assert_eq!(std::fs::read(path).expect("the file"), before);

    // Any command of this build brings it forward, as the refusal says.
    drop(place.store());
    assert_eq!(place.value(VERSION), SCHEMA_VERSION.to_string());
    let candles = place.reader().load(&spot(), None, None).expect("candles");
    assert_eq!(
        as_written(&candles),
        [
            "1792454400000 1792454459999 67012.34000000 67020.00000000 67001.10000000 \
             67015.50000000 12.04510000 Some(1234)",
            "1792540800000 1792540859999 67100.00 67100.00 67100.00 67100.00 1.000 Some(7)",
            "1792540860000 1792540919999 67100.00 67105.50 67099.90 67101.10 0.50000000 None",
        ]
    );
}

#[test]
fn a_backend_without_a_provider_has_no_reader() {
    let place = Place::sqlite();
    let settings = place.settings(StoreOwner::Reader);
    match StoreFactory::empty().open_reader(&settings) {
        Err(StoreOpenError::Config(StorageConfigError::NoAdapter { backend })) => {
            assert_eq!(backend, StorageBackend::Sqlite);
        }
        other => panic!("not a missing provider: {other:?}"),
    }
}

/// A target whose copy is unfinished holds only part of its source's
/// candles: a reader refuses it, as every command but the copy does.
#[cfg(feature = "postgres")]
#[test]
fn a_store_holding_an_unfinished_copy_is_refused_until_it_is_finished() {
    use quantforge::{StoreControl, StoreId, TransferRecord};

    let Some(place) = Place::postgres() else {
        return;
    };
    let store = place.store();
    // A reader opened before the copy begins, as one may stay open.
    let open_before = place.reader();
    let target = store.transfer_target().expect("a transfer target");
    let source = StoreId::new("0123456789abcdef0123456789abcdef").expect("an ID");
    let record = TransferRecord::begin(source.clone(), NOON);
    target.begin_transfer(&record, None).expect("a copy begun");
    let own = store.store_identity().expect("an identity").store_id;
    let refused = format!(
        "store {own} holds an unfinished copy of store {source}, begun 2026-01-01T12:00:00Z; \
         finish it with `quantforge data migrate --from <the source>` before reading it"
    );
    assert_eq!(refusal(place.open_reader()), refused);
    // The copy's rows arrive after its record: whatever the open reader
    // read of them, it is refused.
    store
        .upsert_candles(&spot(), &five_minutes()[..2])
        .expect_err("no write while a copy is unfinished");
    match open_before.load(&spot(), None, None) {
        Err(StorageError::InvalidArgument(message)) => assert_eq!(message, refused),
        other => panic!("not the refusal of an unfinished copy: {other:?}"),
    }
    target
        .finish_transfer(&record.copied(NOON + 1))
        .expect("the copy finished");
    assert_eq!(
        place.reader().load(&spot(), None, None).expect("candles"),
        []
    );
    assert_eq!(open_before.load(&spot(), None, None).expect("candles"), []);
}

/// A reader commits nothing, so it is not held to how a server keeps its
/// commits: it opens on a server with `fsync` off, which refuses a store
/// that writes there under the settings every host resolves by default.
#[cfg(feature = "postgres")]
#[test]
fn a_reader_is_not_held_to_how_a_server_keeps_its_commits() {
    use quantforge::Durability;

    let Some(weak) = crate::postgres::server::url("QUANTFORGE_TEST_POSTGRES_WEAK_URL") else {
        crate::postgres::skip("QUANTFORGE_TEST_POSTGRES_WEAK_URL is unset");
        return;
    };
    let database = crate::postgres::IsolatedDatabase::create(&weak);
    let request = || {
        let mut request = StorageRequest::default();
        request.backend = Some(Supplied::new(StorageBackend::Postgres, "storage"));
        request.postgres_url = Some(Supplied::new(
            ConnectionString::new(database.url.as_str()),
            "postgres_url",
        ));
        request
    };
    // The default: a store that writes refuses the server.
    let enforcing = StorageSettings::resolve(request(), StoreOwner::Command).expect("settings");
    assert!(StoreFactory::builtin().open(&enforcing).is_err());
    // A command told to report, which writes the store anyway.
    let mut reporting = request();
    reporting.durability = Some(Durability::Reported);
    let reporting = StorageSettings::resolve(reporting, StoreOwner::Command).expect("settings");
    let store = StoreFactory::builtin().open(&reporting).expect("a store");
    store
        .upsert_candles(&spot(), &five_minutes())
        .expect("candles");
    // The reader, under the same default settings as the refused store.
    let settings = StorageSettings::resolve(request(), StoreOwner::Reader).expect("settings");
    let reader = StoreFactory::builtin()
        .open_reader(&settings)
        .expect("a reader on a server with fsync off");
    assert_eq!(
        reader.load(&spot(), None, None).expect("candles"),
        five_minutes()
    );
}

/// A reader of a WAL database writes the `-shm` file beside it, as SQLite
/// has any reader do: where it cannot, it is refused with what to do.
#[cfg(unix)]
#[test]
fn a_database_in_a_directory_a_reader_cannot_write_to_is_refused_with_its_remedy() {
    use std::os::unix::fs::PermissionsExt;

    let place = Place::sqlite();
    let path = place.path();
    place
        .store()
        .upsert_candles(&spot(), &five_minutes())
        .expect("candles");
    let directory = path.parent().expect("a directory");
    let mode = |mode| {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode)).expect("chmod")
    };
    mode(0o555);
    // A user who writes anyway, such as root, cannot see the refusal.
    let writable = std::fs::write(directory.join("probe"), b"").is_ok();
    let opened = place.open_reader();
    mode(0o755);
    if writable {
        eprintln!("skipping: this user writes to read-only directories");
        return;
    }
    let refused = refusal(opened);
    assert!(refused.contains("make its directory writable"), "{refused}");
}

/// A role that may read the store's tables and do nothing else is all a
/// reader needs: it issues no statement that writes.
#[cfg(feature = "postgres")]
#[test]
fn a_role_that_may_only_read_is_enough_for_a_reader() {
    use crate::postgres::query;

    /// The role's password.
    const WORD: &str = "reader-only-word";

    /// A login role on the server, with what it was granted in one
    /// database, both taken back as it drops.
    struct Role {
        server: String,
        database: String,
        name: String,
    }

    impl Drop for Role {
        fn drop(&mut self) {
            // Never a second panic while a failed test unwinds.
            let _ = std::panic::catch_unwind(|| {
                let name = &self.name;
                query(
                    &self.database,
                    &format!("REVOKE ALL ON ALL TABLES IN SCHEMA public FROM {name}"),
                );
                query(&self.server, &format!("DROP ROLE IF EXISTS {name}"));
            });
        }
    }

    let Some(place) = Place::postgres() else {
        return;
    };
    let Database::Postgres(url) = &place.database else {
        panic!("not postgres");
    };
    let candles = five_minutes();
    place
        .store()
        .upsert_candles(&spot(), &candles)
        .expect("candles");

    let server = crate::postgres::server_url().expect("the server");
    let role = Role {
        server: server.clone(),
        database: url.clone(),
        name: format!("qf_reader_{}", std::process::id()),
    };
    let name = &role.name;
    query(
        &server,
        &format!("CREATE ROLE {name} LOGIN PASSWORD '{WORD}'"),
    );
    query(
        url,
        &format!("GRANT SELECT ON ALL TABLES IN SCHEMA public TO {name}"),
    );
    let may = |privilege: &str| {
        query(
            url,
            &format!(
                "SELECT CAST(has_table_privilege('{name}', 'candles', '{privilege}') AS TEXT)"
            ),
        )
    };
    assert_eq!(may("SELECT"), [[Some("true".to_string())]]);
    assert_eq!(may("INSERT"), [[Some("false".to_string())]]);

    let mut reading = url::Url::parse(url).expect("the URL");
    reading.set_username(name).expect("a user");
    reading.set_password(Some(WORD)).expect("a password");
    let mut request = StorageRequest::default();
    request.backend = Some(Supplied::new(StorageBackend::Postgres, "storage"));
    request.postgres_url = Some(Supplied::new(
        ConnectionString::new(reading.as_str()),
        "postgres_url",
    ));
    let settings = StorageSettings::resolve(request, StoreOwner::Reader).expect("settings");
    let reader = StoreFactory::builtin()
        .open_reader(&settings)
        .expect("a reader");
    assert_eq!(reader.load(&spot(), None, None).expect("candles"), candles);
    let nothing = reader.nothing_found(&market(ExchangeId::BybitSpot, "BTCUSDT"), None, None);
    assert!(nothing.ends_with("holds no candles of bybit_spot BTCUSDT 1m"));
    // The store's settings name the role nowhere they are shown.
    assert!(!reader.location().contains(name.as_str()));
    assert!(!format!("{reader:?}").contains(WORD));
}
