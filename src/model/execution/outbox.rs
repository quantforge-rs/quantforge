//! The event outbox: what happened, recorded in the transaction that made
//! it happen, for a notifier to deliver afterwards (ADR 0002 §1).

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::futures::{PositionCycleId, ProfileId, SignalId};
use crate::model::identity::text_id;
use crate::model::{InstrumentId, ModelError, OperationId, TimestampMs};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The version of [`OutboxEvent`]'s stored form.
pub const EVENT_FORMAT: u32 = 1;

const RECORD: &str = "outbox event";

/// Where an event's delivery stands, as the parity matrix freezes it as
/// `notification`. A send that may or may not have arrived is
/// `outcome_unknown`, never `delivered` and never sent again unasked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeliveryState {
    Pending,
    Delivered,
    OutcomeUnknown,
}

impl DeliveryState {
    pub const INITIAL: Self = Self::Pending;

    pub const ALL: [Self; 3] = [Self::Pending, Self::Delivered, Self::OutcomeUnknown];

    /// Every allowed move, as the parity matrix lists them.
    pub const TRANSITIONS: [(Self, Self); 2] = [
        (Self::Pending, Self::Delivered),
        (Self::Pending, Self::OutcomeUnknown),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

impl fmt::Display for DeliveryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an event tells of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EventKind {
    /// A signal was admitted. Nothing has been sized, sent or filled.
    SignalAdmitted,
    /// A signal was recorded and refused.
    SignalRejected,
    /// A signal was recorded with nothing to do.
    SignalNoop,
    /// A delivery came under a recorded signal's ID with another payload.
    SignalConflict,
    /// An order was recorded and is about to be sent. That is not a fill.
    OrderSubmitted,
    /// An order's request ended with no answer; it is being looked up.
    OutcomeUnknown,
    /// An order's outcome was settled from the venue's evidence.
    OrderSettled,
    /// An incident was raised, and halts entries.
    IncidentOpened,
    /// An operator cleared an incident.
    IncidentCleared,
}

impl EventKind {
    pub const ALL: [Self; 9] = [
        Self::SignalAdmitted,
        Self::SignalRejected,
        Self::SignalNoop,
        Self::SignalConflict,
        Self::OrderSubmitted,
        Self::OutcomeUnknown,
        Self::OrderSettled,
        Self::IncidentOpened,
        Self::IncidentCleared,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SignalAdmitted => "signal_admitted",
            Self::SignalRejected => "signal_rejected",
            Self::SignalNoop => "signal_noop",
            Self::SignalConflict => "signal_conflict",
            Self::OrderSubmitted => "order_submitted",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::OrderSettled => "order_settled",
            Self::IncidentOpened => "incident_opened",
            Self::IncidentCleared => "incident_cleared",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

text_id!(
    /// What makes an event the same one when its cause is seen again:
    /// unique within its account and product line, so one cause is told
    /// once however often it is reached.
    EventKey,
    "event key",
    "1 to 400 characters without control characters",
    |text| !text.is_empty() && text.chars().count() <= 400 && !text.chars().any(char::is_control)
);

impl EventKey {
    /// The key of what `word` says of `profile`'s `signal`. Only the
    /// signal's ID is free text, and it comes last, so two profiles'
    /// signals never share a key, nor two signals of one profile.
    pub fn of_signal(profile: &ProfileId, word: &str, signal: &SignalId) -> Self {
        Self(format!("sig/{profile}/{word}/{signal}"))
    }

    /// The key of what `word` says of `operation`, which its store
    /// numbered.
    pub fn of_operation(operation: OperationId, word: &str) -> Self {
        Self(format!("op/{operation}/{word}"))
    }
}

/// One event: what happened, to what, and whether it has been delivered.
/// Its facts are text, each amount as the exact decimal it was, so an
/// event says the same thing on every store and after every copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EventParts")]
pub struct OutboxEvent {
    format: u32,
    scope: ExecutionScope,
    key: EventKey,
    kind: EventKind,
    instrument: Option<InstrumentId>,
    signal: Option<SignalId>,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
    facts: BTreeMap<String, String>,
    created_at_ms: TimestampMs,
    delivery: DeliveryState,
    delivery_at_ms: Option<TimestampMs>,
}

impl OutboxEvent {
    /// An event just created, waiting to be delivered.
    pub fn pending(
        scope: ExecutionScope,
        key: EventKey,
        kind: EventKind,
        created_at_ms: TimestampMs,
    ) -> Self {
        Self {
            format: EVENT_FORMAT,
            scope,
            key,
            kind,
            instrument: None,
            signal: None,
            operation: None,
            cycle: None,
            facts: BTreeMap::new(),
            created_at_ms,
            delivery: DeliveryState::INITIAL,
            delivery_at_ms: None,
        }
    }

    /// The event, about `instrument`.
    pub fn on(mut self, instrument: &InstrumentId) -> Self {
        self.instrument = Some(instrument.clone());
        self
    }

    /// The event, about `signal`.
    pub fn of_signal(mut self, signal: &SignalId) -> Self {
        self.signal = Some(signal.clone());
        self
    }

    /// The event, about `operation`.
    pub fn of_operation(mut self, operation: OperationId) -> Self {
        self.operation = Some(operation);
        self
    }

    /// The event, about `cycle`.
    pub fn of_cycle(mut self, cycle: PositionCycleId) -> Self {
        self.cycle = Some(cycle);
        self
    }

    /// The event, with one more fact. A decimal is given as its exact
    /// text.
    pub fn with(mut self, name: &str, value: impl fmt::Display) -> Self {
        self.facts.insert(name.to_string(), value.to_string());
        self
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub fn key(&self) -> &EventKey {
        &self.key
    }

    pub const fn kind(&self) -> EventKind {
        self.kind
    }

    pub fn instrument(&self) -> Option<&InstrumentId> {
        self.instrument.as_ref()
    }

    pub fn signal(&self) -> Option<&SignalId> {
        self.signal.as_ref()
    }

    pub const fn operation(&self) -> Option<OperationId> {
        self.operation
    }

    pub const fn cycle(&self) -> Option<PositionCycleId> {
        self.cycle
    }

    pub fn facts(&self) -> &BTreeMap<String, String> {
        &self.facts
    }

    pub const fn created_at_ms(&self) -> TimestampMs {
        self.created_at_ms
    }

    pub const fn delivery(&self) -> DeliveryState {
        self.delivery
    }

    pub const fn delivery_at_ms(&self) -> Option<TimestampMs> {
        self.delivery_at_ms
    }

    /// The event once its delivery ended as `to`: delivered, or with an
    /// outcome nobody knows. Neither is undone.
    pub fn delivery_ended(
        &self,
        to: DeliveryState,
        at_ms: TimestampMs,
    ) -> Result<Self, ModelError> {
        if !DeliveryState::TRANSITIONS.contains(&(self.delivery, to)) {
            return Err(ModelError::InvalidTransition {
                machine: "notification",
                from: self.delivery.as_str(),
                to: to.as_str(),
            });
        }
        Ok(Self {
            delivery: to,
            delivery_at_ms: Some(at_ms),
            ..self.clone()
        })
    }

    /// Whether `self` may replace `previous`: the same event, its delivery
    /// ended once.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let body = |event: &Self| Self {
            delivery: DeliveryState::INITIAL,
            delivery_at_ms: None,
            ..event.clone()
        };
        if body(self) != body(previous) {
            return Err(not_the_same(RECORD, "content"));
        }
        if !DeliveryState::TRANSITIONS.contains(&(previous.delivery, self.delivery)) {
            return Err(ModelError::InvalidTransition {
                machine: "notification",
                from: previous.delivery.as_str(),
                to: self.delivery.as_str(),
            });
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, EVENT_FORMAT, text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventParts {
    format: u32,
    scope: ExecutionScope,
    key: EventKey,
    kind: EventKind,
    instrument: Option<InstrumentId>,
    signal: Option<SignalId>,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
    facts: BTreeMap<String, String>,
    created_at_ms: TimestampMs,
    delivery: DeliveryState,
    delivery_at_ms: Option<TimestampMs>,
}

impl TryFrom<EventParts> for OutboxEvent {
    type Error = ModelError;

    fn try_from(parts: EventParts) -> Result<Self, Self::Error> {
        if parts.format != EVENT_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: EVENT_FORMAT,
            });
        }
        if let Some(instrument) = &parts.instrument {
            parts.scope.holding(RECORD, instrument)?;
        }
        if parts.signal.is_some() && parts.scope.profile().is_none() {
            return Err(disagrees(
                RECORD,
                "it names a signal and no profile, and a signal is always a profile's",
            ));
        }
        if parts.delivery.is_terminal() != parts.delivery_at_ms.is_some() {
            return Err(disagrees(
                RECORD,
                "its delivery has a time exactly once it has ended",
            ));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            key: parts.key,
            kind: parts.kind,
            instrument: parts.instrument,
            signal: parts.signal,
            operation: parts.operation,
            cycle: parts.cycle,
            facts: parts.facts,
            created_at_ms: parts.created_at_ms,
            delivery: parts.delivery,
            delivery_at_ms: parts.delivery_at_ms,
        })
    }
}
