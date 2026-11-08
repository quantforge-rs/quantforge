//! [`AnyStore`]: the store a host opened, whichever backend it is, behind
//! every storage port at once. MSRV 1.85 has no trait upcasting, so one
//! trait object cannot stand in for the others; this enum implements each
//! port by handing the call to its backend (ADR 0001).
//!
//! PostgreSQL is a variant of its own with the `postgres` feature, every
//! store of it behind the same ports. A host gets either one from
//! [`crate::storage::factory`].

use std::time::Duration;

use crate::model::{
    BotRunState, Candle, ClosedTrade, DurabilityReport, Environment, ExchangeOrder, ExecutionMode,
    LegacyRun, MarketCoverage, MarketId, RunIdentity, SchemaMigration, SegmentHash, SegmentPlan,
    SegmentRefresh, StoreIdentity, TimestampMs,
};
use crate::ports::{
    CandleQuery, CandleStore, RunJournalStore, SegmentStore, StorageError, StoreControl,
    TransferTarget,
};
use crate::storage::SqliteStore;
#[cfg(feature = "postgres")]
use crate::storage::postgres::PostgresStore;
use crate::storage::settings::StorageBackend;

/// An open store of any backend; see the module docs.
#[derive(Debug)]
#[non_exhaustive]
pub enum AnyStore {
    Sqlite(SqliteStore),
    #[cfg(feature = "postgres")]
    Postgres(PostgresStore),
}

impl AnyStore {
    /// The backend behind it.
    pub fn backend(&self) -> StorageBackend {
        match self {
            Self::Sqlite(_) => StorageBackend::Sqlite,
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => StorageBackend::Postgres,
        }
    }

    /// The store as the target of a transfer (`docs/storage-migration.md`),
    /// or `None` for a backend that cannot be one yet: SQLite, until ALG-768
    /// brings the copy back from PostgreSQL.
    pub fn transfer_target(&self) -> Option<&dyn TransferTarget> {
        match self {
            Self::Sqlite(_) => None,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => Some(store),
        }
    }

    /// The same store, its file or pool shared, each call through it bounded
    /// by `budget`: waits, statements and all. A route hands it what its
    /// caller's deadline leaves, such as `StorageLimits::route_budget`, and
    /// a call that cannot finish in time fails as a [`StorageError`] of a
    /// kind the caller can act on, never as a success.
    pub fn within(&self, budget: Duration) -> Self {
        match self {
            Self::Sqlite(store) => Self::Sqlite(store.within(budget)),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => Self::Postgres(store.within(budget)),
        }
    }
}

/// The one place a call is handed to its backend: `delegate!(self, store
/// => expr)` evaluates `expr` with `store` bound to the backend's store.
macro_rules! delegate {
    ($any:expr, $store:ident => $call:expr) => {
        match $any {
            AnyStore::Sqlite($store) => $call,
            #[cfg(feature = "postgres")]
            AnyStore::Postgres($store) => $call,
        }
    };
}

impl CandleStore for AnyStore {
    fn init(&self) -> Result<(), StorageError> {
        delegate!(self, store => CandleStore::init(store))
    }

    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError> {
        delegate!(self, store => store.upsert_candles(market, candles))
    }

    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError> {
        delegate!(self, store => store.load_candles(market, query))
    }

    fn load_recent_candles(
        &self,
        market: &MarketId,
        limit: usize,
    ) -> Result<Vec<Candle>, StorageError> {
        delegate!(self, store => store.load_recent_candles(market, limit))
    }

    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        delegate!(self, store => store.max_open_time_ms(market))
    }

    fn market_coverage(&self) -> Result<Vec<MarketCoverage>, StorageError> {
        delegate!(self, store => store.market_coverage())
    }
}

impl SegmentStore for AnyStore {
    fn refresh_segments(&self, market: &MarketId) -> Result<SegmentRefresh, StorageError> {
        delegate!(self, store => store.refresh_segments(market))
    }

    fn resolve_segments(
        &self,
        market: &MarketId,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
    ) -> Result<SegmentPlan, StorageError> {
        delegate!(self, store => store.resolve_segments(market, first_open_ms, last_open_ms))
    }

    fn load_segment(&self, hash: &SegmentHash) -> Result<Option<Vec<u8>>, StorageError> {
        delegate!(self, store => store.load_segment(hash))
    }
}

impl RunJournalStore for AnyStore {
    fn init(&self) -> Result<(), StorageError> {
        delegate!(self, store => RunJournalStore::init(store))
    }

    fn create_run(&self, state: &BotRunState, identity: &RunIdentity) -> Result<(), StorageError> {
        delegate!(self, store => store.create_run(state, identity))
    }

    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError> {
        delegate!(self, store => store.save_run_state(state))
    }

    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError> {
        delegate!(self, store => store.load_run_state(run_id))
    }

    fn run_identity(&self, run_id: &str) -> Result<Option<RunIdentity>, StorageError> {
        delegate!(self, store => store.run_identity(run_id))
    }

    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError> {
        delegate!(self, store => store.latest_run_for_market(market, strategy_name))
    }

    fn append_order_event(&self, run_id: &str, order: &ExchangeOrder) -> Result<(), StorageError> {
        delegate!(self, store => store.append_order_event(run_id, order))
    }

    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError> {
        delegate!(self, store => store.append_closed_trade(run_id, trade))
    }

    fn list_order_events(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError> {
        delegate!(self, store => store.list_order_events(run_id, limit))
    }

    fn list_closed_trades(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError> {
        delegate!(self, store => store.list_closed_trades(run_id, limit))
    }
}

impl StoreControl for AnyStore {
    fn store_identity(&self) -> Result<StoreIdentity, StorageError> {
        delegate!(self, store => store.store_identity())
    }

    fn migrations(&self) -> Result<Vec<SchemaMigration>, StorageError> {
        delegate!(self, store => store.migrations())
    }

    fn bind_environment(&self, environment: Environment) -> Result<(), StorageError> {
        delegate!(self, store => store.bind_environment(environment))
    }

    fn legacy_runs(&self) -> Result<Vec<LegacyRun>, StorageError> {
        delegate!(self, store => store.legacy_runs())
    }

    fn adopt_run(
        &self,
        run_id: &str,
        identity: &RunIdentity,
        execution_mode: Option<ExecutionMode>,
    ) -> Result<(), StorageError> {
        delegate!(self, store => store.adopt_run(run_id, identity, execution_mode))
    }

    fn durability(&self) -> Result<DurabilityReport, StorageError> {
        delegate!(self, store => store.durability())
    }
}
