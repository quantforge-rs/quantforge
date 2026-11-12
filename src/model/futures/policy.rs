//! An automation profile's policy (ADR 0002 §5, §6): the settings its
//! entries are sized from, and nothing a signal can change. A profile has
//! defaults, settings of their own for the assets that need them, and the
//! capital it risks in each settlement asset. Every change makes a new
//! revision of the whole document, which a store keeps beside the earlier
//! ones, so the policy an entry was admitted under can always be read back.
//!
//! - `document`: the policy as a store keeps it, checked whenever it is
//!   built or read.
//! - `change`: the changes an operator makes, each a new revision.
//! - `effective`: what an instrument trades on once the defaults are
//!   applied, and where each setting came from.
//! - `snapshot`: what an admitted entry carries, the stop it uses, and why
//!   an entry is refused.
//! - `check`: the policy held against the venue's own instrument list.
//!
//! Percentages are in percent, as an operator writes them: a risk of `1`
//! is 1 % of the capital, and a stop of `3` sits 3 % from the entry.

mod change;
mod check;
mod document;
mod effective;
mod snapshot;

use crate::model::identity::text_id;
use crate::model::units::{canonical, decimal_unit, parse_decimal};
use crate::model::{ExchangeId, ModelError, TimestampMs};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

pub use change::{AssetEdit, DefaultsEdit, Edit, NewAsset, PolicyChange};
pub use check::{PolicyProblem, PolicyProblemKind, PolicySubject};
pub use document::{
    AssetPolicy, Capital, POLICY_FORMAT, PolicyDefaults, ProfilePolicy, StopFallback,
};
pub use effective::{EffectivePolicy, SettingSource, Sourced};
pub use snapshot::{EntryStop, POLICY_SNAPSHOT_FORMAT, PolicyRefusal, PolicySnapshot};

decimal_unit!(
    /// The share of a profile's capital one entry may lose at its stop, in
    /// percent: `1` is 1 %. The risk budget is the capital times it; a
    /// signal never changes it.
    RiskPercent,
    "risk percentage",
    "above 0 and at most 100",
    |value| value > Decimal::ZERO && value <= Decimal::ONE_HUNDRED
);

decimal_unit!(
    /// How far a fallback stop sits from the expected entry, in percent of
    /// that price: `3` puts a LONG's stop 3 % below it and a SHORT's 3 %
    /// above. It applies only to an entry whose alert brings no stop.
    StopPercent,
    "stop percentage",
    "above 0 and below 100",
    |value| value > Decimal::ZERO && value < Decimal::ONE_HUNDRED
);

text_id!(
    /// A name an operator gives a profile, shown beside its ID. It is
    /// metadata: changing it never changes what the profile trades.
    ProfileLabel,
    "profile label",
    "1 to 64 characters, none of them a control character, with no space at either end",
    |text| {
        (1..=64).contains(&text.chars().count())
            && !text.chars().any(char::is_control)
            && text.trim() == text
    }
);

/// A revision of a profile's policy: 1 when the profile is created, and one
/// more with every change. A store keeps every revision, so the number
/// names exactly one policy for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64")]
pub struct PolicyRevision(u64);

impl PolicyRevision {
    /// The revision a profile is created at.
    pub const FIRST: Self = Self(1);

    /// The greatest revision: every store keeps one as a signed 64-bit
    /// integer.
    const MAX: u64 = i64::MAX as u64;

    /// Refuses 0, which no profile has, and a number no store could keep.
    pub fn new(value: u64) -> Result<Self, ModelError> {
        if value == 0 || value > Self::MAX {
            return Err(ModelError::InvalidIdentifier {
                what: "policy revision",
                value: value.to_string(),
                rule: "from 1 to 9223372036854775807",
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// The revision the next change makes.
    pub fn next(self) -> Result<Self, ModelError> {
        Self::new(self.0.saturating_add(1))
    }

    /// The revision before this one, or `None` for the first.
    pub fn previous(self) -> Option<Self> {
        (self.0 > 1).then(|| Self(self.0 - 1))
    }
}

impl TryFrom<u64> for PolicyRevision {
    type Error = ModelError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for PolicyRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// When a revision of a profile's policy was made: what a store answers
/// for each revision it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PolicyStamp {
    pub revision: PolicyRevision,
    pub changed_at_ms: TimestampMs,
}

/// A product line as messages name it: its venue and its family, such as
/// `bybit linear`.
pub(super) fn line_name(line: ExchangeId) -> String {
    format!("{} {}", line.venue(), line.category())
}

/// A fault in a policy, or in a change to one, as every part of this module
/// reports it.
pub(super) fn invalid(record: &'static str, reason: impl Into<String>) -> ModelError {
    ModelError::InvalidRecord {
        record,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests;
