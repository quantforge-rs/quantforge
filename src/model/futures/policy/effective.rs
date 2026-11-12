//! What an instrument trades on once the defaults are applied: each
//! setting with where it came from, so an operator sees which edit would
//! change it.

use super::document::{Capital, ProfilePolicy, StopFallback};
use super::{PolicyRevision, RiskPercent, StopPercent, invalid, line_name};
use crate::model::{InstrumentId, Leverage, ModelError};
use serde::{Deserialize, Serialize};

/// Where a setting came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SettingSource {
    /// The asset's own settings.
    Asset,
    /// The profile's defaults.
    Defaults,
}

impl SettingSource {
    /// `asset` or `defaults`, as reports print it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asset => "asset",
            Self::Defaults => "defaults",
        }
    }
}

/// A setting with where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub source: SettingSource,
}

impl<T> Sourced<T> {
    const fn asset(value: T) -> Self {
        Self {
            value,
            source: SettingSource::Asset,
        }
    }

    const fn defaults(value: T) -> Self {
        Self {
            value,
            source: SettingSource::Defaults,
        }
    }

    /// The asset's own setting where it has one, otherwise the defaults'.
    fn pick(own: Option<T>, default: Option<T>) -> Option<Self> {
        own.map(Self::asset).or_else(|| default.map(Self::defaults))
    }
}

/// The settings one instrument trades on in a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectivePolicy {
    pub instrument: InstrumentId,
    /// The revision of the policy this was read from.
    pub revision: PolicyRevision,
    /// Whether the asset has settings of its own; otherwise everything
    /// here is the defaults'.
    pub configured: bool,
    /// Whether new entries are allowed: the asset's own switch, or the
    /// defaults' where it has no settings of its own.
    pub enabled: bool,
    pub risk_percent: Option<Sourced<RiskPercent>>,
    /// The fallback stop. No value from the asset means it turned the
    /// fallback off; none from the defaults means they give none.
    pub stop_percent: Sourced<Option<StopPercent>>,
    pub leverage: Option<Sourced<Leverage>>,
    /// The capital in the instrument's settlement asset, if any is set.
    pub capital: Option<Capital>,
}

impl ProfilePolicy {
    /// What `instrument` trades on in this policy. Refuses an instrument on
    /// another product line than the profile's, and one whose settlement
    /// asset is not the one the profile holds it under.
    pub fn effective(&self, instrument: &InstrumentId) -> Result<EffectivePolicy, ModelError> {
        self.resolve(instrument)
            .map_err(|reason| invalid("policy", reason))
    }

    /// [`ProfilePolicy::effective`], or the reason alone why the profile
    /// cannot say.
    pub(super) fn resolve(&self, instrument: &InstrumentId) -> Result<EffectivePolicy, String> {
        let line = self.exchange();
        if instrument.exchange != line {
            return Err(format!(
                "{} is a {} contract, and profile {} trades {}",
                instrument.symbol,
                line_name(instrument.exchange),
                self.identity.id(),
                line_name(line)
            ));
        }
        let own = self.asset(&instrument.symbol);
        if let Some(held) = own.filter(|asset| asset.instrument.settle != instrument.settle) {
            return Err(format!(
                "profile {} holds {} as settled in {}, and the venue lists it settled in {}",
                self.identity.id(),
                instrument.symbol,
                held.instrument.settle,
                instrument.settle
            ));
        }
        let defaults = &self.defaults;
        let inherited_stop = Sourced::defaults(defaults.stop_percent);
        Ok(EffectivePolicy {
            instrument: instrument.clone(),
            revision: self.revision,
            configured: own.is_some(),
            enabled: own.map_or(defaults.enabled, |asset| asset.enabled),
            risk_percent: Sourced::pick(
                own.and_then(|asset| asset.risk_percent),
                defaults.risk_percent,
            ),
            stop_percent: match own.map(|asset| asset.stop) {
                None | Some(StopFallback::Inherit) => inherited_stop,
                Some(StopFallback::Off) => Sourced::asset(None),
                Some(StopFallback::Percent(percent)) => Sourced::asset(Some(percent)),
            },
            leverage: Sourced::pick(own.and_then(|asset| asset.leverage), defaults.leverage),
            capital: self.capital_in(&instrument.settle).cloned(),
        })
    }
}
