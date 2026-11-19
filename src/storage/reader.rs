//! [`CandleReader`]: a store opened to read its candles, and nothing else.
//! It is what a trusted host hands to code that only reads, such as the
//! Python package's reader (`docs/python-reader.md`).
//!
//! [`StoreFactory::open_reader`](crate::storage::factory::StoreFactory::open_reader)
//! opens one from the settings a command's store is opened from, so a
//! reader and a command agree on which backend is selected and where it
//! is. A reader differs from a command's store in what it leaves alone:
//!
//! - **It creates no database and no schema.** A SQLite file that does not
//!   exist is refused, and so is a database that holds no store: a reader
//!   never makes an empty store to read nothing from. Beside a SQLite
//!   database in WAL mode it may leave the `-wal` and `-shm` files any
//!   reader of one leaves.
//! - **It migrates nothing.** A store at an earlier schema than this
//!   build's is refused with the command that brings it forward, and one
//!   at a later schema as a command refuses it. A reader built on a newer
//!   engine therefore never moves a store out from under the command line
//!   that writes it.
//! - **It writes nothing.** Every connection is read-only, SQLite's by how
//!   the file is opened and PostgreSQL's by its session, so the backend
//!   itself refuses a statement that would write.
//! - **It reads no unfinished copy.** A store that holds part of another
//!   (`docs/storage-migration.md`) is refused until the copy is finished,
//!   as every command but the copy refuses it: as the reader opens, and
//!   before and after every load, since a reader may stay open while a copy
//!   runs. A load counts only where the copy's record is the same before
//!   it and after it.

use crate::engine::ENGINE_VERSION;
use crate::model::{Candle, MarketId, TimestampMs, TransferRecord, TransferState, ms_to_rfc3339};
use crate::ports::{CandleQuery, CandleStore, StorageError, StoreControl};
use crate::storage::any::AnyStore;
use crate::storage::common::shell_word;
use crate::storage::settings::{StorageBackend, StorageSettings};
use crate::storage::sqlite::SCHEMA_VERSION;

/// How many times a load is made where a copy's record changed while it
/// read: a record changes twice at most, from none to unfinished, which
/// refuses the load, and from unfinished to finished.
const READS_BESIDE_A_COPY: usize = 3;

/// A store opened to read its candles; see the module docs.
#[derive(Debug)]
pub struct CandleReader {
    store: AnyStore,
    /// Where the store is, as an operator may see it.
    location: String,
}

impl CandleReader {
    /// A reader over `store`, which `settings` describe and which was
    /// opened to read: held to the schema this build reads, and to holding
    /// no unfinished copy, before anything is read from it.
    pub(crate) fn over(store: AnyStore, settings: &StorageSettings) -> Result<Self, StorageError> {
        let reader = Self {
            store,
            location: settings.location(),
        };
        reader.held_to_this_schema(settings)?;
        reader.copy_held()?;
        Ok(reader)
    }

    /// The backend the store is on.
    pub fn backend(&self) -> StorageBackend {
        self.store.backend()
    }

    /// Where the store is, as an operator may see it: a file's path, or
    /// `postgres://host:port/database`, never a user or password.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// The candles of `market` that opened from `start_ms` to `end_ms`,
    /// both included, oldest first. A bound left out bounds nothing: with
    /// neither, every candle of the market. A market the store holds
    /// nothing of, and a range that holds no candle, give none, which
    /// [`nothing_found`](Self::nothing_found) accounts for.
    ///
    /// A copy that runs into the store while the reader is open refuses
    /// the load while it is unfinished. One that began and finished while
    /// the load read has the load made again: the copy's record is looked
    /// at before the load and after it, and a copy writes its record before
    /// its first row and finishes it after its last, so a load whose record
    /// stayed the same read no row of a copy under way.
    pub fn load(
        &self,
        market: &MarketId,
        start_ms: Option<TimestampMs>,
        end_ms: Option<TimestampMs>,
    ) -> Result<Vec<Candle>, StorageError> {
        read_beside_copies(
            || self.copy_held(),
            || {
                self.store.load_candles(
                    market,
                    CandleQuery {
                        start_time_ms: start_ms,
                        end_time_ms: end_ms,
                        limit: None,
                    },
                )
            },
        )
    }

    /// Why a range of `market` held no candle, as one sentence for whoever
    /// asked: the store holds none of the market, or it holds them from one
    /// time to another and none where they were asked for, or the range
    /// ends before it starts. It reads the store again, so it says what the
    /// store holds now: where a writer stored candles in the range since it
    /// was read, the sentence says that the range held none as it was read;
    /// and where the store cannot be read again, it says that, and why.
    pub fn nothing_found(
        &self,
        market: &MarketId,
        start_ms: Option<TimestampMs>,
        end_ms: Option<TimestampMs>,
    ) -> String {
        self.what_is_held(market, start_ms, end_ms)
            .unwrap_or_else(|failure| {
                format!(
                    "{} held no candles of {}{} as they were read, and what it holds besides \
                     could not be read: {failure}",
                    self.location,
                    named(market),
                    asked(start_ms, end_ms)
                )
            })
    }

    /// Why a range whose two bounds both lie inside one millisecond, the one
    /// that begins at `millisecond`, held no candle: a candle opens on a
    /// whole millisecond. A host whose bounds can fall between two
    /// milliseconds says this where both did, inside the same one, and the
    /// range it asked for is then from the millisecond after the first to
    /// the one before the second.
    pub fn nothing_inside(&self, market: &MarketId, millisecond: TimestampMs) -> String {
        format!(
            "{} holds no candles of {} between two times inside the millisecond that begins at \
             {}: a candle opens on a whole millisecond",
            self.location,
            named(market),
            ms_to_rfc3339(millisecond)
        )
    }

    /// [`nothing_found`](Self::nothing_found)'s sentence, from what the
    /// store holds now.
    fn what_is_held(
        &self,
        market: &MarketId,
        start_ms: Option<TimestampMs>,
        end_ms: Option<TimestampMs>,
    ) -> Result<String, StorageError> {
        let location = &self.location;
        let market_named = named(market);
        let asked = asked(start_ms, end_ms);
        if let (Some(start), Some(end)) = (start_ms, end_ms) {
            if start > end {
                return Ok(format!(
                    "{location} holds no candles of {market_named}{asked}: the range ends before \
                     it starts"
                ));
            }
        }
        let Some((first, last)) = self.span(market)? else {
            return Ok(format!("{location} holds no candles of {market_named}"));
        };
        // A range that reaches into what the store holds now may be a hole
        // in it, or a writer may have filled it since it was read.
        let reaches =
            start_ms.is_none_or(|start| start <= last) && end_ms.is_none_or(|end| end >= first);
        let filled = reaches
            && !self
                .store
                .load_candles(
                    market,
                    CandleQuery {
                        start_time_ms: start_ms,
                        end_time_ms: end_ms,
                        limit: Some(1),
                    },
                )?
                .is_empty();
        let (first, last) = (ms_to_rfc3339(first), ms_to_rfc3339(last));
        if filled {
            return Ok(format!(
                "{location} held no candles of {market_named}{asked} as they were read; it holds \
                 them from {first} to {last} now"
            ));
        }
        Ok(format!(
            "{location} holds no candles of {market_named}{asked}; it holds them from {first} to \
             {last}"
        ))
    }

    /// The open times of the first and the last candle the store holds of
    /// `market`, or `None` where it holds none.
    fn span(&self, market: &MarketId) -> Result<Option<(TimestampMs, TimestampMs)>, StorageError> {
        let Some(last) = self.store.max_open_time_ms(market)? else {
            return Ok(None);
        };
        let first = self.store.load_candles(
            market,
            CandleQuery {
                limit: Some(1),
                ..CandleQuery::default()
            },
        )?;
        // A market emptied between the two reads holds nothing.
        Ok(first.first().map(|candle| (candle.open_time_ms, last)))
    }

    /// Refuses a store at another schema than this build reads, and a
    /// database that holds no store.
    fn held_to_this_schema(&self, settings: &StorageSettings) -> Result<(), StorageError> {
        let location = &self.location;
        let found = match &self.store {
            AnyStore::Sqlite(store) => store.recorded_version()?,
            #[cfg(feature = "postgres")]
            AnyStore::Postgres(store) => store.recorded_version()?,
        };
        if found == SCHEMA_VERSION {
            return Ok(());
        }
        if found > SCHEMA_VERSION {
            return Err(StorageError::SchemaUnsupported {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found == 0 {
            return Err(StorageError::InvalidArgument(format!(
                "{location} holds no QuantForge store; a reader creates none, and `quantforge \
                 data sync` does"
            )));
        }
        let forward = match settings {
            StorageSettings::Sqlite(sqlite) => {
                format!(
                    "`quantforge --db {} store status`",
                    shell_word(&sqlite.path)
                )
            }
            StorageSettings::Postgres(_) => {
                "`quantforge --storage postgres store status` with the same URL".to_string()
            }
        };
        Err(StorageError::InvalidArgument(format!(
            "{location} is at schema {found}, and this build reads schema {SCHEMA_VERSION}; a \
             reader migrates nothing: bring the store forward with the quantforge command line \
             of this version ({ENGINE_VERSION}), where any command does it in place, such as \
             {forward}"
        )))
    }

    /// The record of the copy the store holds, `None` where it holds none:
    /// a store that holds part of another, an unfinished copy, is refused,
    /// since its candles are not all there yet.
    fn copy_held(&self) -> Result<Option<TransferRecord>, StorageError> {
        let Some(target) = self.store.transfer_target() else {
            return Ok(None);
        };
        let Some(record) = target.transfer_record()? else {
            return Ok(None);
        };
        if record.state != TransferState::Copying {
            return Ok(Some(record));
        }
        let identity = self.store.store_identity()?;
        Err(StorageError::InvalidArgument(format!(
            "store {} holds an unfinished copy of store {}, begun {}; finish it with `quantforge \
             data migrate --from <the source>` before reading it",
            identity.store_id,
            record.source_store_id,
            ms_to_rfc3339(record.started_at_ms)
        )))
    }
}

/// What `read` gives, read where no copy into the store ran: `copy` is the
/// copy's record, which refuses an unfinished one, looked at before the
/// read and after it, and the read counts where the two are the same. A
/// record that changed means a copy began and finished while `read` read,
/// and the read is made again, after it.
fn read_beside_copies<R: PartialEq, T>(
    mut copy: impl FnMut() -> Result<R, StorageError>,
    mut read: impl FnMut() -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let mut before = copy()?;
    for _ in 0..READS_BESIDE_A_COPY {
        let read_then = read()?;
        let after = copy()?;
        if after == before {
            return Ok(read_then);
        }
        before = after;
    }
    Err(StorageError::Contention(
        "a copy into the store changed its record each time its candles were read; read them \
         again"
            .to_string(),
    ))
}

/// A range as a sentence names it: from one time to another, from one on,
/// up to one, or nothing where neither bound is given.
fn asked(start_ms: Option<TimestampMs>, end_ms: Option<TimestampMs>) -> String {
    match (start_ms, end_ms) {
        (Some(start), Some(end)) => {
            format!(" from {} to {}", ms_to_rfc3339(start), ms_to_rfc3339(end))
        }
        (Some(start), None) => format!(" from {} on", ms_to_rfc3339(start)),
        (None, Some(end)) => format!(" up to {}", ms_to_rfc3339(end)),
        (None, None) => String::new(),
    }
}

/// A market as a message names it: its product line, its symbol and its
/// interval, such as `binance_spot BTCUSDT 1m`.
fn named(market: &MarketId) -> String {
    format!(
        "{} {} {}",
        market.exchange.as_str(),
        market.symbol.as_str(),
        market.interval.as_str()
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A copy's record as `read_beside_copies` sees it, step by step:
    /// `Ok(n)` a record, the same one where `n` is, and `Err` an unfinished
    /// copy.
    fn scripted(records: &[Result<u32, &str>]) -> impl FnMut() -> Result<u32, StorageError> {
        let mut next = records.iter();
        move || match next.next().expect("a record for every look") {
            Ok(record) => Ok(*record),
            Err(refused) => Err(StorageError::InvalidArgument(refused.to_string())),
        }
    }

    #[test]
    fn a_read_counts_where_the_copy_record_stayed_the_same() {
        let reads = RefCell::new(0);
        let read = || {
            *reads.borrow_mut() += 1;
            Ok(*reads.borrow())
        };
        let read = read_beside_copies(scripted(&[Ok(0), Ok(0)]), read).expect("read");
        assert_eq!(read, 1);
    }

    #[test]
    fn a_copy_that_began_and_finished_while_the_candles_were_read_has_them_read_again() {
        let reads = RefCell::new(0);
        let read = || {
            *reads.borrow_mut() += 1;
            Ok(*reads.borrow())
        };
        // No record before the first read, a finished copy's after it: the
        // second read, made after the copy, is what counts.
        let read = read_beside_copies(scripted(&[Ok(0), Ok(1), Ok(1)]), read).expect("read");
        assert_eq!(read, 2);
    }

    #[test]
    fn an_unfinished_copy_before_or_after_the_read_refuses_it() {
        for records in [
            vec![Err("unfinished")],
            vec![Ok(0), Err("unfinished")],
            vec![Ok(0), Ok(1), Err("unfinished")],
        ] {
            let refused = read_beside_copies(scripted(&records), || Ok(())).expect_err("refused");
            assert_eq!(refused.to_string(), "invalid argument: unfinished");
        }
    }

    #[test]
    fn a_record_that_keeps_changing_is_contention_after_three_reads() {
        let reads = RefCell::new(0);
        let read = || {
            *reads.borrow_mut() += 1;
            Ok(())
        };
        let refused =
            read_beside_copies(scripted(&[Ok(0), Ok(1), Ok(2), Ok(3)]), read).expect_err("refused");
        assert!(
            matches!(refused, StorageError::Contention(_)),
            "{refused:?}"
        );
        assert_eq!(*reads.borrow(), 3);
    }
}
