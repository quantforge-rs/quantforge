//! The fixture copied into PostgreSQL, an isolated database per test on
//! the server `QUANTFORGE_TEST_POSTGRES_URL` names; without it the tests
//! skip. The copy verifies with the oracle's checksums on a byte-ordered
//! and on a linguistic collation, reads back through every port as the
//! source does, numbers new rows past the copied ones, converges when it
//! is cut off and run again, and refuses a target that holds rows of its
//! own. While a copy is unfinished every other write is refused; a write
//! racing the copy's beginning either commits first, and the copy refuses
//! its rows, or waits and is refused; and a numbered table whose keys are
//! all at or below zero keeps its numbering.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use quantforge::model::futures::{
    OwnerTag, PolicyChange, ProfileId, ProfileIdentity, ProfileLabel, ProfilePolicy,
};
use quantforge::model::{
    AccountRef, Category, IdentitySource, RunIdentity, TRANSFER_TABLES, TransferMarket,
    TransferRow, TransferTable, TransferValue, Venue, VenueAccountId,
};
use quantforge::storage::sqlite::SqliteSnapshot;
use quantforge::{
    AnyStore, CandleQuery, CandleStore, ClosedTrade, CopyStart, EngineError, Environment,
    ExchangeOrder, MarketId, PolicyStore, RunJournalStore, SegmentStore, SqliteStore, StorageError,
    StoreActivation, StoreControl, StoreFactory, StoreId, StoreIdentity, TransferBegun,
    TransferConfig, TransferEngine, TransferRecord, TransferSource, TransferState, TransferTarget,
};
use tempfile::TempDir;

use super::fixture::{self, Cases, Side, scopes_of};
use crate::postgres::{
    IsolatedDatabase, VARIABLE, client, query, selection, server_url, wait_for_a_lock,
};

const LINGUISTIC: &str = "TEMPLATE template0 LC_COLLATE 'en_US.UTF-8' LC_CTYPE 'en_US.UTF-8'";

/// A seeded source, and a target on a database of its own.
struct Bed {
    _dir: TempDir,
    path: PathBuf,
    cases: Cases,
    database: IsolatedDatabase,
    target: AnyStore,
}

impl Bed {
    fn new(options: &str) -> Option<Self> {
        let Some(server) = server_url() else {
            eprintln!("skipping: {VARIABLE} is unset");
            return None;
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("source.sqlite");
        let cases = fixture::seed(&path);
        let database = IsolatedDatabase::create_with(&server, options);
        let target = StoreFactory::builtin()
            .open(&selection(&database.url))
            .expect("the target");
        Some(Self {
            _dir: dir,
            path,
            cases,
            database,
            target,
        })
    }

    fn snapshot(&self) -> SqliteSnapshot {
        SqliteSnapshot::open(&self.path).expect("the source")
    }

    fn target(&self) -> &dyn TransferTarget {
        self.target.transfer_target().expect("a target")
    }

    fn engine<'a>(
        &'a self,
        snapshot: &'a SqliteSnapshot,
        target: &'a dyn TransferTarget,
        batch_rows: usize,
    ) -> TransferEngine<'a> {
        TransferEngine::new(
            snapshot,
            target,
            TransferConfig {
                batch_rows,
                assume_quiescent: false,
            },
        )
    }

    fn copy(&self, batch_rows: usize) -> quantforge::TransferSummary {
        let snapshot = self.snapshot();
        self.engine(&snapshot, self.target(), batch_rows)
            .copy()
            .expect("copy")
    }

    fn verify(&self) -> quantforge::VerifyReport {
        let snapshot = self.snapshot();
        self.engine(&snapshot, self.target(), 3)
            .verify()
            .expect("verify")
    }

    fn count(&self, sql: &str) -> String {
        query(&self.database.url, sql)[0][0]
            .clone()
            .unwrap_or_default()
    }
}

fn market(exchange: &str, symbol: &str, interval: &str) -> MarketId {
    let exchange = quantforge::ExchangeId::from_storage_name(exchange).expect("a product line");
    MarketId::new(
        exchange,
        quantforge::Symbol::new(symbol).expect("a symbol"),
        interval.parse().expect("an interval"),
    )
}

#[test]
fn the_fixture_copies_and_verifies_with_the_oracle_checksums_on_either_collation() {
    for options in ["", LINGUISTIC] {
        let Some(bed) = Bed::new(options) else {
            return;
        };
        let summary = bed.copy(2);
        assert_eq!(summary.start, CopyStart::Started);
        for copied in &summary.tables {
            let rows = bed.cases.table(copied.table).rows.len() as u64;
            assert_eq!(
                (copied.rows, copied.written),
                (rows, rows),
                "{}",
                copied.table
            );
        }
        assert_eq!(summary.record.state, TransferState::Copied);

        let report = bed.verify();
        assert!(report.is_ok(), "{options}: {:?}", report.differences);
        assert_eq!(
            scopes_of(&report, Side::Source),
            bed.cases.scopes,
            "{options}"
        );
        assert_eq!(
            scopes_of(&report, Side::Target),
            bed.cases.scopes,
            "{options}"
        );
    }
}

#[test]
fn every_port_reads_the_copy_as_it_reads_the_source() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    bed.copy(5000);
    let source = SqliteStore::new(&bed.path);
    let target = &bed.target;

    let mut markets = vec![
        market("binance_spot", "BTCUSDT", "1m"),
        market("bybit_linear", "BTCUSDT", "1m"),
        market("binance_inverse", "BTCUSD", "5m"),
        market("binance_spot", "币安人生USDT", "1m"),
    ];
    for market in &markets {
        let all = CandleQuery::default();
        assert_eq!(
            target
                .load_candles(market, all.clone())
                .expect("target candles"),
            source.load_candles(market, all).expect("source candles"),
            "{market:?}"
        );
    }
    // Text no Decimal holds is refused alike on both sides.
    markets.push(market("binance_spot", "ETHUSDT", "1m"));
    let eth = markets.last().expect("eth");
    let refused = |store: &dyn CandleStore| match store.load_candles(eth, CandleQuery::default()) {
        Err(StorageError::InvalidArgument(reason)) => reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused(target), refused(&source));

    for row in &bed.cases.table("bot_runs").rows {
        let TransferValue::Text(run_id) = &row[0] else {
            panic!("a run id");
        };
        assert_eq!(
            target.run_identity(run_id).expect("target identity"),
            source.run_identity(run_id).expect("source identity"),
            "{run_id}"
        );
        if run_id != "run-legacy" {
            assert_eq!(
                target.load_run_state(run_id).expect("target state"),
                source.load_run_state(run_id).expect("source state"),
                "{run_id}"
            );
        }
        assert_eq!(
            target
                .list_order_events(run_id, 100)
                .expect("target events"),
            source
                .list_order_events(run_id, 100)
                .expect("source events"),
            "{run_id}"
        );
        assert_eq!(
            target
                .list_closed_trades(run_id, 100)
                .expect("target trades"),
            source
                .list_closed_trades(run_id, 100)
                .expect("source trades"),
            "{run_id}"
        );
    }
    assert_eq!(
        target.legacy_runs().expect("target legacy"),
        source.legacy_runs().expect("source legacy")
    );

    let spot = &markets[0];
    let (first, last) = (1_792_454_400_000, 1_792_454_400_000 + 86_400_000 + 60_000);
    let plan = target
        .resolve_segments(spot, first, last)
        .expect("target plan");
    assert_eq!(
        plan,
        source
            .resolve_segments(spot, first, last)
            .expect("source plan")
    );
    for segment in &plan.segments {
        let bytes = target
            .load_segment(&segment.hash)
            .expect("load")
            .expect("stored");
        assert_eq!(
            Some(bytes.clone()),
            source.load_segment(&segment.hash).expect("load")
        );
        let candles = quantforge::storage::segments::open(&bytes, &segment.hash).expect("open");
        assert_eq!(candles.len(), 3);
    }

    let (theirs, ours) = (
        source.store_identity().expect("source record"),
        target.store_identity().expect("target record"),
    );
    assert_eq!(ours.environment, Some(Environment::Testnet));
    assert_ne!(ours.store_id, theirs.store_id);
    assert_eq!(ours.activation, StoreActivation::Inactive);
    assert_eq!(ours.activation_epoch.0, 0);
}

/// The fixture's text of `profile`'s policy at `revision`.
fn fixture_policy(cases: &Cases, profile: &str, revision: u64) -> String {
    let revision = i64::try_from(revision).expect("a revision");
    cases
        .table("policy_revisions")
        .rows
        .iter()
        .find_map(|row| match (&row[1], &row[2], &row[4]) {
            (
                TransferValue::Text(id),
                TransferValue::Integer(number),
                TransferValue::Text(text),
            ) if id == profile && *number == revision => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the fixture has no revision {revision} of {profile}"))
}

#[test]
fn every_profile_and_every_revision_of_its_policy_reads_from_the_copy_as_from_the_source() {
    // On a linguistic collation, which would list the profiles otherwise.
    let Some(bed) = Bed::new(LINGUISTIC) else {
        return;
    };
    bed.copy(3);
    let source = SqliteStore::new(&bed.path);
    let target = &bed.target;
    let profiles = target.profiles().expect("target profiles");
    assert_eq!(profiles, source.profiles().expect("source profiles"));
    let ids: Vec<&str> = profiles.iter().map(|held| held.id().as_str()).collect();
    assert_eq!(ids, ["main", "main-2", "main0", "main_3"], "byte by byte");

    let mut revisions = 0;
    for identity in &profiles {
        let id = identity.id();
        let stamps = target.policy_revisions(id).expect("target stamps");
        assert_eq!(
            stamps,
            source.policy_revisions(id).expect("source stamps"),
            "{id}"
        );
        for stamp in &stamps {
            let held = target
                .profile_policy_at(id, stamp.revision)
                .expect("a read")
                .expect("the revision");
            let theirs = source
                .profile_policy_at(id, stamp.revision)
                .expect("a read");
            assert_eq!(Some(&held), theirs.as_ref(), "{id} {}", stamp.revision);
            assert_eq!(held.identity(), identity, "{id}");
            // The oracle's document is, byte for byte, the one this build
            // writes of the policy it reads from it.
            assert_eq!(
                held.to_stored().expect("json"),
                fixture_policy(&bed.cases, id.as_str(), stamp.revision.get()),
                "{id} {}",
                stamp.revision
            );
            revisions += 1;
        }
        assert_eq!(
            target.profile_policy(id).expect("the newest"),
            source.profile_policy(id).expect("the newest"),
            "{id}"
        );
    }
    assert_eq!(revisions, bed.cases.table("policy_revisions").rows.len());

    // A change made on the copy is the profile's next revision, numbered
    // past every row copied, whichever profile they belong to.
    let main = ProfileId::new("main").expect("a profile id");
    let newest = target
        .profile_policy(&main)
        .expect("a read")
        .expect("main's");
    assert_eq!(newest.revision().get(), 4);
    let label = ProfileLabel::new("on the copy").expect("a label");
    let next = newest
        .apply(&PolicyChange::Label(Some(label)))
        .expect("a change the policy takes")
        .expect("a change that changes it");
    assert!(
        target
            .save_policy(&next, 1_792_454_600_000)
            .expect("a save")
    );
    assert_eq!(bed.count("SELECT max(seq) FROM policy_revisions"), "12");
    assert_eq!(bed.count("SELECT count(*) FROM policy_revisions"), "9");
    assert_eq!(
        target.profile_policy(&main).expect("a read"),
        Some(next),
        "the copy's own revision"
    );
}

#[test]
fn the_copy_numbers_new_rows_past_the_copied_ones() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    bed.copy(5000);
    let source = SqliteStore::new(&bed.path);
    let order: ExchangeOrder = source
        .list_order_events("run-a", 1)
        .expect("events")
        .remove(0);
    let trade: ClosedTrade = source
        .list_closed_trades("run-a", 1)
        .expect("trades")
        .remove(0);
    let spot = market("binance_spot", "BTCUSDT", "1m");
    let mut candle = source
        .load_candles(&spot, CandleQuery::default())
        .expect("candles")
        .remove(0);
    candle.open_time_ms += 3 * 86_400_000;
    candle.close_time_ms += 3 * 86_400_000;

    bed.target
        .append_order_event("run-a", &order)
        .expect("an event");
    bed.target
        .append_closed_trade("run-a", &trade)
        .expect("a trade");
    bed.target
        .upsert_candles(&spot, &[candle])
        .expect("a candle");

    assert_eq!(bed.count("SELECT max(seq) FROM order_events"), "11");
    assert_eq!(bed.count("SELECT max(seq) FROM closed_trades"), "5");
    assert_eq!(bed.count("SELECT max(id) FROM segment_dirty"), "14");
    assert_eq!(bed.count("SELECT count(*) FROM segment_dirty"), "5");
}

/// The real target, failing its writes once the given number is spent.
struct Cut<'a> {
    inner: &'a dyn TransferTarget,
    writes_left: AtomicUsize,
}

impl TransferSource for Cut<'_> {
    fn transfer_identity(&self) -> Result<StoreIdentity, StorageError> {
        self.inner.transfer_identity()
    }

    fn markets(&self, table: &TransferTable) -> Result<Vec<TransferMarket>, StorageError> {
        self.inner.markets(table)
    }

    fn read_rows(
        &self,
        table: &TransferTable,
        market: Option<&TransferMarket>,
        after: Option<&TransferValue>,
        limit: usize,
    ) -> Result<Vec<TransferRow>, StorageError> {
        self.inner.read_rows(table, market, after, limit)
    }
}

impl TransferTarget for Cut<'_> {
    fn transfer_record(&self) -> Result<Option<TransferRecord>, StorageError> {
        self.inner.transfer_record()
    }

    fn begin_transfer(
        &self,
        record: &TransferRecord,
        environment: Option<Environment>,
    ) -> Result<TransferBegun, StorageError> {
        self.inner.begin_transfer(record, environment)
    }

    fn write_rows(&self, table: &TransferTable, rows: &[TransferRow]) -> Result<u64, StorageError> {
        match self.writes_left.load(Ordering::SeqCst) {
            0 => Err(StorageError::OutcomeUnknown("cut off".to_string())),
            left => {
                self.writes_left.store(left - 1, Ordering::SeqCst);
                self.inner.write_rows(table, rows)
            }
        }
    }

    fn finish_transfer(&self, record: &TransferRecord) -> Result<(), StorageError> {
        self.inner.finish_transfer(record)
    }
}

#[test]
fn a_copy_cut_off_and_run_again_converges_on_what_a_whole_copy_writes() {
    for cut in [0, 4, 9, 23] {
        let Some(bed) = Bed::new("") else {
            return;
        };
        let snapshot = bed.snapshot();
        let cut_target = Cut {
            inner: bed.target(),
            writes_left: AtomicUsize::new(cut),
        };
        let error = bed
            .engine(&snapshot, &cut_target, 1)
            .copy()
            .expect_err("cut off");
        assert!(
            matches!(error, EngineError::CopyStopped { .. }),
            "cut {cut}: {error:?}"
        );
        let record = bed
            .target()
            .transfer_record()
            .expect("record")
            .expect("begun");
        assert_eq!(record.state, TransferState::Copying, "cut {cut}");

        let summary = bed.copy(1);
        assert_eq!(summary.start, CopyStart::Resumed, "cut {cut}");
        let written: u64 = summary.tables.iter().map(|copied| copied.written).sum();
        let all: u64 = bed.cases.tables.iter().map(|t| t.rows.len() as u64).sum();
        assert_eq!(written, all - cut as u64, "cut {cut}");

        let report = bed.verify();
        assert!(report.is_ok(), "cut {cut}: {:?}", report.differences);
        assert_eq!(
            scopes_of(&report, Side::Target),
            bed.cases.scopes,
            "cut {cut}"
        );

        let again = bed.copy(1);
        assert_eq!(again.start, CopyStart::AlreadyCopied, "cut {cut}");
        assert_eq!(scopes_of(&bed.verify(), Side::Target), bed.cases.scopes);
    }
}

#[test]
fn a_target_holding_rows_of_its_own_is_refused_and_left_as_it_was() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    let spot = market("binance_spot", "BTCUSDT", "1m");
    let candle = SqliteStore::new(&bed.path)
        .load_candles(&spot, CandleQuery::default())
        .expect("candles")
        .remove(0);
    bed.target
        .upsert_candles(&spot, &[candle])
        .expect("a candle");

    let snapshot = bed.snapshot();
    match bed.engine(&snapshot, bed.target(), 5000).copy() {
        Err(EngineError::TransferRefused(reason)) => {
            assert!(
                reason.contains("rows of its own (candles, segment_dirty)"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(bed.target().transfer_record().expect("record").is_none());
    assert_eq!(bed.count("SELECT count(*) FROM candles"), "1");
    assert_eq!(
        bed.count("SELECT environment IS NULL FROM store_control"),
        "t"
    );
}

#[test]
fn a_mismatch_planted_in_the_copy_is_found_and_named() {
    let Some(bed) = Bed::new(LINGUISTIC) else {
        return;
    };
    bed.copy(5000);
    query(
        &bed.database.url,
        "UPDATE candles SET volume = '0.50' WHERE symbol = 'BTCUSDT' AND exchange = \
         'binance_spot' AND volume = '0.5'; DELETE FROM segment_dirty WHERE id = 7; UPDATE \
         bot_runs SET identity_source = 'recorded' WHERE run_id = 'run-legacy'",
    );

    let report = bed.verify();
    assert!(!report.is_ok());
    let found: Vec<String> = report.differences.iter().map(ToString::to_string).collect();
    assert_eq!(
        found,
        [
            "candles binance_spot BTCUSDT 1m open_time_ms=1792454520000: volume differs: \
             source \"0.5\", target \"0.50\"",
            "bot_runs run_id=\"run-legacy\": identity_source differs: source \"legacy\", \
             target \"recorded\"",
            "segment_dirty id=7: only in the source",
        ]
    );
}

/// Each transferred table's row count, in copy order.
fn counts(bed: &Bed) -> Vec<String> {
    TRANSFER_TABLES
        .iter()
        .map(|table| bed.count(&format!("SELECT count(*) FROM {}", table.name)))
        .collect()
}

/// Asserts that `result` is the refusal of a write to a store holding an
/// unfinished copy of store `source`.
fn assert_refused<T: std::fmt::Debug>(write: &str, result: Result<T, StorageError>, source: &str) {
    match result {
        Err(StorageError::InvalidArgument(reason)) => assert!(
            reason.contains(&format!("holds an unfinished copy of store {source}")),
            "{write}: {reason}"
        ),
        other => panic!("{write}: {other:?}"),
    }
}

#[test]
fn every_write_to_a_target_whose_copy_is_unfinished_is_refused_until_it_finishes() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    // Every row but the last two marks: the target holds runs, journals and
    // segments, and its copy is unfinished.
    let snapshot = bed.snapshot();
    let cut = Cut {
        inner: bed.target(),
        writes_left: AtomicUsize::new(29),
    };
    bed.engine(&snapshot, &cut, 1).copy().expect_err("cut off");
    let source = snapshot.transfer_identity().expect("identity").store_id;
    let source = source.as_str();
    let store = &bed.target;
    let spot = market("binance_spot", "BTCUSDT", "1m");
    let mut candle = store
        .load_candles(&spot, CandleQuery::default())
        .expect("candles")
        .remove(0);
    candle.open_time_ms += 3 * 86_400_000;
    candle.close_time_ms += 3 * 86_400_000;
    let state = store.load_run_state("run-a").expect("load").expect("run-a");
    let mut another = state.clone();
    another.run_id = "run-new".to_string();
    let identity = store.run_identity("run-a").expect("read").expect("run-a's");
    let adopted = RunIdentity {
        environment: Some(Environment::Testnet),
        account: None,
        source: IdentitySource::Adopted,
    };
    let order = store
        .list_order_events("run-a", 1)
        .expect("events")
        .remove(0);
    let trade = store
        .list_closed_trades("run-a", 1)
        .expect("trades")
        .remove(0);
    // A mark below the tip, written behind the store's back, gives a refresh
    // a window to write back.
    let window = "exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                  AND window_start_ms = 1792454400000";
    query(
        &bed.database.url,
        "INSERT INTO segment_dirty (exchange, symbol, interval, window_start_ms) \
         VALUES ('binance_spot', 'BTCUSDT', '1m', 1792454400000)",
    );
    // And a profile with its first revision, as the copy will bring them:
    // a change to it must find the copy unfinished, not the profile missing.
    let profile = |name: &str, account: &str, tag: &str| {
        let account = VenueAccountId::issued(account).expect("an account id");
        ProfilePolicy::new(
            ProfileIdentity::new(
                ProfileId::new(name).expect("a profile id"),
                AccountRef::new(Venue::Bybit, Environment::Testnet, account).expect("an account"),
                Category::Linear,
                OwnerTag::new(tag).expect("an owner tag"),
            )
            .expect("an identity"),
        )
    };
    let held = profile("held", "71002298", "held00");
    query(
        &bed.database.url,
        "INSERT INTO automation_profiles VALUES \
         ('held', 'bybit', 'testnet', 'venue', '71002298', 'linear', 'held00', 1)",
    );
    query(
        &bed.database.url,
        &format!(
            "INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json) \
             VALUES ('held', 1, 1, '{}')",
            held.to_stored().expect("json")
        ),
    );
    assert_eq!(
        store.profile_policy(held.identity().id()).expect("a read"),
        Some(held.clone())
    );
    let before = counts(&bed);

    let upserted = store.upsert_candles(&spot, std::slice::from_ref(&candle));
    assert_refused("upsert_candles", upserted, source);
    assert_refused("create_run", store.create_run(&another, &identity), source);
    assert_refused("save_run_state", store.save_run_state(&state), source);
    let appended = store.append_order_event("run-a", &order);
    assert_refused("append_order_event", appended, source);
    let appended = store.append_closed_trade("run-a", &trade);
    assert_refused("append_closed_trade", appended, source);
    let bound = store.bind_environment(Environment::Testnet);
    assert_refused("bind_environment", bound, source);
    let adoption = store.adopt_run("run-legacy", &adopted, None);
    assert_refused("adopt_run", adoption, source);
    assert_refused("refresh_segments", store.refresh_segments(&spot), source);
    // A profile of the store's own environment, which the copy's beginning
    // bound it to, and the next revision of the one the store holds.
    let created = profile("late", "71002299", "late00");
    assert_refused("create_profile", store.create_profile(&created, 1), source);
    let label = ProfileLabel::new("late").expect("a label");
    let changed = held
        .apply(&PolicyChange::Label(Some(label)))
        .expect("a change the policy takes")
        .expect("a change that changes it");
    assert_refused("save_policy", store.save_policy(&changed, 2), source);
    assert_eq!(counts(&bed), before);

    query(
        &bed.database.url,
        &format!("DELETE FROM segment_dirty WHERE {window}"),
    );
    for table in ["policy_revisions", "automation_profiles"] {
        query(
            &bed.database.url,
            &format!("DELETE FROM {table} WHERE profile_id = 'held'"),
        );
    }
    assert_eq!(bed.copy(1).start, CopyStart::Resumed);
    store
        .append_order_event("run-a", &order)
        .expect("an event, once the copy is finished");
    store
        .upsert_candles(&spot, &[candle])
        .expect("a candle, once the copy is finished");
}

#[test]
fn a_copy_beginning_waits_for_a_writer_holding_a_table_and_then_refuses_its_rows() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let writer = runtime.block_on(client(&bed.database.url));
    runtime.block_on(async {
        writer
            .batch_execute(
                "BEGIN; INSERT INTO candles VALUES ('binance_spot', 'BTCUSDT', '1m', 0, 59999, \
                 '1.0', '1.0', '1.0', '1.0', '1.0', 1)",
            )
            .await
            .expect("a candle written and not committed");
    });
    std::thread::scope(|scope| {
        let copy = scope.spawn(|| {
            let snapshot = bed.snapshot();
            bed.engine(&snapshot, bed.target(), 5000).copy()
        });
        wait_for_a_lock(&bed.database.url);
        runtime.block_on(async { writer.batch_execute("COMMIT").await.expect("commit") });
        match copy.join().expect("the copy's thread") {
            Err(EngineError::TransferRefused(reason)) => {
                assert!(reason.contains("rows of its own (candles)"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    });
    assert!(bed.target().transfer_record().expect("record").is_none());
    assert_eq!(bed.count("SELECT count(*) FROM candles"), "1");
}

/// Begins a copy in raw SQL, as `begin_transfer` does, holds it open while
/// `write` runs on another thread until the write waits on its locks, then
/// commits: the write must find the copy's record and write nothing.
fn refused_behind_a_beginning_copy<W>(bed: &Bed, write: W)
where
    W: FnOnce() -> Result<(), StorageError> + Send,
{
    const SOURCE: &str = "0123456789abcdef0123456789abcdef";
    let record = TransferRecord::begin(StoreId::new(SOURCE).expect("an ID"), 1).to_json();
    let tables: Vec<&str> = TRANSFER_TABLES.iter().map(|table| table.name).collect();
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let copy = runtime.block_on(client(&bed.database.url));
    runtime.block_on(async {
        copy.batch_execute(&format!(
            "BEGIN; DELETE FROM meta WHERE key = 'transfer'; LOCK TABLE {} IN SHARE MODE",
            tables.join(", ")
        ))
        .await
        .expect("the copy begun");
        copy.execute(
            "INSERT INTO meta (key, value) VALUES ('transfer', $1)",
            &[&record],
        )
        .await
        .expect("the copy's record");
    });
    let before = counts(bed);
    std::thread::scope(|scope| {
        let waiting = scope.spawn(write);
        wait_for_a_lock(&bed.database.url);
        runtime.block_on(async { copy.batch_execute("COMMIT").await.expect("commit") });
        assert_refused(
            "the waiting write",
            waiting.join().expect("its thread"),
            SOURCE,
        );
    });
    assert_eq!(counts(bed), before);
}

#[test]
fn a_write_waiting_on_a_copy_beginning_is_refused_once_the_copy_is_recorded() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    let source = SqliteStore::new(&bed.path);
    let spot = market("binance_spot", "BTCUSDT", "1m");
    let candles = source
        .load_candles(&spot, CandleQuery::default())
        .expect("candles");
    let order = source
        .list_order_events("run-a", 1)
        .expect("events")
        .remove(0);
    // A transaction that asks after its first statement, then a statement
    // that carries the guard in its own condition.
    refused_behind_a_beginning_copy(&bed, || {
        bed.target.upsert_candles(&spot, &candles).map(|_| ())
    });
    refused_behind_a_beginning_copy(&bed, || bed.target.append_order_event("run-a", &order));
}

#[test]
fn numbered_tables_whose_keys_are_all_at_or_below_zero_keep_their_numbering() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    rusqlite::Connection::open(&bed.path)
        .expect("the source")
        .execute_batch(
            "UPDATE order_events SET seq = seq - 100; UPDATE closed_trades SET seq = seq - 100; \
             UPDATE segment_dirty SET id = id - 100;",
        )
        .expect("every number moved below zero");
    assert_eq!(bed.copy(5000).record.state, TransferState::Copied);

    let source = SqliteStore::new(&bed.path);
    let order = source
        .list_order_events("run-a", 1)
        .expect("events")
        .remove(0);
    let trade = source
        .list_closed_trades("run-a", 1)
        .expect("trades")
        .remove(0);
    bed.target
        .append_order_event("run-a", &order)
        .expect("an event");
    bed.target
        .append_closed_trade("run-a", &trade)
        .expect("a trade");
    assert_eq!(bed.count("SELECT max(seq) FROM order_events"), "1");
    assert_eq!(bed.count("SELECT max(seq) FROM closed_trades"), "1");
}
