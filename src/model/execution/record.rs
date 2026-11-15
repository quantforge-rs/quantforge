//! One record of a unit, whatever its kind: where it is kept, what it is
//! known by, the position it is part of, and whether it may follow the
//! record it replaces.

use super::{
    CycleRecord, ExecutionScope, FillRecord, Incident, IntentRecord, OperationRecord, OrderRecord,
    OutboxEvent, Reservation, SignalRecord, disagrees,
};
use crate::model::ModelError;
use crate::model::futures::PositionKey;
use std::fmt;

/// A record as a store read it, with the revision a later change names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored<T> {
    pub record: T,
    /// 1 when the record was first written, and one more for each change.
    pub revision: u64,
}

/// What a change expects to find where it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Expect {
    /// No record: the change writes the first.
    Absent,
    /// The record at this revision: the change replaces it.
    Revision(u64),
}

/// The kinds of record a unit changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordKind {
    Signal,
    Operation,
    Intent,
    Order,
    Fill,
    Cycle,
    Reservation,
    Incident,
    Event,
}

impl RecordKind {
    pub const ALL: [Self; 9] = [
        Self::Signal,
        Self::Operation,
        Self::Intent,
        Self::Order,
        Self::Fill,
        Self::Cycle,
        Self::Reservation,
        Self::Incident,
        Self::Event,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::Operation => "operation",
            Self::Intent => "intent",
            Self::Order => "order",
            Self::Fill => "fill",
            Self::Cycle => "cycle",
            Self::Reservation => "reservation",
            Self::Incident => "incident",
            Self::Event => "event",
        }
    }
}

impl fmt::Display for RecordKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One record a unit writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Signal(SignalRecord),
    Operation(OperationRecord),
    Intent(IntentRecord),
    Order(OrderRecord),
    Fill(FillRecord),
    Cycle(CycleRecord),
    Reservation(Reservation),
    Incident(Incident),
    Event(OutboxEvent),
}

impl Record {
    pub const fn kind(&self) -> RecordKind {
        match self {
            Self::Signal(_) => RecordKind::Signal,
            Self::Operation(_) => RecordKind::Operation,
            Self::Intent(_) => RecordKind::Intent,
            Self::Order(_) => RecordKind::Order,
            Self::Fill(_) => RecordKind::Fill,
            Self::Cycle(_) => RecordKind::Cycle,
            Self::Reservation(_) => RecordKind::Reservation,
            Self::Incident(_) => RecordKind::Incident,
            Self::Event(_) => RecordKind::Event,
        }
    }

    /// What the record is known by among its kind, for a person to read:
    /// `main/trend:entry:long:SOLUSDT.P:…` for a signal, `7` for an
    /// operation.
    pub fn name(&self) -> String {
        match self {
            Self::Signal(signal) => format!("{}/{}", signal.profile(), signal.id()),
            Self::Operation(operation) => operation.id().to_string(),
            Self::Intent(intent) => format!("{}/{}", intent.profile(), intent.signal()),
            Self::Order(order) => order.client_order_id().to_string(),
            Self::Fill(fill) => {
                format!("{}/{}", fill.fill().instrument.symbol, fill.trade_id())
            }
            Self::Cycle(cycle) => cycle.id().get().to_string(),
            Self::Reservation(reservation) => format!("operation {}", reservation.operation()),
            Self::Incident(incident) => incident.key().to_string(),
            Self::Event(event) => event.key().to_string(),
        }
    }

    /// What a store knows the record by among its kind, so that two
    /// changes with one identity are changes of one record: a signal and
    /// its intent within their profile, an order, a fill, an incident and
    /// an event within their account, and an operation, its reservation
    /// and a cycle by the number the store gave. A name alone says less: a
    /// profile of the same name on another account has records of its own.
    pub(super) fn identity(&self) -> (RecordKind, Option<ExecutionScope>, String) {
        let scope = self.scope();
        let within = match self {
            Self::Signal(_) | Self::Intent(_) => Some(scope.clone()),
            Self::Order(_) | Self::Fill(_) | Self::Incident(_) | Self::Event(_) => {
                Some(scope.of_its_account())
            }
            Self::Operation(_) | Self::Cycle(_) | Self::Reservation(_) => None,
        };
        (self.kind(), within, self.name())
    }

    /// Whose record it is: the venue, the product family, the environment,
    /// the account and, where one acts, the profile.
    pub fn scope(&self) -> &ExecutionScope {
        match self {
            Self::Signal(record) => record.scope(),
            Self::Operation(record) => record.scope(),
            Self::Intent(record) => record.scope(),
            Self::Order(record) => record.scope(),
            Self::Fill(record) => record.scope(),
            Self::Cycle(record) => record.scope(),
            Self::Reservation(record) => record.scope(),
            Self::Incident(record) => record.scope(),
            Self::Event(record) => record.scope(),
        }
    }

    /// The futures position the record is part of, whose lease a unit
    /// that changes it is written under. A signal, an operation, an
    /// incident, an event and anything on Spot is part of none.
    pub fn position(&self) -> Result<Option<PositionKey>, ModelError> {
        let on = |scope: &ExecutionScope, instrument| {
            if scope.category().is_futures() {
                scope.position_key(instrument).map(Some)
            } else {
                Ok(None)
            }
        };
        match self {
            Self::Intent(intent) => Ok(Some(intent.key().clone())),
            Self::Cycle(cycle) => Ok(Some(cycle.key().clone())),
            Self::Reservation(held) => on(held.scope(), held.instrument()),
            Self::Order(order) => on(order.scope(), order.instrument()),
            Self::Fill(fill) => on(fill.scope(), &fill.fill().instrument),
            Self::Signal(_) | Self::Operation(_) | Self::Incident(_) | Self::Event(_) => Ok(None),
        }
    }

    /// Whether `self` may replace `previous`: the same record, changed
    /// only as its own rules allow.
    pub fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        match (self, previous) {
            (Self::Signal(next), Self::Signal(old)) => next.follows(old),
            (Self::Operation(next), Self::Operation(old)) => next.follows(old),
            (Self::Intent(next), Self::Intent(old)) => next.follows(old),
            (Self::Order(next), Self::Order(old)) => next.follows(old),
            (Self::Cycle(next), Self::Cycle(old)) => next.follows(old),
            (Self::Reservation(next), Self::Reservation(old)) => next.follows(old),
            (Self::Incident(next), Self::Incident(old)) => next.follows(old),
            (Self::Event(next), Self::Event(old)) => next.follows(old),
            _ => Err(disagrees(
                "execution unit",
                format!("a {} is recorded once and never replaced", self.kind()),
            )),
        }
    }

    /// The record as a store keeps it.
    pub fn to_stored(&self) -> Result<String, ModelError> {
        match self {
            Self::Signal(record) => record.to_stored(),
            Self::Operation(record) => record.to_stored(),
            Self::Intent(record) => record.to_stored(),
            Self::Order(record) => record.to_stored(),
            Self::Fill(record) => record.to_stored(),
            Self::Cycle(record) => record.to_stored(),
            Self::Reservation(record) => record.to_stored(),
            Self::Incident(record) => record.to_stored(),
            Self::Event(record) => record.to_stored(),
        }
    }

    /// Reads a stored record of `kind` back.
    pub fn from_stored(kind: RecordKind, text: &str) -> Result<Self, ModelError> {
        Ok(match kind {
            RecordKind::Signal => Self::Signal(SignalRecord::from_stored(text)?),
            RecordKind::Operation => Self::Operation(OperationRecord::from_stored(text)?),
            RecordKind::Intent => Self::Intent(IntentRecord::from_stored(text)?),
            RecordKind::Order => Self::Order(OrderRecord::from_stored(text)?),
            RecordKind::Fill => Self::Fill(FillRecord::from_stored(text)?),
            RecordKind::Cycle => Self::Cycle(CycleRecord::from_stored(text)?),
            RecordKind::Reservation => Self::Reservation(Reservation::from_stored(text)?),
            RecordKind::Incident => Self::Incident(Incident::from_stored(text)?),
            RecordKind::Event => Self::Event(OutboxEvent::from_stored(text)?),
        })
    }
}
