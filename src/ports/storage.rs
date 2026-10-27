//! The storage ports: what a persistence backend implements, with the one
//! failure channel they report through. SQLite implements them today;
//! PostgreSQL follows (`docs/storage-schema.md` is their shared contract).

use std::error::Error as StdError;
use thiserror::Error;

use crate::model::{
    BotRunState, Candle, ClosedTrade, Environment, ExchangeOrder, ExecutionMode, LegacyRun,
    MarketId, RunIdentity, SchemaMigration, SegmentHash, SegmentPlan, SegmentRefresh,
    StoreIdentity, TimestampMs,
};

#[derive(Clone, Debug, Default)]
pub struct CandleQuery {
    pub start_time_ms: Option<TimestampMs>,
    pub end_time_ms: Option<TimestampMs>,
    pub limit: Option<usize>,
}

/// A storage backend's failures. Older schemas are never refused: `init`
/// migrates them (`docs/storage-schema.md`); only a schema newer than the
/// build is.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum StorageError {
    #[error("storage error")]
    Other(#[source] Box<dyn StdError + Send + Sync>),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error(
        "database schema version {found} is newer than this build supports ({supported}); use a \
         newer quantforge release, and never an older one on this database"
    )]
    SchemaUnsupported { found: u32, supported: u32 },
}

impl StorageError {
    pub fn other<E>(err: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::Other(Box::new(err))
    }
}

pub trait CandleStore: Send + Sync {
    fn init(&self) -> Result<(), StorageError>;
    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError>;
    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError>;
    fn load_recent_candles(
        &self,
        market: &MarketId,
        limit: usize,
    ) -> Result<Vec<Candle>, StorageError>;
    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError>;
}

/// A market's closed candles packed window by window into immutable
/// segments, each named by the hash of its payload, beside the candles they
/// are built from. [`crate::storage::segments`] is the format.
pub trait SegmentStore: Send + Sync {
    /// Packs every closed window whose candles changed since it was last
    /// packed. The window holding the newest candle is the tip, and stays
    /// unpacked.
    fn refresh_segments(&self, market: &MarketId) -> Result<SegmentRefresh, StorageError>;
    /// The segments, and the tip after them, holding `market`'s candles with
    /// open times from `first_open_ms` to `last_open_ms`.
    fn resolve_segments(
        &self,
        market: &MarketId,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
    ) -> Result<SegmentPlan, StorageError>;
    /// A segment's stored bytes, its payload zstd-compressed, or `None` when
    /// no segment has that name.
    fn load_segment(&self, hash: &SegmentHash) -> Result<Option<Vec<u8>>, StorageError>;
}

/// Bot runs and their journals. A run is created once, with the identity
/// of the process that starts it, and only updated after that: the
/// identity never changes, and an update of a run that was never created
/// is refused. A run a build before schema 5 wrote has a legacy identity
/// and loads, but is driven again only once an operator adopts it
/// ([`StoreControl::adopt_run`]).
pub trait RunJournalStore: Send + Sync {
    fn init(&self) -> Result<(), StorageError>;
    /// Records a new run with its identity. Refuses a `run_id` already
    /// recorded, and a legacy identity: only a migration marks runs legacy.
    fn create_run(&self, state: &BotRunState, identity: &RunIdentity) -> Result<(), StorageError>;
    /// Saves the state of a run created before. Refuses an unknown run.
    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError>;
    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError>;
    /// The identity a run was created or adopted with.
    fn run_identity(&self, run_id: &str) -> Result<Option<RunIdentity>, StorageError>;
    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError>;
    fn append_order_event(&self, run_id: &str, order: &ExchangeOrder) -> Result<(), StorageError>;
    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError>;
    fn list_order_events(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError>;
    fn list_closed_trades(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError>;
}

/// The store's own record: its identity, the environment it is bound to,
/// its activation, the schema chain it went through, and the legacy runs
/// waiting for an operator. `docs/storage-schema.md` is the contract.
/// Activation moves (`StoreActivation::move_to`) are not persisted here:
/// ALG-738 adds them, ALG-767 enforces them before arming.
pub trait StoreControl: Send + Sync {
    fn store_identity(&self) -> Result<StoreIdentity, StorageError>;
    /// Every step of the schema chain applied to this store, oldest first.
    fn migrations(&self) -> Result<Vec<SchemaMigration>, StorageError>;
    /// Binds the store to `environment` for life. Refuses a store bound to
    /// another one; binding again to the same one changes nothing.
    fn bind_environment(&self, environment: Environment) -> Result<(), StorageError>;
    /// The runs written before schema 5, oldest first.
    fn legacy_runs(&self) -> Result<Vec<LegacyRun>, StorageError>;
    /// Gives a legacy run the identity an operator verified. A run that
    /// recorded no execution mode (QuantForge 0.2.0) takes the one the
    /// operator states; a run that recorded one refuses a stated one. The
    /// run's original `state_json` stays in its `legacy_state_json`.
    fn adopt_run(
        &self,
        run_id: &str,
        identity: &RunIdentity,
        execution_mode: Option<ExecutionMode>,
    ) -> Result<(), StorageError>;
}
