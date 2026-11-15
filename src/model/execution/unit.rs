//! The unit: everything one write to an execution store changes, applied
//! whole or not at all. A unit is one call and one transaction, so no
//! transaction is ever open while an exchange is asked anything.

use super::{
    EventKind, Expect, Fence, OutboxEvent, Record, RecordKind, SignalRecord, SignalState, disagrees,
};
use crate::model::futures::{IntentState, PoolScope, PositionKey};
use crate::model::{ModelError, TimestampMs};

/// One change of a unit: a record, and what it expects to find.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub record: Record,
    pub expect: Expect,
}

impl Change {
    /// The first write of `record`.
    pub fn new(record: Record) -> Self {
        Self {
            record,
            expect: Expect::Absent,
        }
    }

    /// `record` in place of the one a store holds at `revision`.
    pub fn replace(record: Record, revision: u64) -> Self {
        Self {
            record,
            expect: Expect::Revision(revision),
        }
    }
}

/// A delivery a unit records in its profile's inbox. The store decides
/// what it is: the first of its signal, which the rest of the unit is
/// then applied with; the same one again, which changes nothing; or
/// another payload under the signal's ID, which is noted with
/// `conflict_event` and changes nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryWrite {
    /// The signal as this delivery brought it: received, or already
    /// judged where its admission is decided in the same unit.
    pub record: SignalRecord,
    /// What the outbox is told if the delivery conflicts.
    pub conflict_event: OutboxEvent,
}

/// The revision a unit that reserves or releases margin read its pool at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PoolGuard {
    pub revision: u64,
}

/// Everything one write changes. See the module docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionUnit {
    /// When the unit is written, by its writer's clock.
    pub at_ms: TimestampMs,
    /// The lease the unit is written under, where it changes a position.
    pub fence: Option<Fence>,
    /// The pool revision its reservations were sized against.
    pub pool: Option<PoolGuard>,
    pub delivery: Option<DeliveryWrite>,
    pub changes: Vec<Change>,
}

impl ExecutionUnit {
    /// A unit with nothing in it yet.
    pub fn at(at_ms: TimestampMs) -> Self {
        Self {
            at_ms,
            fence: None,
            pool: None,
            delivery: None,
            changes: Vec::new(),
        }
    }

    /// The unit written under `fence`.
    pub fn under(mut self, fence: Fence) -> Self {
        self.fence = Some(fence);
        self
    }

    /// The unit with one more change.
    pub fn with(mut self, change: Change) -> Self {
        self.changes.push(change);
        self
    }

    /// The position the unit changes, if it changes one: the position of
    /// every record that is part of one, which must be one and the same.
    pub fn position(&self) -> Result<Option<PositionKey>, ModelError> {
        let mut found: Option<PositionKey> = None;
        for change in &self.changes {
            let part_of = match (&change.record, change.expect) {
                // An intent is first recorded by admission, which holds no
                // lease: nothing can be sent for it until it is changed.
                (Record::Intent(_), Expect::Absent) => None,
                (record, _) => record.position()?,
            };
            match (&found, part_of) {
                (Some(held), Some(key)) if *held != key => {
                    return Err(refused(format!(
                        "it changes two positions, {} and {}; a unit changes one",
                        held.instrument(),
                        key.instrument()
                    )));
                }
                (None, Some(key)) => found = Some(key),
                _ => {}
            }
        }
        Ok(found)
    }

    /// The account and pool the unit reserves from or releases to, if it
    /// does either.
    pub fn reserved_pool(&self) -> Result<Option<(PositionKey, PoolScope)>, ModelError> {
        let mut found: Option<(PositionKey, PoolScope)> = None;
        for change in &self.changes {
            let Record::Reservation(held) = &change.record else {
                continue;
            };
            let key = held.scope().position_key(held.instrument())?;
            let same = found
                .as_ref()
                .is_none_or(|(of, pool)| of.account() == key.account() && pool == held.pool());
            if !same {
                return Err(refused(
                    "it reserves from two pools; a unit changes the reservations of one"
                        .to_string(),
                ));
            }
            found = Some((key, held.pool().clone()));
        }
        Ok(found)
    }

    /// Whether the unit is one a store may be asked to apply. This holds
    /// what no revision can: that a position's records change only under
    /// its lease, that an intent is recorded with the admission of its
    /// own signal and never apart from it, that each kind of record is
    /// first written the way its kind is, and that a unit gives its
    /// position one unresolved entry and one open cycle at most.
    pub fn check(&self) -> Result<(), ModelError> {
        if self.delivery.is_none() && self.changes.is_empty() {
            return Err(refused("it changes nothing".to_string()));
        }
        let mut seen: Vec<(RecordKind, String)> = Vec::new();
        for change in &self.changes {
            let record = &change.record;
            let allowed = match (record.kind(), change.expect) {
                // A signal is first recorded by a delivery, and an
                // operation is numbered by its store.
                (RecordKind::Signal | RecordKind::Operation, Expect::Absent) => false,
                (RecordKind::Fill, Expect::Revision(_)) => false,
                _ => true,
            };
            if !allowed {
                return Err(refused(format!(
                    "{} {} cannot be {} by a change",
                    record.kind(),
                    record.name(),
                    match change.expect {
                        Expect::Absent => "first recorded",
                        Expect::Revision(_) => "replaced",
                    }
                )));
            }
            let named = (record.kind(), record.name());
            if seen.contains(&named) {
                return Err(refused(format!("it changes {} {} twice", named.0, named.1)));
            }
            seen.push(named);
        }
        self.check_fence()?;
        self.check_pool()?;
        self.check_admissions()?;
        self.check_slots()
    }

    fn check_fence(&self) -> Result<(), ModelError> {
        let Some(position) = self.position()? else {
            return Ok(());
        };
        match &self.fence {
            Some(fence) if fence.key == position => Ok(()),
            Some(fence) => Err(refused(format!(
                "it changes {}, and is written under the lease of {}",
                position.instrument(),
                fence.key.instrument()
            ))),
            None => Err(refused(format!(
                "it changes the position on {} without that position's lease",
                position.instrument()
            ))),
        }
    }

    fn check_pool(&self) -> Result<(), ModelError> {
        match (self.reserved_pool()?, self.pool) {
            (Some(_), None) => Err(refused(
                "it changes a reservation without naming the revision its pool was read at"
                    .to_string(),
            )),
            (None, Some(_)) => Err(refused(
                "it names a pool's revision and changes no reservation".to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// An admitted signal and its intent are written together.
    fn check_admissions(&self) -> Result<(), ModelError> {
        if let Some(delivery) = &self.delivery {
            let event = &delivery.conflict_event;
            if event.kind() != EventKind::SignalConflict || event.scope() != delivery.record.scope()
            {
                return Err(refused(
                    "its delivery's conflict event is not a signal conflict of the same profile"
                        .to_string(),
                ));
            }
        }
        let delivered = self.delivery.iter().map(|delivery| &delivery.record);
        let judged = self
            .changes
            .iter()
            .filter_map(|change| match &change.record {
                Record::Signal(signal) => Some(signal),
                _ => None,
            });
        let signals: Vec<&SignalRecord> = delivered.chain(judged).collect();
        // A signal is its scope's: two accounts' profiles of one name that
        // take one alert hold two signals, and each is paired with its own
        // intent.
        let known_by = |signal: &SignalRecord| (signal.scope().clone(), signal.id().clone());
        let name = |signal: &SignalRecord| format!("{}/{}", signal.profile(), signal.id());
        let mut seen = Vec::new();
        for signal in &signals {
            if seen.contains(&known_by(signal)) {
                return Err(refused(format!("it records signal {} twice", name(signal))));
            }
            seen.push(known_by(signal));
        }
        let admitted: Vec<&SignalRecord> = signals
            .iter()
            .copied()
            .filter(|signal| signal.state() == SignalState::Admitted)
            .collect();
        let mut recorded = Vec::new();
        for change in &self.changes {
            let (Record::Intent(intent), Expect::Absent) = (&change.record, change.expect) else {
                continue;
            };
            let of = (intent.scope().clone(), intent.signal().clone());
            let admits = admitted.iter().any(|signal| known_by(signal) == of);
            if intent.intent().state() != IntentState::Admitted || !admits {
                return Err(refused(format!(
                    "intent {} is first recorded as admitted, in the unit that admits its signal",
                    change.record.name()
                )));
            }
            recorded.push(of);
        }
        match admitted
            .iter()
            .find(|signal| !recorded.contains(&known_by(signal)))
        {
            Some(signal) => Err(refused(format!(
                "it admits signal {} without recording its intent",
                name(signal)
            ))),
            None => Ok(()),
        }
    }

    /// A position has one unresolved entry and one open cycle at a time,
    /// so a unit gives its position at most one of each. What the store
    /// already holds is the store's to check.
    fn check_slots(&self) -> Result<(), ModelError> {
        let mut taken: Vec<&'static str> = Vec::new();
        for change in &self.changes {
            let slot = match &change.record {
                Record::Intent(intent) if intent.blocks_entries() => "unresolved entries",
                Record::Cycle(cycle) if cycle.is_open() => "open cycles",
                _ => continue,
            };
            if taken.contains(&slot) {
                return Err(refused(format!(
                    "it gives its position two {slot}; a position has one unresolved entry and \
                     one open cycle at a time"
                )));
            }
            taken.push(slot);
        }
        Ok(())
    }
}

fn refused(reason: String) -> ModelError {
    disagrees("execution unit", reason)
}
