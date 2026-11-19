//! [`CandleReader`]: a store opened to read its candles, and nothing else.
//! It is what a trusted host hands to code that only reads, such as the
//! Python package's reader (`docs/python-reader.md`).
//!
//! [`StoreFactory::open_reader`](crate::storage::factory::StoreFactory::open_reader)
//! opens one from the settings a command's store is opened from, so a
//! reader and a command agree on which backend is selected and where it
//! is. A reader differs from a command's store in what it leaves alone:
//!
//! - **It creates nothing.** A SQLite file that does not exist is refused,
//!   and so is a database that holds no store: a reader never makes an
//!   empty store to read nothing from.
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
//!   again after every load, since a reader may stay open while a copy
//!   begins.

use crate::engine::ENGINE_VERSION;
use crate::model::{Candle, MarketId, TimestampMs, TransferState, ms_to_rfc3339};
use crate::ports::{CandleQuery, CandleStore, StorageError, StoreControl};
use crate::storage::any::AnyStore;
use crate::storage::settings::{StorageBackend, StorageSettings};
use crate::storage::sqlite::SCHEMA_VERSION;

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
        reader.held_to_no_unfinished_copy()?;
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
    /// A copy that began into the store since the reader opened refuses
    /// the load: the record of a copy is written before its first row, so
    /// a load that read any row of an unfinished copy finds the record
    /// after it.
    pub fn load(
        &self,
        market: &MarketId,
        start_ms: Option<TimestampMs>,
        end_ms: Option<TimestampMs>,
    ) -> Result<Vec<Candle>, StorageError> {
        let candles = self.store.load_candles(
            market,
            CandleQuery {
                start_time_ms: start_ms,
                end_time_ms: end_ms,
                limit: None,
            },
        )?;
        self.held_to_no_unfinished_copy()?;
        Ok(candles)
    }

    /// Why a range of `market` held no candle, as one sentence for whoever
    /// asked: the store holds none of the market, or it holds them from one
    /// time to another and none where they were asked for, or the range
    /// ends before it starts. It reads the store again, so it says what the
    /// store holds now: where a writer stored candles in the range since it
    /// was read, the sentence says that the range held none as it was read.
    pub fn nothing_found(
        &self,
        market: &MarketId,
        start_ms: Option<TimestampMs>,
        end_ms: Option<TimestampMs>,
    ) -> Result<String, StorageError> {
        let location = &self.location;
        let market_named = named(market);
        let asked = match (start_ms, end_ms) {
            (Some(start), Some(end)) => {
                format!(" from {} to {}", ms_to_rfc3339(start), ms_to_rfc3339(end))
            }
            (Some(start), None) => format!(" from {} on", ms_to_rfc3339(start)),
            (None, Some(end)) => format!(" up to {}", ms_to_rfc3339(end)),
            (None, None) => String::new(),
        };
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
                format!("`quantforge --db {} store status`", sqlite.path.display())
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

    /// Refuses a store that holds part of another: its candles are not all
    /// there yet.
    fn held_to_no_unfinished_copy(&self) -> Result<(), StorageError> {
        let Some(target) = self.store.transfer_target() else {
            return Ok(());
        };
        let Some(record) = target.transfer_record()? else {
            return Ok(());
        };
        if record.state != TransferState::Copying {
            return Ok(());
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
