//! The records an execution store keeps (ADR 0002 §15): the signal inbox,
//! operations, intents, orders, fills, position cycles, reservations,
//! incidents and the event outbox, with the lease that fences a position
//! and the one unit every write is made in. `docs/execution-store.md` is
//! the contract, and [`crate::ports::ExecutionStore`] the port that keeps
//! them.
//!
//! Three identities stay apart, and none stands in for another:
//! - **a signal** is known by its profile and its ID, with the hash of its
//!   payload ([`SignalRecord`]). That alone decides whether a delivery is
//!   new, a duplicate or a conflict, whatever became of any position since;
//! - **an operation** is one action that changes something on an exchange
//!   ([`OperationRecord`]), numbered by its store. It may come from a
//!   signal or from a command, and needs neither a signal nor a position;
//! - **a position cycle** is one owned position from flat back to flat
//!   ([`CycleRecord`]). An intent and an operation name the cycle they
//!   belong to beside their own identity, never as part of it.
//!
//! Every record carries its [`ExecutionScope`]: the venue, the product
//! family, the environment and the account, with the profile where one
//! acts. The same signal ID, the same symbol and the same client order ID
//! under two scopes are two records that never meet.
//!
//! Each record is stored as one JSON document that names its format, with
//! every decimal as exact text; a document of another format is refused,
//! never guessed at. A record changes only into a successor its own rules
//! allow ([`Record::follows`]), so a store that compares and swaps on a
//! record's revision never moves one backwards.

mod commit;
mod cycle;
mod fill;
mod incident;
mod intent;
mod lease;
mod operation;
mod order;
mod outbox;
mod record;
mod replacement;
mod reservation;
mod scope;
mod signal;
mod unit;

pub use commit::{Commit, Fenced, Found, Stale};
pub use cycle::{CYCLE_RECORD_FORMAT, CycleRecord};
pub use fill::{FILL_RECORD_FORMAT, FillRecord};
pub use incident::{
    Clearance, Halt, INCIDENT_FORMAT, Incident, IncidentKey, IncidentKind, IncidentState,
};
pub use intent::{INTENT_RECORD_FORMAT, IntentRecord};
pub use lease::{Fence, FenceToken, Lease, LeaseAnswer, LeaseHolder, LeaseRequest};
pub use operation::{NewOperation, OPERATION_FORMAT, OperationKey, OperationKind, OperationRecord};
pub use order::{ORDER_RECORD_FORMAT, OrderEvidence, OrderPhase, OrderRecord, OrderRefusal};
pub use outbox::{DeliveryState, EVENT_FORMAT, EventKey, EventKind, OutboxEvent};
pub use record::{Expect, Record, RecordKind, Stored};
pub use replacement::{OrderRef, Replacement, SpotReplaceState};
pub use reservation::{
    PoolReservations, RESERVATION_FORMAT, Release, ReleaseReason, Reservation, ReservationState,
};
pub use scope::ExecutionScope;
pub use signal::{SIGNAL_RECORD_FORMAT, SignalDelivery, SignalRecord, SignalState};
pub use unit::{Change, DeliveryWrite, ExecutionUnit, PoolGuard};

use super::ModelError;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A record as a store keeps it: JSON in one fixed spelling.
fn to_stored<T: Serialize>(record: &'static str, value: &T) -> Result<String, ModelError> {
    serde_json::to_string(value).map_err(|error| ModelError::InvalidRecord {
        record,
        reason: error.to_string(),
    })
}

/// Reads a stored record back. Its format is read first, so that a record
/// of another format is refused as that, whatever else it holds.
fn from_stored<T: DeserializeOwned>(
    record: &'static str,
    expected: u32,
    text: &str,
) -> Result<T, ModelError> {
    #[derive(serde::Deserialize)]
    struct Format {
        format: u32,
    }
    let invalid = |error: serde_json::Error| {
        // A record's own check already says which record it refuses.
        let said = error.to_string();
        let own = format!("invalid {record}: ");
        ModelError::InvalidRecord {
            record,
            reason: said.strip_prefix(&own).unwrap_or(&said).to_string(),
        }
    };
    let Format { format } = serde_json::from_str(text).map_err(invalid)?;
    if format != expected {
        return Err(ModelError::UnsupportedFormat {
            record,
            found: format,
            expected,
        });
    }
    serde_json::from_str(text).map_err(invalid)
}

/// The refusal of a record whose parts do not agree with each other.
fn disagrees(record: &'static str, reason: impl Into<String>) -> ModelError {
    ModelError::InvalidRecord {
        record,
        reason: reason.into(),
    }
}

/// The refusal of a successor that is not the record it follows.
fn not_the_same(record: &'static str, what: &str) -> ModelError {
    disagrees(
        record,
        format!("its {what} differs from the record it follows, and that never changes"),
    )
}
