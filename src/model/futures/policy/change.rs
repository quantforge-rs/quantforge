//! The changes an operator makes to a profile's policy. Each is applied to
//! one revision and makes the next: nothing is changed in place, so a
//! revision an entry was admitted under stays what it was, and an edit
//! never reaches a position that is already open.

use super::document::{AssetPolicy, Capital, ProfilePolicy, StopFallback};
use super::{ProfileLabel, RiskPercent, StopPercent, invalid, line_name};
use crate::model::{Asset, InstrumentId, Leverage, ModelError, Money, Symbol};
use rust_decimal::Decimal;

/// What a change does to one optional setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edit<T> {
    /// Leaves it as it is.
    #[default]
    Keep,
    /// Sets it.
    Set(T),
    /// Removes it: a default is then unset, and an asset's own setting
    /// goes back to the defaults'.
    Clear,
}

impl<T: Copy> Edit<T> {
    fn apply_to(self, setting: &mut Option<T>) {
        match self {
            Self::Keep => {}
            Self::Set(value) => *setting = Some(value),
            Self::Clear => *setting = None,
        }
    }
}

/// A change to the defaults. A field left as it is keeps its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefaultsEdit {
    /// Whether assets with no settings of their own trade on the defaults.
    pub enabled: Option<bool>,
    pub risk_percent: Edit<RiskPercent>,
    pub stop_percent: Edit<StopPercent>,
    pub leverage: Edit<Leverage>,
}

/// An asset given settings of its own. A setting left out is the defaults'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAsset {
    /// The instrument as the venue lists it, settlement asset included.
    pub instrument: InstrumentId,
    pub enabled: bool,
    pub risk_percent: Option<RiskPercent>,
    pub stop: StopFallback,
    pub leverage: Option<Leverage>,
}

/// A change to an asset's own settings. A field left as it is keeps its
/// value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetEdit {
    pub enabled: Option<bool>,
    pub risk_percent: Edit<RiskPercent>,
    pub stop: Option<StopFallback>,
    pub leverage: Edit<Leverage>,
}

/// One change to a profile's policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyChange {
    /// Names the profile, or removes its name.
    Label(Option<ProfileLabel>),
    /// Changes the defaults.
    Defaults(DefaultsEdit),
    /// Gives an asset settings of its own.
    AddAsset(NewAsset),
    /// Changes an asset's own settings.
    EditAsset { symbol: Symbol, edit: AssetEdit },
    /// Puts every setting of an asset back on the defaults. The asset
    /// stays, enabled or disabled as it was.
    ResetAsset { symbol: Symbol },
    /// Sets the capital that entries settled in the amount's asset are
    /// sized from.
    SetCapital(Money),
    /// Removes the capital in an asset: entries settled in it are refused
    /// from then on.
    ClearCapital(Asset),
}

impl ProfilePolicy {
    /// The policy after `change`, one revision on, or `None` when the
    /// policy already is what the change asks for. A change that would
    /// leave an enabled asset, or enabled defaults, without a risk
    /// percentage or a leverage is refused, as is one that names an asset
    /// the profile cannot hold.
    pub fn apply(&self, change: &PolicyChange) -> Result<Option<Self>, ModelError> {
        let refuse = |reason: String| invalid("policy change", reason);
        let mut next = self.clone();
        match change {
            PolicyChange::Label(label) => next.label = label.clone(),
            PolicyChange::Defaults(edit) => {
                let defaults = &mut next.defaults;
                defaults.enabled = edit.enabled.unwrap_or(defaults.enabled);
                edit.risk_percent.apply_to(&mut defaults.risk_percent);
                edit.stop_percent.apply_to(&mut defaults.stop_percent);
                edit.leverage.apply_to(&mut defaults.leverage);
            }
            PolicyChange::AddAsset(new) => next.add(new).map_err(refuse)?,
            PolicyChange::EditAsset { symbol, edit } => {
                let asset = next.own(symbol).map_err(refuse)?;
                asset.enabled = edit.enabled.unwrap_or(asset.enabled);
                edit.risk_percent.apply_to(&mut asset.risk_percent);
                asset.stop = edit.stop.unwrap_or(asset.stop);
                edit.leverage.apply_to(&mut asset.leverage);
            }
            PolicyChange::ResetAsset { symbol } => {
                let asset = next.own(symbol).map_err(refuse)?;
                asset.risk_percent = None;
                asset.stop = StopFallback::Inherit;
                asset.leverage = None;
            }
            PolicyChange::SetCapital(amount) => next.set_capital(amount).map_err(refuse)?,
            PolicyChange::ClearCapital(asset) => {
                next.capital.retain(|capital| capital.asset() != asset);
            }
        }
        if next == *self {
            return Ok(None);
        }
        next.stamp(self)?;
        match next.fault() {
            Some(reason) => Err(refuse(reason)),
            None => Ok(Some(next)),
        }
    }

    fn add(&mut self, new: &NewAsset) -> Result<(), String> {
        let line = self.exchange();
        let symbol = &new.instrument.symbol;
        if new.instrument.exchange != line {
            return Err(format!(
                "{symbol} is a {} contract, and profile {} trades {}",
                line_name(new.instrument.exchange),
                self.identity.id(),
                line_name(line)
            ));
        }
        if self.asset(symbol).is_some() {
            return Err(format!(
                "asset {symbol} already has settings of its own; edit them"
            ));
        }
        let at = self
            .assets
            .partition_point(|asset| asset.symbol().as_str() < symbol.as_str());
        self.assets.insert(
            at,
            AssetPolicy {
                instrument: new.instrument.clone(),
                enabled: new.enabled,
                risk_percent: new.risk_percent,
                stop: new.stop,
                leverage: new.leverage,
                // Stamped with the others once the change is whole.
                revision: self.revision,
            },
        );
        Ok(())
    }

    fn own(&mut self, symbol: &Symbol) -> Result<&mut AssetPolicy, String> {
        let profile = self.identity.id().clone();
        self.assets
            .iter_mut()
            .find(|asset| asset.symbol() == symbol)
            .ok_or_else(|| {
                format!("profile {profile} holds no settings for {symbol}; add the asset first")
            })
    }

    fn set_capital(&mut self, amount: &Money) -> Result<(), String> {
        if amount.amount() <= Decimal::ZERO {
            return Err(format!(
                "capital of {} {} is not above zero",
                amount.amount(),
                amount.asset()
            ));
        }
        let at = self
            .capital
            .partition_point(|capital| capital.asset() < amount.asset());
        let entry = Capital {
            amount: amount.clone(),
            revision: self.revision,
        };
        match self.capital.get_mut(at) {
            // The same amount written with another scale is the same
            // capital: nothing changes, and no revision is made of it.
            Some(held) if held.asset() == amount.asset() => {
                if held.amount.amount() != amount.amount() {
                    *held = entry;
                }
            }
            _ => self.capital.insert(at, entry),
        }
        Ok(())
    }

    /// Makes this the revision after `before`, stamping it on every part
    /// that differs from `before`'s.
    fn stamp(&mut self, before: &Self) -> Result<(), ModelError> {
        let revision = before.revision.next()?;
        self.revision = revision;
        if self.defaults != before.defaults {
            self.defaults.revision = revision;
        }
        for asset in &mut self.assets {
            if before.asset(asset.symbol()) != Some(&*asset) {
                asset.revision = revision;
            }
        }
        for capital in &mut self.capital {
            if before.capital_in(capital.asset()) != Some(&*capital) {
                capital.revision = revision;
            }
        }
        Ok(())
    }
}
