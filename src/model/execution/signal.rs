//! The signal inbox: one record for each signal a profile was sent, and
//! one note for each delivery of it.

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::futures::{
    Delivery, ExternalSignal, ProfileId, SignalHash, SignalId, SignalRefusal,
};
use crate::model::{ModelError, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of [`SignalRecord`]'s stored form.
pub const SIGNAL_RECORD_FORMAT: u32 = 1;

const RECORD: &str = "signal record";

/// What admission made of a signal. A signal is recorded as `received`
/// and judged once; after that its record never changes, and an admitted
/// signal's progress is its intent's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalState {
    /// Recorded, and not yet judged.
    Received,
    /// Admitted: an intent was recorded with it.
    Admitted,
    /// Refused, for the reason the record names.
    Rejected,
    /// Recorded, with nothing to do.
    Noop,
}

impl SignalState {
    pub const INITIAL: Self = Self::Received;

    pub const ALL: [Self; 4] = [Self::Received, Self::Admitted, Self::Rejected, Self::Noop];

    /// Every allowed move: a judgment, made once.
    pub const TRANSITIONS: [(Self, Self); 3] = [
        (Self::Received, Self::Admitted),
        (Self::Received, Self::Rejected),
        (Self::Received, Self::Noop),
    ];

    /// The state as a receipt and the parity matrix spell it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Admitted => "admitted",
            Self::Rejected => "rejected",
            Self::Noop => "noop",
        }
    }

    /// Whether the signal has been judged.
    pub const fn is_judged(self) -> bool {
        !matches!(self, Self::Received)
    }

    /// The state a delivery is answered with, when it was `delivery` to
    /// the inbox: the signal's own for its first delivery, `duplicate`
    /// for one already recorded, and `rejected` for a conflict.
    pub const fn answered(self, delivery: Delivery) -> &'static str {
        match delivery {
            Delivery::New => self.as_str(),
            Delivery::Duplicate => "duplicate",
            Delivery::Conflict => "rejected",
        }
    }
}

impl fmt::Display for SignalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One signal as its profile's inbox holds it: what was delivered, the
/// canonical payload its hash names, and what admission made of it. The
/// inbox holds one record for a profile and a signal ID, whatever happens
/// to any position afterwards: that is what makes a delivery sent again a
/// duplicate for good, and never a second admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SignalRecordParts")]
pub struct SignalRecord {
    format: u32,
    scope: ExecutionScope,
    signal: ExternalSignal,
    payload: String,
    received_at_ms: TimestampMs,
    state: SignalState,
    refusal: Option<SignalRefusal>,
    note: Option<String>,
    judged_at_ms: Option<TimestampMs>,
}

impl SignalRecord {
    /// A delivery just received for the profile `scope` names: the one its
    /// URL selected, whichever profile the signal itself names. `payload`
    /// is the signal's canonical payload, and must be what its hash names.
    pub fn received(
        scope: ExecutionScope,
        signal: ExternalSignal,
        payload: impl Into<String>,
        received_at_ms: TimestampMs,
    ) -> Result<Self, ModelError> {
        Self::checked(SignalRecordParts {
            format: SIGNAL_RECORD_FORMAT,
            scope,
            signal,
            payload: payload.into(),
            received_at_ms,
            state: SignalState::Received,
            refusal: None,
            note: None,
            judged_at_ms: None,
        })
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    /// The profile whose inbox holds the record.
    pub fn profile(&self) -> &ProfileId {
        // Checked when the record was built.
        self.scope.profile().unwrap_or(&self.signal.profile)
    }

    pub fn signal(&self) -> &ExternalSignal {
        &self.signal
    }

    pub fn id(&self) -> &SignalId {
        &self.signal.id
    }

    pub fn payload_hash(&self) -> &SignalHash {
        &self.signal.payload_hash
    }

    /// The canonical payload the hash names.
    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub const fn received_at_ms(&self) -> TimestampMs {
        self.received_at_ms
    }

    pub const fn state(&self) -> SignalState {
        self.state
    }

    /// Why the signal was refused, where it was.
    pub fn refusal(&self) -> Option<&SignalRefusal> {
        self.refusal.as_ref()
    }

    /// What a signal with nothing to do was recorded with.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub const fn judged_at_ms(&self) -> Option<TimestampMs> {
        self.judged_at_ms
    }

    /// The record once admitted.
    pub fn admitted(&self, at_ms: TimestampMs) -> Result<Self, ModelError> {
        self.judged(SignalState::Admitted, None, None, at_ms)
    }

    /// The record once refused.
    pub fn rejected(&self, refusal: SignalRefusal, at_ms: TimestampMs) -> Result<Self, ModelError> {
        self.judged(SignalState::Rejected, Some(refusal), None, at_ms)
    }

    /// The record once found to ask for nothing.
    pub fn noop(&self, note: impl Into<String>, at_ms: TimestampMs) -> Result<Self, ModelError> {
        self.judged(SignalState::Noop, None, Some(note.into()), at_ms)
    }

    fn judged(
        &self,
        to: SignalState,
        refusal: Option<SignalRefusal>,
        note: Option<String>,
        at_ms: TimestampMs,
    ) -> Result<Self, ModelError> {
        if self.state.is_judged() {
            return Err(ModelError::InvalidTransition {
                machine: "signal",
                from: self.state.as_str(),
                to: to.as_str(),
            });
        }
        Ok(Self {
            state: to,
            refusal,
            note,
            judged_at_ms: Some(at_ms),
            ..self.clone()
        })
    }

    /// What a delivery is to this record: the same signal again, or
    /// another payload under its ID.
    pub fn delivery_of(&self, arrived: &SignalHash) -> Delivery {
        Delivery::of(Some(self.payload_hash()), arrived)
    }

    /// Whether `self` may replace `previous`: the same signal, judged once.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let same = self.scope == previous.scope
            && self.signal == previous.signal
            && self.payload == previous.payload
            && self.received_at_ms == previous.received_at_ms;
        if !same {
            return Err(not_the_same(RECORD, "signal"));
        }
        if !SignalState::TRANSITIONS.contains(&(previous.state, self.state)) {
            return Err(ModelError::InvalidTransition {
                machine: "signal",
                from: previous.state.as_str(),
                to: self.state.as_str(),
            });
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, SIGNAL_RECORD_FORMAT, text)
    }

    fn checked(parts: SignalRecordParts) -> Result<Self, ModelError> {
        if parts.format != SIGNAL_RECORD_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: SIGNAL_RECORD_FORMAT,
            });
        }
        parts.scope.profile_of(RECORD)?;
        if SignalHash::of(parts.payload.as_bytes()) != parts.signal.payload_hash {
            return Err(disagrees(
                RECORD,
                "its payload is not the one its hash names",
            ));
        }
        let judged = parts.state.is_judged();
        let consistent = judged == parts.judged_at_ms.is_some()
            && (parts.state == SignalState::Rejected) == parts.refusal.is_some()
            && (parts.note.is_none() || parts.state == SignalState::Noop);
        if !consistent {
            return Err(disagrees(
                RECORD,
                format!(
                    "a {} signal has a judgment time once judged, a refusal exactly when \
                     rejected, and a note only when it asks for nothing",
                    parts.state
                ),
            ));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            signal: parts.signal,
            payload: parts.payload,
            received_at_ms: parts.received_at_ms,
            state: parts.state,
            refusal: parts.refusal,
            note: parts.note,
            judged_at_ms: parts.judged_at_ms,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignalRecordParts {
    format: u32,
    scope: ExecutionScope,
    signal: ExternalSignal,
    payload: String,
    received_at_ms: TimestampMs,
    state: SignalState,
    refusal: Option<SignalRefusal>,
    note: Option<String>,
    judged_at_ms: Option<TimestampMs>,
}

impl TryFrom<SignalRecordParts> for SignalRecord {
    type Error = ModelError;

    fn try_from(parts: SignalRecordParts) -> Result<Self, Self::Error> {
        Self::checked(parts)
    }
}

/// One delivery of a signal, as the inbox notes it: every authenticated
/// delivery leaves one, the first and each one after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalDelivery {
    pub scope: ExecutionScope,
    pub signal: SignalId,
    /// What the delivery was to the inbox when it arrived.
    pub disposition: Delivery,
    pub payload_hash: SignalHash,
    /// The canonical payload of a conflicting delivery, which the signal's
    /// own record does not hold.
    pub payload: Option<String>,
    pub fire_time_ms: TimestampMs,
    pub received_at_ms: TimestampMs,
}

impl SignalDelivery {
    /// The note of `arrived`, a delivery that was `disposition` to the
    /// inbox. Only a conflicting delivery keeps its payload: the first
    /// one's is in the signal's record, and a duplicate's is the same.
    pub fn of(arrived: &SignalRecord, disposition: Delivery) -> Self {
        Self {
            scope: arrived.scope.clone(),
            signal: arrived.signal.id.clone(),
            disposition,
            payload_hash: arrived.signal.payload_hash,
            payload: (disposition == Delivery::Conflict).then(|| arrived.payload.clone()),
            fire_time_ms: arrived.signal.fire_time_ms,
            received_at_ms: arrived.received_at_ms,
        }
    }
}

#[cfg(test)]
mod tests;
