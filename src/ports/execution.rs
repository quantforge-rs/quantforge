//! The execution store port (ADR 0002 §15): a store's signal inbox, its
//! operations, its ledger of intents, orders, fills, position cycles,
//! reservations and incidents, its event outbox, and the leases that fence
//! each position. `docs/execution-store.md` is the contract,
//! `docs/storage-schema.md` the schema, and `docs/storage-conformance.md`
//! the behavior every backend passes.
//!
//! **One write.** Everything a caller changes at once is one
//! [`ExecutionUnit`], and [`ExecutionStore::commit`] applies it whole or
//! not at all: a delivery with its judgment, an intent with its order and
//! its reservation, a transition with the event that tells of it. The port
//! has no way to open a transaction and keep it, so none is ever open
//! while an exchange is asked anything.
//!
//! **Compare and swap.** A change names the revision of the record it
//! replaces, or that it expects none. What it finds instead is answered,
//! not raised: [`Commit::Stale`] names the record that moved, nothing was
//! written, and the caller reads again. A record is replaced only by a
//! successor its own rules allow, so no caller moves one backwards.
//!
//! **Fenced.** A unit that changes a position's records names the fence
//! of the lease it is written under. It is applied only while that lease
//! is still its writer's, on a store that is active at the epoch the lease
//! was granted under; otherwise the answer is [`Commit::Fenced`]. A store
//! that is not active grants no lease, and a copy of a store brings none.
//!
//! **The inbox decides what a delivery is.** A unit's delivery is the
//! first of its signal, the same one again, or another payload under its
//! ID, by the profile, the signal's ID and the hash of its payload alone.
//!
//! **One environment.** A store keeps the records of the environment it
//! is bound to, and of none before it is bound: a unit, an operation or a
//! lease of another is refused.
//!
//! A store never reads what a record means: the rules are the model's, in
//! [`crate::model::execution`]. Failures are [`StorageError`]s of the
//! kinds `docs/storage-durability.md` gives; an `Ok` is a durable commit.

use super::StorageError;
use crate::model::execution::{
    Commit, CycleRecord, EventKey, ExecutionScope, ExecutionUnit, Fence, FillRecord, Incident,
    IncidentKey, IntentRecord, Lease, LeaseAnswer, LeaseRequest, NewOperation, OperationRecord,
    OrderRecord, OutboxEvent, PoolReservations, Reservation, SignalDelivery, SignalRecord, Stored,
};
use crate::model::futures::{PoolScope, PositionCycleId, PositionKey, SignalId};
use crate::model::{AccountRef, InstrumentId, OperationId, TimestampMs, VenueTradeId};

pub trait ExecutionStore: Send + Sync {
    /// Applies `unit` whole, or not at all, and says which. Refuses a unit
    /// that is not one a store may apply ([`ExecutionUnit::check`]), and a
    /// change whose record could not follow the one it replaces.
    fn commit(&self, unit: &ExecutionUnit) -> Result<Commit, StorageError>;

    /// Numbers a new operation and records it, before anything is sent
    /// for it. An operation with a key already recorded for its account
    /// and product line is that operation, answered as it stands, so a
    /// command sent twice and a restart both find the one they began;
    /// a key that names another kind of operation, or one on another
    /// instrument, is refused.
    fn begin_operation(
        &self,
        operation: &NewOperation,
        at_ms: TimestampMs,
    ) -> Result<Stored<OperationRecord>, StorageError>;

    /// Asks for a position's lease. Refused on a store that is not
    /// active. Each new grant takes the position's next fence token.
    fn acquire_lease(&self, request: &LeaseRequest) -> Result<LeaseAnswer, StorageError>;

    /// Extends the lease `fence` names, or answers `None` when it is not
    /// its holder's any more, or the store is not active at its epoch.
    fn renew_lease(
        &self,
        fence: &Fence,
        ttl_ms: i64,
        now_ms: TimestampMs,
    ) -> Result<Option<Lease>, StorageError>;

    /// Gives the lease `fence` names back, and says whether it was still
    /// its holder's. The next grant takes the next token all the same.
    fn release_lease(&self, fence: &Fence) -> Result<bool, StorageError>;

    /// The lease on a position as the store holds it, expired or not.
    fn lease(&self, key: &PositionKey) -> Result<Option<Lease>, StorageError>;

    /// The signal a profile's inbox holds under an ID.
    fn signal(
        &self,
        scope: &ExecutionScope,
        signal: &SignalId,
    ) -> Result<Option<Stored<SignalRecord>>, StorageError>;

    /// The signals recorded and not yet judged, oldest first: what a
    /// process that starts judges before it takes new ones.
    fn received_signals(&self, limit: usize) -> Result<Vec<Stored<SignalRecord>>, StorageError>;

    /// Every delivery of a signal, in the order they arrived.
    fn deliveries(
        &self,
        scope: &ExecutionScope,
        signal: &SignalId,
    ) -> Result<Vec<SignalDelivery>, StorageError>;

    /// The intent of an admitted signal.
    fn intent(
        &self,
        scope: &ExecutionScope,
        signal: &SignalId,
    ) -> Result<Option<Stored<IntentRecord>>, StorageError>;

    /// The intents on a position that are not settled yet, of any profile,
    /// in the order they were admitted.
    fn position_intents(
        &self,
        key: &PositionKey,
    ) -> Result<Vec<Stored<IntentRecord>>, StorageError>;

    /// Every intent in the store that is not settled yet, in the order
    /// they were admitted: what a process that starts resumes.
    fn unsettled_intents(&self) -> Result<Vec<Stored<IntentRecord>>, StorageError>;

    fn operation(&self, id: OperationId) -> Result<Option<Stored<OperationRecord>>, StorageError>;

    /// Every replacement still pending or of unknown outcome, by its
    /// operation's number.
    fn unsettled_replacements(&self) -> Result<Vec<Stored<OperationRecord>>, StorageError>;

    /// The order recorded under a client order ID.
    fn order(
        &self,
        scope: &ExecutionScope,
        client_order_id: &str,
    ) -> Result<Option<Stored<OrderRecord>>, StorageError>;

    /// An operation's orders, in the order they were recorded.
    fn orders(&self, operation: OperationId) -> Result<Vec<Stored<OrderRecord>>, StorageError>;

    /// The fill recorded under a venue's trade ID.
    fn fill(
        &self,
        scope: &ExecutionScope,
        instrument: &InstrumentId,
        trade: &VenueTradeId,
    ) -> Result<Option<FillRecord>, StorageError>;

    /// A cycle's fills, in the order they were recorded.
    fn fills(&self, cycle: PositionCycleId) -> Result<Vec<FillRecord>, StorageError>;

    fn cycle(&self, id: PositionCycleId) -> Result<Option<Stored<CycleRecord>>, StorageError>;

    /// The open cycle on a position: at most one.
    fn open_cycle(&self, key: &PositionKey) -> Result<Option<Stored<CycleRecord>>, StorageError>;

    /// Every open cycle in the store, by its number.
    fn open_cycles(&self) -> Result<Vec<Stored<CycleRecord>>, StorageError>;

    /// What is held against one pool of one account, with the pool's
    /// revision, read in one snapshot.
    fn pool(
        &self,
        account: &AccountRef,
        pool: &PoolScope,
    ) -> Result<PoolReservations, StorageError>;

    /// The reservation made for an entry's operation.
    fn reservation(
        &self,
        operation: OperationId,
    ) -> Result<Option<Stored<Reservation>>, StorageError>;

    fn incident(
        &self,
        scope: &ExecutionScope,
        key: &IncidentKey,
    ) -> Result<Option<Stored<Incident>>, StorageError>;

    /// Every incident not cleared yet, in the order they were raised.
    fn open_incidents(&self) -> Result<Vec<Stored<Incident>>, StorageError>;

    fn event(
        &self,
        scope: &ExecutionScope,
        key: &EventKey,
    ) -> Result<Option<Stored<OutboxEvent>>, StorageError>;

    /// The events not delivered yet, in the order they were created.
    fn pending_events(&self, limit: usize) -> Result<Vec<Stored<OutboxEvent>>, StorageError>;
}
