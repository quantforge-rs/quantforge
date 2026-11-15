//! Incidents: what halts entries until an operator has looked (ADR 0002
//! §5, §8).

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::futures::{PositionCycleId, PositionKey, ProfileId};
use crate::model::identity::text_id;
use crate::model::{InstrumentId, ModelError, OperationId, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of [`Incident`]'s stored form.
pub const INCIDENT_FORMAT: u32 = 1;

const RECORD: &str = "incident";

/// Where an incident stands, as the parity matrix freezes it as
/// `incident`. Only an operator clears one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IncidentState {
    Open,
    Cleared,
}

impl IncidentState {
    pub const INITIAL: Self = Self::Open;

    pub const ALL: [Self; 2] = [Self::Open, Self::Cleared];

    /// Every allowed move, as the parity matrix lists them.
    pub const TRANSITIONS: [(Self, Self); 1] = [(Self::Open, Self::Cleared)];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Cleared => "cleared",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Cleared)
    }
}

impl fmt::Display for IncidentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why an incident was raised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum IncidentKind {
    /// An entry still unsettled when its settle window ended.
    UnsettledEntry,
    /// A position whose stop could not be placed or repaired.
    ProtectionFailure,
    /// Orders on the account that carry the profile's owner tag and
    /// another store's activation epoch.
    SecondWriter,
}

impl IncidentKind {
    pub const ALL: [Self; 3] = [
        Self::UnsettledEntry,
        Self::ProtectionFailure,
        Self::SecondWriter,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsettledEntry => "unsettled_entry",
            Self::ProtectionFailure => "protection_failure",
            Self::SecondWriter => "second_writer",
        }
    }
}

impl fmt::Display for IncidentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which entries an open incident halts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Halt {
    /// Entries on one instrument of the incident's account.
    Position(InstrumentId),
    /// Every entry of the incident's profile.
    Profile,
}

text_id!(
    /// What makes an incident the same one when it is raised again, such
    /// as `op/7/unsettled_entry`: unique within its account and product
    /// line, so one cause raises one incident.
    IncidentKey,
    "incident key",
    "1 to 200 characters without control characters",
    |text| !text.is_empty() && text.chars().count() <= 200 && !text.chars().any(char::is_control)
);

/// When an operator cleared an incident, and what they noted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clearance {
    pub at_ms: TimestampMs,
    pub note: String,
}

/// One incident: what happened, what it halts, and whether an operator
/// has cleared it. While it is open, the entries it halts are refused as
/// `incident_open`; exits, stops and the protective close go on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "IncidentParts")]
pub struct Incident {
    format: u32,
    scope: ExecutionScope,
    key: IncidentKey,
    kind: IncidentKind,
    halts: Halt,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
    message: String,
    opened_at_ms: TimestampMs,
    cleared: Option<Clearance>,
}

impl Incident {
    /// An incident just raised.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        scope: ExecutionScope,
        key: IncidentKey,
        kind: IncidentKind,
        halts: Halt,
        operation: Option<OperationId>,
        cycle: Option<PositionCycleId>,
        message: impl Into<String>,
        opened_at_ms: TimestampMs,
    ) -> Result<Self, ModelError> {
        Self::try_from(IncidentParts {
            format: INCIDENT_FORMAT,
            scope,
            key,
            kind,
            halts,
            operation,
            cycle,
            message: message.into(),
            opened_at_ms,
            cleared: None,
        })
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub fn key(&self) -> &IncidentKey {
        &self.key
    }

    pub const fn kind(&self) -> IncidentKind {
        self.kind
    }

    pub fn halts(&self) -> &Halt {
        &self.halts
    }

    pub const fn operation(&self) -> Option<OperationId> {
        self.operation
    }

    pub const fn cycle(&self) -> Option<PositionCycleId> {
        self.cycle
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn opened_at_ms(&self) -> TimestampMs {
        self.opened_at_ms
    }

    pub fn cleared(&self) -> Option<&Clearance> {
        self.cleared.as_ref()
    }

    pub const fn state(&self) -> IncidentState {
        match self.cleared {
            None => IncidentState::Open,
            Some(_) => IncidentState::Cleared,
        }
    }

    /// Whether the incident, while open, halts `profile`'s entries on
    /// `key`: every profile's on the instrument it names, on its own
    /// account, or every entry of its own profile.
    pub fn halts_entries(&self, profile: &ProfileId, key: &PositionKey) -> bool {
        self.cleared.is_none()
            && self.scope.covers(key)
            && match &self.halts {
                Halt::Position(instrument) => instrument == key.instrument(),
                Halt::Profile => self.scope.profile() == Some(profile),
            }
    }

    /// The incident once an operator cleared it. One cleared stays so.
    pub fn clear(&self, note: impl Into<String>, at_ms: TimestampMs) -> Result<Self, ModelError> {
        if self.cleared.is_some() {
            return Err(ModelError::InvalidTransition {
                machine: "incident",
                from: IncidentState::Cleared.as_str(),
                to: IncidentState::Cleared.as_str(),
            });
        }
        Ok(Self {
            cleared: Some(Clearance {
                at_ms,
                note: note.into(),
            }),
            ..self.clone()
        })
    }

    /// Whether `self` may replace `previous`: the same incident, cleared
    /// once.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let same = Self {
            cleared: None,
            ..self.clone()
        } == Self {
            cleared: None,
            ..previous.clone()
        };
        if !same {
            return Err(not_the_same(RECORD, "cause"));
        }
        if previous.cleared.is_some() || self.cleared.is_none() {
            return Err(ModelError::InvalidTransition {
                machine: "incident",
                from: previous.state().as_str(),
                to: self.state().as_str(),
            });
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, INCIDENT_FORMAT, text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentParts {
    format: u32,
    scope: ExecutionScope,
    key: IncidentKey,
    kind: IncidentKind,
    halts: Halt,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
    message: String,
    opened_at_ms: TimestampMs,
    cleared: Option<Clearance>,
}

impl TryFrom<IncidentParts> for Incident {
    type Error = ModelError;

    fn try_from(parts: IncidentParts) -> Result<Self, Self::Error> {
        if parts.format != INCIDENT_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: INCIDENT_FORMAT,
            });
        }
        parts.scope.profile_of(RECORD)?;
        if let Halt::Position(instrument) = &parts.halts {
            parts.scope.holding(RECORD, instrument)?;
        }
        if parts.message.trim().is_empty() {
            return Err(disagrees(RECORD, "it says nothing of what happened"));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            key: parts.key,
            kind: parts.kind,
            halts: parts.halts,
            operation: parts.operation,
            cycle: parts.cycle,
            message: parts.message,
            opened_at_ms: parts.opened_at_ms,
            cleared: parts.cleared,
        })
    }
}
