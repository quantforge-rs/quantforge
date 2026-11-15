//! What becomes of a unit: applied, or not, and why not.

use super::{FenceToken, LeaseHolder, RecordKind, SignalRecord, Stored};
use crate::model::StoreActivation;
use crate::model::futures::ActivationEpoch;
use std::fmt;

/// What a store found where a change expected something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Found {
    /// No record, where one was to be replaced.
    Absent,
    /// A record at this revision, where none or another was expected.
    Present { revision: u64 },
}

/// A unit that was not applied because what it expected no longer holds:
/// another writer changed a record first. Nothing was written; its writer
/// reads again and decides again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stale {
    /// A record was not as its change expected.
    Record {
        kind: RecordKind,
        name: String,
        found: Found,
    },
    /// The pool has been reserved from, or released to, since it was read.
    Pool { revision: u64 },
}

impl fmt::Display for Stale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record {
                kind,
                name,
                found: Found::Absent,
            } => write!(f, "the store holds no {kind} {name}"),
            Self::Record {
                kind,
                name,
                found: Found::Present { revision },
            } => write!(f, "{kind} {name} stands at revision {revision}"),
            Self::Pool { revision } => write!(f, "the pool stands at revision {revision}"),
        }
    }
}

/// A unit that was not applied because the lease it names is not its
/// writer's any more, or the store may not execute. Nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fenced {
    /// Nobody holds the position's lease.
    NotHeld,
    /// The lease is another holder's, or was granted again since.
    Lost {
        holder: LeaseHolder,
        token: FenceToken,
    },
    /// The store is not active at the epoch the lease was granted under.
    StoreNotActive {
        activation: StoreActivation,
        epoch: ActivationEpoch,
    },
}

impl fmt::Display for Fenced {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHeld => f.write_str("nobody holds the position's lease"),
            Self::Lost { holder, token } => {
                write!(f, "the position's lease is {holder}'s, under token {token}")
            }
            Self::StoreNotActive { activation, epoch } => write!(
                f,
                "the store is {activation} at epoch {}, and only a store active at the epoch a \
                 lease was granted under executes",
                epoch.0
            ),
        }
    }
}

/// What became of a unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Commit {
    /// Every change was written, with the delivery if there was one.
    Applied,
    /// The unit's delivery is one the inbox already holds: it was noted,
    /// and nothing else was written. The record is the signal's own.
    Duplicate(Stored<SignalRecord>),
    /// The unit's delivery brought another payload under a recorded
    /// signal's ID: it was noted with its conflict event, and nothing else
    /// was written. The record is the signal's own, unchanged.
    Conflict(Stored<SignalRecord>),
    /// Nothing was written: see [`Stale`].
    Stale(Stale),
    /// Nothing was written: see [`Fenced`].
    Fenced(Fenced),
}

impl Commit {
    pub const fn is_applied(&self) -> bool {
        matches!(self, Self::Applied)
    }
}
