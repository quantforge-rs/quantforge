//! What an admitted entry carries of its profile's policy: whose it is,
//! the exact settings it is sized from and the revisions they come from,
//! taken once and never changed. A signal contributes one thing only, its
//! own stop; the account, the risk, the leverage, the capital and whether
//! the asset may trade are the operator's, and nothing a payload holds
//! reaches them.

use super::document::ProfilePolicy;
use super::effective::Sourced;
use super::{PolicyRevision, RiskPercent, StopPercent, invalid};
use crate::model::futures::{FuturesInstrument, ProfileId};
use crate::model::{AccountRef, InstrumentId, Leverage, ModelError, Money, Price, Refusal};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of [`PolicySnapshot`]'s stored form. A snapshot of another
/// version is refused, never guessed at.
pub const POLICY_SNAPSHOT_FORMAT: u32 = 1;

/// Why the policy refuses an entry: one of the parity matrix's refusals,
/// with what the operator would change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRefusal {
    pub refusal: Refusal,
    pub message: String,
}

impl PolicyRefusal {
    fn new(refusal: Refusal, message: impl Into<String>) -> Self {
        Self {
            refusal,
            message: message.into(),
        }
    }
}

impl fmt::Display for PolicyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.refusal, self.message)
    }
}

impl std::error::Error for PolicyRefusal {}

/// The stop an entry is sized against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryStop {
    /// The alert's own stop price. Sizing still refuses it when it lies on
    /// the wrong side of the entry.
    Signal(Price),
    /// The policy's fallback: this far from the expected entry.
    Fallback(StopPercent),
}

/// The policy an entry was admitted under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SnapshotParts")]
pub struct PolicySnapshot {
    format: u32,
    profile: ProfileId,
    account: AccountRef,
    revision: PolicyRevision,
    instrument: InstrumentId,
    risk_percent: Sourced<RiskPercent>,
    stop_percent: Sourced<Option<StopPercent>>,
    leverage: Sourced<Leverage>,
    capital: Money,
    capital_revision: PolicyRevision,
}

impl PolicySnapshot {
    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn profile(&self) -> &ProfileId {
        &self.profile
    }

    /// The profile's account: the only one an entry admitted under this
    /// snapshot may trade on.
    pub fn account(&self) -> &AccountRef {
        &self.account
    }

    /// The revision of the profile's policy the snapshot was taken from.
    pub const fn revision(&self) -> PolicyRevision {
        self.revision
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub const fn risk_percent(&self) -> Sourced<RiskPercent> {
        self.risk_percent
    }

    /// The fallback stop the policy gives, if it gives one.
    pub const fn stop_percent(&self) -> Sourced<Option<StopPercent>> {
        self.stop_percent
    }

    pub const fn leverage(&self) -> Sourced<Leverage> {
        self.leverage
    }

    /// The capital the entry is sized from, in the instrument's settlement
    /// asset.
    pub fn capital(&self) -> &Money {
        &self.capital
    }

    /// The revision that set that capital.
    pub const fn capital_revision(&self) -> PolicyRevision {
        self.capital_revision
    }

    /// The stop the entry uses. The alert's own stop comes first and is
    /// never replaced by the fallback, even when sizing then refuses it;
    /// without one the policy's fallback percentage applies; without
    /// either the entry is refused.
    pub fn stop_for(&self, signal_stop: Option<Price>) -> Result<EntryStop, PolicyRefusal> {
        match (signal_stop, self.stop_percent.value) {
            (Some(price), _) => Ok(EntryStop::Signal(price)),
            (None, Some(percent)) => Ok(EntryStop::Fallback(percent)),
            (None, None) => Err(PolicyRefusal::new(
                Refusal::NoStop,
                format!(
                    "the alert brings no stop, and profile {} gives {} no fallback stop \
                     percentage",
                    self.profile, self.instrument.symbol
                ),
            )),
        }
    }

    fn fault(&self) -> Option<String> {
        if !self.instrument.category().is_futures() {
            return Some("a spot instrument has no policy".to_string());
        }
        if self.instrument.venue() != self.account.venue() {
            return Some(format!(
                "{} trades on {}, and its account is on {}",
                self.instrument.symbol,
                self.instrument.venue(),
                self.account.venue()
            ));
        }
        if self.capital.asset() != &self.instrument.settle {
            return Some(format!(
                "its capital is in {}, and {} settles in {}",
                self.capital.asset(),
                self.instrument.symbol,
                self.instrument.settle
            ));
        }
        if self.capital.amount() <= Decimal::ZERO {
            return Some("its capital is not above zero".to_string());
        }
        (self.capital_revision > self.revision).then(|| {
            format!(
                "its capital is of revision {}, later than its own revision {}",
                self.capital_revision, self.revision
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotParts {
    format: u32,
    profile: ProfileId,
    account: AccountRef,
    revision: PolicyRevision,
    instrument: InstrumentId,
    risk_percent: Sourced<RiskPercent>,
    stop_percent: Sourced<Option<StopPercent>>,
    leverage: Sourced<Leverage>,
    capital: Money,
    capital_revision: PolicyRevision,
}

impl TryFrom<SnapshotParts> for PolicySnapshot {
    type Error = ModelError;

    fn try_from(parts: SnapshotParts) -> Result<Self, Self::Error> {
        if parts.format != POLICY_SNAPSHOT_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: "policy snapshot",
                found: parts.format,
                expected: POLICY_SNAPSHOT_FORMAT,
            });
        }
        let snapshot = Self {
            format: parts.format,
            profile: parts.profile,
            account: parts.account,
            revision: parts.revision,
            instrument: parts.instrument,
            risk_percent: parts.risk_percent,
            stop_percent: parts.stop_percent,
            leverage: parts.leverage,
            capital: parts.capital,
            capital_revision: parts.capital_revision,
        };
        match snapshot.fault() {
            Some(reason) => Err(invalid("policy snapshot", reason)),
            None => Ok(snapshot),
        }
    }
}

impl ProfilePolicy {
    /// The policy an entry on `instrument` is admitted under, or why this
    /// policy refuses the entry. The instrument is the venue's own listing,
    /// read now: its product line and its settlement asset are checked
    /// against the profile's every time.
    ///
    /// Only entries are held to this. An exit, a stop repair or a
    /// protective close takes no policy, so disabling an asset, or the
    /// defaults, never strands a position that is open.
    ///
    /// The leverage is the operator's setting, as it stands. Whether the
    /// venue's brackets take it for this instrument, account and size is
    /// sizing's to say (ADR 0002 §6), which refuses what they do not take
    /// as `above_leverage_bracket`.
    pub fn entry_snapshot(
        &self,
        instrument: &FuturesInstrument,
    ) -> Result<PolicySnapshot, PolicyRefusal> {
        let id = instrument.id();
        let profile = self.identity.id();
        let symbol = &id.symbol;
        let effective = self
            .resolve(id)
            .map_err(|reason| PolicyRefusal::new(Refusal::AmbiguousSelection, reason))?;
        if !effective.enabled {
            return Err(if effective.configured {
                PolicyRefusal::new(
                    Refusal::NotArmed,
                    format!(
                        "{symbol} is disabled in profile {profile}; enable the asset to take \
                         entries on it"
                    ),
                )
            } else {
                PolicyRefusal::new(
                    Refusal::UnknownTicker,
                    format!(
                        "profile {profile} has no settings for {symbol}, and its defaults are \
                         not enabled"
                    ),
                )
            });
        }
        if let Err(outside) = instrument.universe() {
            return Err(PolicyRefusal::new(
                outside.refusal(),
                format!("{symbol}: {outside}"),
            ));
        }
        let Some(capital) = effective.capital else {
            return Err(PolicyRefusal::new(
                Refusal::NoCapitalInSettlementAsset,
                format!(
                    "profile {profile} has no capital in {}, which {symbol} settles in",
                    id.settle
                ),
            ));
        };
        // A document never holds an enabled asset, or enabled defaults,
        // without both; a policy that did would enter nothing.
        let (Some(risk_percent), Some(leverage)) = (effective.risk_percent, effective.leverage)
        else {
            return Err(PolicyRefusal::new(
                Refusal::NotArmed,
                format!("profile {profile} gives {symbol} no risk percentage or no leverage"),
            ));
        };
        Ok(PolicySnapshot {
            format: POLICY_SNAPSHOT_FORMAT,
            profile: profile.clone(),
            account: self.identity.account_ref().clone(),
            revision: self.revision,
            instrument: id.clone(),
            risk_percent,
            stop_percent: effective.stop_percent,
            leverage,
            capital: capital.amount().clone(),
            capital_revision: capital.revision(),
        })
    }
}
