//! Operations: every action that changes something on an exchange has
//! one (ADR 0002 §2), numbered by the store that records it, and recorded
//! before any request is sent for it.

use super::{ExecutionScope, Replacement, disagrees, from_stored, not_the_same, to_stored};
use crate::model::futures::{PositionCycleId, ProfileId, SignalId};
use crate::model::identity::text_id;
use crate::model::{InstrumentId, ModelError, OperationId};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of [`OperationRecord`]'s stored form.
pub const OPERATION_FORMAT: u32 = 1;

const RECORD: &str = "operation record";

/// What an operation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OperationKind {
    /// A market entry: the only kind that opens exposure.
    Entry,
    /// A reduce-only close: an exit signal's, or the protective close.
    Close,
    /// Placing, replacing or cancelling a position's stop.
    Stop,
    /// Setting an instrument's leverage.
    Leverage,
    /// A Spot order amended, or cancelled and replaced.
    SpotReplace,
}

impl OperationKind {
    pub const ALL: [Self; 5] = [
        Self::Entry,
        Self::Close,
        Self::Stop,
        Self::Leverage,
        Self::SpotReplace,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Close => "close",
            Self::Stop => "stop",
            Self::Leverage => "leverage",
            Self::SpotReplace => "spot_replace",
        }
    }

    /// Whether the kind acts on a futures position.
    const fn is_futures(self) -> bool {
        !matches!(self, Self::SpotReplace)
    }
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

text_id!(
    /// What makes an operation the same one when it is asked for again: a
    /// command sent twice, or a signal's operation looked for after a
    /// restart. It is unique within its account and product line; an
    /// operation without one is always new.
    OperationKey,
    "operation key",
    "1 to 400 characters without control characters",
    |text| !text.is_empty() && text.chars().count() <= 400 && !text.chars().any(char::is_control)
);

impl OperationKey {
    /// The key of the `ordinal`th operation carrying out `signal` for
    /// `profile`: 1 for the first. Only the signal's ID is free text, and
    /// it comes last, so no two signals share a key.
    pub fn of_signal(profile: &ProfileId, signal: &SignalId, ordinal: u32) -> Self {
        Self(format!("sig/{profile}/{ordinal}/{signal}"))
    }
}

/// An operation to record: everything but its number, which the store
/// gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewOperation {
    pub scope: ExecutionScope,
    pub kind: OperationKind,
    pub key: Option<OperationKey>,
    pub instrument: InstrumentId,
    /// The signal the operation carries out, when one caused it.
    pub signal: Option<SignalId>,
    /// The position cycle it belongs to, once it has one.
    pub cycle: Option<PositionCycleId>,
    pub replacement: Option<Replacement>,
}

impl NewOperation {
    /// The operation as `id`, or why it is not one a store keeps.
    pub fn numbered(self, id: OperationId) -> Result<OperationRecord, ModelError> {
        OperationRecord::try_from(OperationParts {
            format: OPERATION_FORMAT,
            id,
            scope: self.scope,
            kind: self.kind,
            key: self.key,
            instrument: self.instrument,
            signal: self.signal,
            cycle: self.cycle,
            replacement: self.replacement,
        })
    }
}

/// One operation: who acts, on what, and what it is tied to. Its number is
/// its identity. The signal that caused it and the cycle it belongs to are
/// each optional and each its own: an operation started by a command has
/// no signal, and a Spot replacement has no position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "OperationParts")]
pub struct OperationRecord {
    format: u32,
    id: OperationId,
    scope: ExecutionScope,
    kind: OperationKind,
    key: Option<OperationKey>,
    instrument: InstrumentId,
    signal: Option<SignalId>,
    cycle: Option<PositionCycleId>,
    replacement: Option<Replacement>,
}

impl OperationRecord {
    pub const fn format(&self) -> u32 {
        self.format
    }

    pub const fn id(&self) -> OperationId {
        self.id
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub const fn kind(&self) -> OperationKind {
        self.kind
    }

    pub fn key(&self) -> Option<&OperationKey> {
        self.key.as_ref()
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub fn signal(&self) -> Option<&SignalId> {
        self.signal.as_ref()
    }

    pub const fn cycle(&self) -> Option<PositionCycleId> {
        self.cycle
    }

    pub fn replacement(&self) -> Option<&Replacement> {
        self.replacement.as_ref()
    }

    /// The operation tied to `cycle`. A cycle, once named, is never
    /// replaced.
    pub fn in_cycle(&self, cycle: PositionCycleId) -> Result<Self, ModelError> {
        Self::try_from(OperationParts {
            cycle: Some(cycle),
            ..self.parts()
        })
        .and_then(|next| next.follows(self).map(|()| next))
    }

    /// The operation with its replacement moved on.
    pub fn with_replacement(&self, replacement: Replacement) -> Result<Self, ModelError> {
        Self::try_from(OperationParts {
            replacement: Some(replacement),
            ..self.parts()
        })
        .and_then(|next| next.follows(self).map(|()| next))
    }

    /// Whether `self` may replace `previous`: the same operation, with a
    /// cycle named at most once and its replacement moved along its state
    /// machine or not at all.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let same = self.id == previous.id
            && self.scope == previous.scope
            && self.kind == previous.kind
            && self.key == previous.key
            && self.instrument == previous.instrument
            && self.signal == previous.signal;
        if !same {
            return Err(not_the_same(RECORD, "identity"));
        }
        if previous.cycle.is_some() && self.cycle != previous.cycle {
            return Err(not_the_same(RECORD, "position cycle"));
        }
        match (&previous.replacement, &self.replacement) {
            (Some(old), Some(new)) => new.follows(old),
            (None, None) => Ok(()),
            _ => Err(not_the_same(RECORD, "replacement")),
        }
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, OPERATION_FORMAT, text)
    }

    fn parts(&self) -> OperationParts {
        OperationParts {
            format: self.format,
            id: self.id,
            scope: self.scope.clone(),
            kind: self.kind,
            key: self.key.clone(),
            instrument: self.instrument.clone(),
            signal: self.signal.clone(),
            cycle: self.cycle,
            replacement: self.replacement.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationParts {
    format: u32,
    id: OperationId,
    scope: ExecutionScope,
    kind: OperationKind,
    key: Option<OperationKey>,
    instrument: InstrumentId,
    signal: Option<SignalId>,
    cycle: Option<PositionCycleId>,
    replacement: Option<Replacement>,
}

impl TryFrom<OperationParts> for OperationRecord {
    type Error = ModelError;

    fn try_from(parts: OperationParts) -> Result<Self, Self::Error> {
        if parts.format != OPERATION_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: OPERATION_FORMAT,
            });
        }
        parts.scope.holding(RECORD, &parts.instrument)?;
        let futures = parts.scope.category().is_futures();
        if parts.kind.is_futures() != futures {
            return Err(disagrees(
                RECORD,
                format!(
                    "a {} operation does not act on a {} instrument",
                    parts.kind,
                    parts.scope.category()
                ),
            ));
        }
        if (parts.kind == OperationKind::SpotReplace) != parts.replacement.is_some() {
            return Err(disagrees(
                RECORD,
                "a replacement names the order it replaces, and no other operation does",
            ));
        }
        if parts.signal.is_some() && parts.scope.profile().is_none() {
            return Err(disagrees(
                RECORD,
                "it names a signal and no profile, and a signal is always a profile's",
            ));
        }
        if parts.cycle.is_some() && !futures {
            return Err(disagrees(
                RECORD,
                "it names a position cycle, and spot holds no position",
            ));
        }
        Ok(Self {
            format: parts.format,
            id: parts.id,
            scope: parts.scope,
            kind: parts.kind,
            key: parts.key,
            instrument: parts.instrument,
            signal: parts.signal,
            cycle: parts.cycle,
            replacement: parts.replacement,
        })
    }
}

#[cfg(test)]
mod tests;
