//! A profile's policy as a store keeps it: one document for each revision,
//! complete in itself, checked whenever it is built or read. Whatever a
//! document holds could trade: an enabled asset always has a risk
//! percentage and a leverage, its own or the defaults'.

use super::{PolicyRevision, ProfileLabel, RiskPercent, StopPercent, invalid, line_name};
use crate::model::futures::ProfileIdentity;
use crate::model::{Asset, ExchangeId, InstrumentId, Leverage, ModelError, Money, Symbol};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// The version of [`ProfilePolicy`]'s stored form. A document of another
/// version is refused, never guessed at.
pub const POLICY_FORMAT: u32 = 1;

/// What an asset does for a stop when the alert brings none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopFallback {
    /// As the profile's defaults say.
    Inherit,
    /// Nothing: an entry without a stop of its own is refused, whatever
    /// the defaults say.
    Off,
    /// A stop this far from the expected entry.
    Percent(StopPercent),
}

/// The settings an asset trades on where it has none of its own. They
/// enable nothing by themselves: an asset without settings trades on them
/// only while `enabled` is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDefaults {
    pub(super) enabled: bool,
    pub(super) risk_percent: Option<RiskPercent>,
    pub(super) stop_percent: Option<StopPercent>,
    pub(super) leverage: Option<Leverage>,
    pub(super) revision: PolicyRevision,
}

impl PolicyDefaults {
    /// Whether an asset with no settings of its own trades on the defaults.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn risk_percent(&self) -> Option<RiskPercent> {
        self.risk_percent
    }

    /// The fallback stop, or `None` when an entry must bring its own.
    pub const fn stop_percent(&self) -> Option<StopPercent> {
        self.stop_percent
    }

    pub const fn leverage(&self) -> Option<Leverage> {
        self.leverage
    }

    /// The revision that last changed the defaults.
    pub const fn revision(&self) -> PolicyRevision {
        self.revision
    }
}

/// An asset's own settings. A setting left out is the defaults'.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetPolicy {
    pub(super) instrument: InstrumentId,
    pub(super) enabled: bool,
    pub(super) risk_percent: Option<RiskPercent>,
    pub(super) stop: StopFallback,
    pub(super) leverage: Option<Leverage>,
    pub(super) revision: PolicyRevision,
}

impl AssetPolicy {
    /// The instrument, with the settlement asset the venue listed it under
    /// when it was added.
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub fn symbol(&self) -> &Symbol {
        &self.instrument.symbol
    }

    /// Whether new entries on the asset are allowed. A disabled asset keeps
    /// its monitoring, its protection and its exits.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// The asset's own risk percentage, or `None` where it takes the
    /// defaults'.
    pub const fn risk_percent(&self) -> Option<RiskPercent> {
        self.risk_percent
    }

    pub const fn stop(&self) -> StopFallback {
        self.stop
    }

    /// The asset's own leverage, or `None` where it takes the defaults'.
    pub const fn leverage(&self) -> Option<Leverage> {
        self.leverage
    }

    /// The revision that last changed the asset's own settings.
    pub const fn revision(&self) -> PolicyRevision {
        self.revision
    }
}

/// The capital a profile sizes from in one settlement asset: a setting,
/// never the account's balance, and never converted into another asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capital {
    pub(super) amount: Money,
    pub(super) revision: PolicyRevision,
}

impl Capital {
    pub fn amount(&self) -> &Money {
        &self.amount
    }

    pub fn asset(&self) -> &Asset {
        self.amount.asset()
    }

    /// The revision that set this amount.
    pub const fn revision(&self) -> PolicyRevision {
        self.revision
    }
}

/// One revision of a profile's policy: who the profile is, its defaults,
/// the assets with settings of their own, and its capital.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PolicyParts")]
pub struct ProfilePolicy {
    pub(super) format: u32,
    pub(super) identity: ProfileIdentity,
    pub(super) revision: PolicyRevision,
    pub(super) label: Option<ProfileLabel>,
    pub(super) defaults: PolicyDefaults,
    /// In the byte order of their symbols.
    pub(super) assets: Vec<AssetPolicy>,
    /// In the byte order of their assets' codes.
    pub(super) capital: Vec<Capital>,
}

impl ProfilePolicy {
    /// The policy a profile is created with, revision 1: no settings, no
    /// capital, nothing enabled.
    pub fn new(identity: ProfileIdentity) -> Self {
        Self {
            format: POLICY_FORMAT,
            identity,
            revision: PolicyRevision::FIRST,
            label: None,
            defaults: PolicyDefaults {
                enabled: false,
                risk_percent: None,
                stop_percent: None,
                leverage: None,
                revision: PolicyRevision::FIRST,
            },
            assets: Vec::new(),
            capital: Vec::new(),
        }
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn identity(&self) -> &ProfileIdentity {
        &self.identity
    }

    pub const fn revision(&self) -> PolicyRevision {
        self.revision
    }

    pub fn label(&self) -> Option<&ProfileLabel> {
        self.label.as_ref()
    }

    pub fn defaults(&self) -> &PolicyDefaults {
        &self.defaults
    }

    /// The assets with settings of their own, in the byte order of their
    /// symbols.
    pub fn assets(&self) -> &[AssetPolicy] {
        &self.assets
    }

    /// The asset's own settings, if it has any.
    pub fn asset(&self, symbol: &Symbol) -> Option<&AssetPolicy> {
        self.assets.iter().find(|asset| asset.symbol() == symbol)
    }

    /// The capital in every settlement asset, in the byte order of their
    /// codes.
    pub fn capital(&self) -> &[Capital] {
        &self.capital
    }

    /// The capital entries settled in `asset` are sized from.
    pub fn capital_in(&self, asset: &Asset) -> Option<&Capital> {
        self.capital.iter().find(|capital| capital.asset() == asset)
    }

    /// The product line the profile trades.
    pub fn exchange(&self) -> ExchangeId {
        ExchangeId::from_parts(
            self.identity.account_ref().venue(),
            self.identity.category(),
        )
    }

    /// The document as a store keeps it: JSON in one fixed spelling, every
    /// decimal as exact text with its scale.
    pub fn to_stored(&self) -> Result<String, ModelError> {
        serde_json::to_string(self).map_err(|error| invalid("policy", error.to_string()))
    }

    /// Reads a stored document back, with every check a new one passes.
    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        let parts: PolicyParts =
            serde_json::from_str(text).map_err(|error| invalid("policy", error.to_string()))?;
        Self::try_from(parts)
    }

    /// The risk percentage `asset` trades on: its own, or the defaults'.
    pub(super) fn risk_of(&self, asset: &AssetPolicy) -> Option<RiskPercent> {
        asset.risk_percent.or(self.defaults.risk_percent)
    }

    /// The leverage `asset` trades on: its own, or the defaults'.
    pub(super) fn leverage_of(&self, asset: &AssetPolicy) -> Option<Leverage> {
        asset.leverage.or(self.defaults.leverage)
    }

    /// Why the document is not one a store may keep, if it is not.
    pub(super) fn fault(&self) -> Option<String> {
        let line = self.exchange();
        let stamps = [self.defaults.revision]
            .into_iter()
            .chain(self.assets.iter().map(|asset| asset.revision))
            .chain(self.capital.iter().map(|capital| capital.revision));
        if let Some(later) = stamps.into_iter().find(|stamp| *stamp > self.revision) {
            return Some(format!(
                "a part of it is stamped revision {later}, later than its own revision {}",
                self.revision
            ));
        }
        if self.defaults.enabled
            && (self.defaults.risk_percent.is_none() || self.defaults.leverage.is_none())
        {
            return Some(
                "the defaults are enabled without both a risk percentage and a leverage; set \
                 them, or disable the defaults"
                    .to_string(),
            );
        }
        for pair in self.assets.windows(2) {
            if pair[0].symbol().as_str() >= pair[1].symbol().as_str() {
                return Some(format!(
                    "asset {} is listed twice or out of order",
                    pair[1].symbol()
                ));
            }
        }
        for asset in &self.assets {
            if asset.instrument.exchange != line {
                return Some(format!(
                    "asset {} is a {} contract, and the profile trades {}",
                    asset.symbol(),
                    line_name(asset.instrument.exchange),
                    line_name(line)
                ));
            }
            let missing = match (self.risk_of(asset), self.leverage_of(asset)) {
                (None, _) => "a risk percentage",
                (_, None) => "a leverage",
                _ => continue,
            };
            if asset.enabled {
                return Some(format!(
                    "asset {} is enabled without {missing}, its own or the defaults'; set one, \
                     or disable the asset",
                    asset.symbol()
                ));
            }
        }
        for pair in self.capital.windows(2) {
            if pair[0].asset() >= pair[1].asset() {
                return Some(format!(
                    "capital in {} is listed twice or out of order",
                    pair[1].asset()
                ));
            }
        }
        self.capital
            .iter()
            .find(|capital| capital.amount.amount() <= Decimal::ZERO)
            .map(|capital| {
                format!(
                    "capital of {} {} is not above zero",
                    capital.amount.amount(),
                    capital.asset()
                )
            })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyParts {
    format: u32,
    identity: ProfileIdentity,
    revision: PolicyRevision,
    label: Option<ProfileLabel>,
    defaults: PolicyDefaults,
    assets: Vec<AssetPolicy>,
    capital: Vec<Capital>,
}

impl TryFrom<PolicyParts> for ProfilePolicy {
    type Error = ModelError;

    fn try_from(parts: PolicyParts) -> Result<Self, Self::Error> {
        if parts.format != POLICY_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: "policy",
                found: parts.format,
                expected: POLICY_FORMAT,
            });
        }
        let policy = Self {
            format: parts.format,
            identity: parts.identity,
            revision: parts.revision,
            label: parts.label,
            defaults: parts.defaults,
            assets: parts.assets,
            capital: parts.capital,
        };
        match policy.fault() {
            Some(reason) => Err(invalid("policy", reason)),
            None => Ok(policy),
        }
    }
}
