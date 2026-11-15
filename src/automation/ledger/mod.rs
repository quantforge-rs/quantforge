//! The execution ledger: what admission and an executor keep a signal's
//! progress through, over [`ExecutionStore`] (`docs/execution-store.md`).
//!
//! - **Admission** ([`ExecutionLedger::admit`]) records a delivery with
//!   what became of it in one write: refused, with nothing to do, or
//!   admitted with its intent and the policy it was admitted under. The
//!   same delivery again changes nothing, and another payload under its
//!   signal's ID is refused as a conflict, for good.
//! - **An order's identity comes first**
//!   ([`ExecutionLedger::record_attempt`]): the client order ID an order
//!   will carry is recorded, under the position's lease, before the order
//!   is sent. A process that dies after that finds the attempt, and knows
//!   what to ask the venue about.
//! - **Evidence settles** ([`ExecutionLedger::settle`]): an order or a
//!   refusal read back from the venue moves the intent, records the fills,
//!   opens the position's cycle at an entry's first fill and releases its
//!   reservation, all in one write.
//! - **A restart asks before it sends** ([`ExecutionLedger::resume`]): an
//!   intent found with an attempt recorded and no outcome is settled by a
//!   query for that attempt's client order ID. This never sends an order;
//!   an entry is never sent twice, whatever the answer.
//!
//! Every write is one unit, so each of these either happened whole or did
//! not happen, and none holds a transaction while a venue is asked. A
//! write that finds its records changed by another writer, or its lease
//! gone, is told so ([`LedgerError::Stale`], [`LedgerError::Fenced`]) and
//! has changed nothing.

mod admission;
mod orders;
mod resume;

use std::fmt;

use thiserror::Error;

use crate::model::ModelError;
use crate::model::execution::{Commit, ExecutionUnit, Fenced, SignalRecord, Stale};
use crate::model::futures::Delivery;
use crate::ports::{Clock, ExchangeError, ExecutionStore, StorageError};

pub use admission::Admission;
pub use orders::Evidence;
pub use resume::Resumed;

/// Why the ledger did not do what it was asked.
#[derive(Debug, Error)]
pub enum LedgerError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Model(#[from] ModelError),

    /// The venue could not be asked. Nothing was decided.
    #[error(transparent)]
    Exchange(#[from] ExchangeError),

    /// Another writer changed a record this write was made from. Nothing
    /// was written: read again, and decide again.
    #[error("another writer got there first: {0}; nothing was written")]
    Stale(Stale),

    /// The position's lease is not this writer's any more, or the store
    /// may not execute. Nothing was written, and nothing may be sent.
    #[error("this writer may not act on the position: {0}; nothing was written")]
    Fenced(Fenced),

    /// A delivery kept meeting other deliveries of its signal.
    #[error("the signal kept changing while its delivery was recorded; deliver it again")]
    Busy,
}

/// The ledger over one store.
pub struct ExecutionLedger<'a> {
    store: &'a dyn ExecutionStore,
    clock: &'a dyn Clock,
}

impl fmt::Debug for ExecutionLedger<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionLedger").finish_non_exhaustive()
    }
}

/// What a delivery was to an inbox that already held its signal, with the
/// signal as recorded.
type Again = (Delivery, crate::model::execution::Stored<SignalRecord>);

impl<'a> ExecutionLedger<'a> {
    pub fn new(store: &'a dyn ExecutionStore, clock: &'a dyn Clock) -> Self {
        Self { store, clock }
    }

    /// Applies a unit that must apply whole: anything else is its
    /// writer's to know.
    fn apply(&self, unit: &ExecutionUnit) -> Result<(), LedgerError> {
        match self.store.commit(unit)? {
            Commit::Applied => Ok(()),
            Commit::Stale(stale) => Err(LedgerError::Stale(stale)),
            Commit::Fenced(fenced) => Err(LedgerError::Fenced(fenced)),
            Commit::Duplicate(_) | Commit::Conflict(_) => Err(LedgerError::Busy),
        }
    }

    /// Applies a unit that carries a delivery: `None` once it is applied,
    /// or what the delivery was to an inbox that already held its signal.
    fn deliver(&self, unit: &ExecutionUnit) -> Result<Option<Again>, LedgerError> {
        match self.store.commit(unit)? {
            Commit::Applied => Ok(None),
            Commit::Duplicate(held) => Ok(Some((Delivery::Duplicate, held))),
            Commit::Conflict(held) => Ok(Some((Delivery::Conflict, held))),
            Commit::Stale(stale) => Err(LedgerError::Stale(stale)),
            Commit::Fenced(fenced) => Err(LedgerError::Fenced(fenced)),
        }
    }
}
