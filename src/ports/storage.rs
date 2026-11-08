//! The storage ports: what a persistence backend implements, with the one
//! failure channel they report through. SQLite implements them today;
//! PostgreSQL follows. `docs/storage-schema.md` is their shared schema, and
//! `docs/storage-conformance.md` the behavior every backend passes.

use std::error::Error as StdError;
use std::fmt;
use thiserror::Error;

use crate::model::{
    BotRunState, Candle, ClosedTrade, DurabilityReport, Environment, ExchangeOrder, ExecutionMode,
    LegacyRun, MarketCoverage, MarketId, RunIdentity, SchemaMigration, SegmentHash, SegmentPlan,
    SegmentRefresh, StoreIdentity, TimestampMs,
};

#[derive(Clone, Debug, Default)]
pub struct CandleQuery {
    pub start_time_ms: Option<TimestampMs>,
    pub end_time_ms: Option<TimestampMs>,
    pub limit: Option<usize>,
}

/// A storage backend's failures. Older schemas are never refused: `init`
/// migrates them (`docs/storage-schema.md`); only a schema newer than the
/// build is. [`StorageError::kind`] tells callers what to do about one, the
/// same on every backend: `docs/storage-durability.md` maps each backend's
/// failures onto the kinds.
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

    /// Nothing was changed: no connection, no free one in time, or a read
    /// that ran out of time. Safe to retry.
    #[error("storage unavailable: {0}")]
    Unavailable(String),

    /// Nothing was changed: another writer held what this call needed past
    /// its wait budget. Safe to retry once the writer is done.
    #[error("storage busy: {0}")]
    Contention(String),

    /// Nothing was changed: the backend rolled the transaction back to
    /// settle a deadlock or a serialization conflict. Safe to run the whole
    /// transaction again, and only the transaction.
    #[error("storage conflict: {0}")]
    Conflict(String),

    /// A write lost its answer and may or may not have committed: look
    /// before retrying, since a blind retry can write a row twice.
    #[error("storage write outcome unknown: {0}; check what was written before retrying")]
    OutcomeUnknown(String),

    /// The store's effective settings are weaker than the durability
    /// contract, so an acknowledgement would promise more than it can keep.
    #[error("storage cannot keep its commits: {0}")]
    DurabilityUnmet(String),
}

impl StorageError {
    pub fn other<E>(err: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::Other(Box::new(err))
    }

    /// What a caller does about this failure.
    pub fn kind(&self) -> StorageErrorKind {
        match self {
            Self::Other(_) => StorageErrorKind::Other,
            Self::InvalidArgument(_) => StorageErrorKind::InvalidArgument,
            Self::SchemaUnsupported { .. } => StorageErrorKind::SchemaUnsupported,
            Self::Unavailable(_) => StorageErrorKind::Unavailable,
            Self::Contention(_) => StorageErrorKind::Contention,
            Self::Conflict(_) => StorageErrorKind::Conflict,
            Self::OutcomeUnknown(_) => StorageErrorKind::OutcomeUnknown,
            Self::DurabilityUnmet(_) => StorageErrorKind::DurabilityUnmet,
        }
    }
}

/// What a storage failure asks of its caller, with the same meaning on
/// every backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageErrorKind {
    /// Nothing was changed, and the store could not be reached in time.
    /// Retry.
    Unavailable,
    /// Nothing was changed, and another writer held a lock past the budget.
    /// Retry.
    Contention,
    /// Nothing was changed, and the transaction lost to another. Run the
    /// whole transaction again.
    Conflict,
    /// A write may or may not have committed. Find out before anything
    /// else, and never report it as done.
    OutcomeUnknown,
    /// The store's settings are weaker than the contract. Fix them.
    DurabilityUnmet,
    /// The call was refused: fix the argument.
    InvalidArgument,
    /// The database is from a newer build.
    SchemaUnsupported,
    /// A failure with no category: the database refused a statement, or
    /// its contents could not be read.
    Other,
}

impl StorageErrorKind {
    /// The kind's name as reports print it, such as `outcome_unknown`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Contention => "contention",
            Self::Conflict => "conflict",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::DurabilityUnmet => "durability_unmet",
            Self::InvalidArgument => "invalid_argument",
            Self::SchemaUnsupported => "schema_unsupported",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for StorageErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
    /// Every market the store holds candles of, with the open times of its
    /// first and last candle, in no particular order, read in one snapshot.
    /// A market stored under a name this build cannot read back, such as a
    /// product line a later build added, is left out and logged: it is never
    /// read as some other market.
    fn market_coverage(&self) -> Result<Vec<MarketCoverage>, StorageError>;
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
    /// The run of `market` and `strategy_name` updated last. Of runs
    /// updated in the same millisecond, the one started last; of those, the
    /// one with the greatest `run_id`.
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
    /// The settings this store's commits rely on, read back from a
    /// connection (`docs/storage-durability.md`).
    fn durability(&self) -> Result<DurabilityReport, StorageError>;
}
