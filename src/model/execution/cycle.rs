//! A position cycle as the ledger keeps it.

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::ModelError;
use crate::model::futures::{PositionCycle, PositionCycleId, PositionKey, ProtectionState};
use serde::{Deserialize, Serialize};

/// The version of [`CycleRecord`]'s stored form.
pub const CYCLE_RECORD_FORMAT: u32 = 1;

const RECORD: &str = "cycle record";

/// One owned position cycle: from flat, through an entry, back to flat,
/// with its protection, why it closed and what it made. A position has at
/// most one open cycle, and a closed one never opens again.
///
/// A cycle takes the number of the operation whose entry opened it, so it
/// is numbered by its store and names its own origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CycleRecordParts")]
pub struct CycleRecord {
    format: u32,
    scope: ExecutionScope,
    cycle: PositionCycle,
}

impl CycleRecord {
    /// The record of `cycle`, in its own profile's scope. Refuses a cycle
    /// numbered otherwise than by the operation that opened it.
    pub fn new(cycle: PositionCycle) -> Result<Self, ModelError> {
        Self::try_from(CycleRecordParts {
            format: CYCLE_RECORD_FORMAT,
            scope: ExecutionScope::of_position(cycle.key(), cycle.profile()),
            cycle,
        })
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub fn cycle(&self) -> &PositionCycle {
        &self.cycle
    }

    pub const fn id(&self) -> PositionCycleId {
        self.cycle.id()
    }

    pub fn key(&self) -> &PositionKey {
        self.cycle.key()
    }

    /// Whether the position is still held, protected or not.
    pub fn is_open(&self) -> bool {
        self.cycle.protection() != ProtectionState::Closed
    }

    /// The record with its cycle moved on.
    pub fn with_cycle(&self, cycle: PositionCycle) -> Result<Self, ModelError> {
        let next = Self::new(cycle)?;
        next.follows(self)?;
        Ok(next)
    }

    /// Whether `self` may replace `previous`: the same cycle, its
    /// protection moved along its state machine or not at all.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let (old, new) = (&previous.cycle, &self.cycle);
        let same = new.id() == old.id()
            && new.key() == old.key()
            && new.profile() == old.profile()
            && new.side() == old.side()
            && new.opened_by() == old.opened_by()
            && new.opened_at_ms() == old.opened_at_ms();
        if !same {
            return Err(not_the_same(RECORD, "cycle"));
        }
        let (from, to) = (old.protection(), new.protection());
        if from != to && !from.can_move_to(to) {
            return Err(ModelError::InvalidTransition {
                machine: "position protection",
                from: from.as_str(),
                to: to.as_str(),
            });
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, CYCLE_RECORD_FORMAT, text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CycleRecordParts {
    format: u32,
    scope: ExecutionScope,
    cycle: PositionCycle,
}

impl TryFrom<CycleRecordParts> for CycleRecord {
    type Error = ModelError;

    fn try_from(parts: CycleRecordParts) -> Result<Self, Self::Error> {
        if parts.format != CYCLE_RECORD_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: CYCLE_RECORD_FORMAT,
            });
        }
        let cycle = &parts.cycle;
        if parts.scope != ExecutionScope::of_position(cycle.key(), cycle.profile()) {
            return Err(disagrees(
                RECORD,
                "its scope is not its own profile's on its own position",
            ));
        }
        if cycle.id().get() != cycle.opened_by().get() {
            return Err(disagrees(
                RECORD,
                format!(
                    "cycle {} was opened by operation {}, and a cycle takes the number of the \
                     operation that opened it",
                    cycle.id().get(),
                    cycle.opened_by()
                ),
            ));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            cycle: parts.cycle,
        })
    }
}
