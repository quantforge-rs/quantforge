//! A policy held against the venue's own instrument list, before its
//! profile may execute: every asset that could take an entry must be a
//! contract the venue lists on the profile's product line, inside the
//! universe, settled in the asset the profile holds it under, with a
//! leverage the venue accepts and capital to size from.

use super::document::{AssetPolicy, ProfilePolicy};
use super::line_name;
use crate::model::futures::{FuturesInstrument, LeverageFilter};
use crate::model::{Leverage, Symbol};
use rust_decimal::Decimal;
use std::fmt;

/// What a problem is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicySubject {
    /// The profile's defaults.
    Defaults,
    /// One asset's settings.
    Asset(Symbol),
}

impl fmt::Display for PolicySubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Defaults => f.write_str("defaults"),
            Self::Asset(symbol) => write!(f, "{symbol}"),
        }
    }
}

/// What kind of problem the venue's list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolicyProblemKind {
    /// The venue lists no such contract on the profile's product line.
    NotListed,
    /// The venue lists it settled in another asset than the profile holds
    /// it under.
    SettlementChanged,
    /// Universe rule 1 leaves it out.
    OutsideUniverse,
    /// The leverage is outside what the venue accepts for it.
    LeverageOutOfRange,
    /// The leverage is not on the venue's leverage step.
    LeverageOffStep,
    /// No capital is set in the asset it settles in.
    NoCapital,
}

impl PolicyProblemKind {
    /// Every kind, in the order an asset is looked at: the first that
    /// applies is the asset's problem.
    pub const ALL: [Self; 6] = [
        Self::NotListed,
        Self::SettlementChanged,
        Self::OutsideUniverse,
        Self::LeverageOutOfRange,
        Self::LeverageOffStep,
        Self::NoCapital,
    ];

    /// The kind's name as reports print it, such as `not_listed`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotListed => "not_listed",
            Self::SettlementChanged => "settlement_changed",
            Self::OutsideUniverse => "outside_universe",
            Self::LeverageOutOfRange => "leverage_out_of_range",
            Self::LeverageOffStep => "leverage_off_step",
            Self::NoCapital => "no_capital",
        }
    }
}

/// One reason a profile cannot execute as its policy stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyProblem {
    pub subject: PolicySubject,
    pub kind: PolicyProblemKind,
    pub message: String,
}

impl fmt::Display for PolicyProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.message)
    }
}

/// Why the venue does not accept `leverage` under `filter`, if it does not.
fn leverage_fault(leverage: Leverage, filter: &LeverageFilter) -> Option<PolicyProblemKind> {
    if leverage < filter.min || leverage > filter.max {
        return Some(PolicyProblemKind::LeverageOutOfRange);
    }
    let above = leverage.value() - filter.min.value();
    let on_step = above
        .checked_rem(filter.step)
        .is_some_and(|rest| rest == Decimal::ZERO);
    (!on_step).then_some(PolicyProblemKind::LeverageOffStep)
}

impl ProfilePolicy {
    /// Every reason the profile cannot execute as this policy stands, by
    /// the venue's `instruments`: the profile's product line as its adapter
    /// lists it now. Only what could take an entry is held to the list: the
    /// enabled assets, in the order of their symbols, and the defaults
    /// while they are enabled. An empty answer means every such entry
    /// would find its instrument, its leverage and its capital.
    pub fn check(&self, instruments: &[FuturesInstrument]) -> Vec<PolicyProblem> {
        let line = self.exchange();
        let listed = |symbol: &Symbol| {
            instruments
                .iter()
                .find(|listed| listed.id().exchange == line && &listed.id().symbol == symbol)
        };
        let mut problems: Vec<PolicyProblem> = self
            .assets
            .iter()
            .filter(|asset| asset.enabled)
            .filter_map(|asset| {
                let (kind, message) = self.asset_fault(asset, listed(asset.symbol()))?;
                Some(PolicyProblem {
                    subject: PolicySubject::Asset(asset.symbol().clone()),
                    kind,
                    message,
                })
            })
            .collect();
        if self.defaults.enabled && self.capital.is_empty() {
            problems.push(PolicyProblem {
                subject: PolicySubject::Defaults,
                kind: PolicyProblemKind::NoCapital,
                message: "the defaults are enabled, and the profile has no capital in any asset"
                    .to_string(),
            });
        }
        problems
    }

    /// The first thing that keeps an enabled asset from taking an entry.
    fn asset_fault(
        &self,
        asset: &AssetPolicy,
        listed: Option<&FuturesInstrument>,
    ) -> Option<(PolicyProblemKind, String)> {
        let line = self.exchange();
        let symbol = asset.symbol();
        let Some(listed) = listed else {
            return Some((
                PolicyProblemKind::NotListed,
                format!("the venue lists no {} contract {symbol}", line_name(line)),
            ));
        };
        let settle = &listed.id().settle;
        if settle != &asset.instrument.settle {
            return Some((
                PolicyProblemKind::SettlementChanged,
                format!(
                    "the venue lists it settled in {settle}, and the profile holds it as \
                     settled in {}",
                    asset.instrument.settle
                ),
            ));
        }
        if let Err(outside) = listed.universe() {
            return Some((PolicyProblemKind::OutsideUniverse, outside.to_string()));
        }
        let leverage = self.leverage_of(asset);
        let filter = listed.spec().leverage.as_ref();
        if let (Some(leverage), Some(filter)) = (leverage, filter) {
            if let Some(kind) = leverage_fault(leverage, filter) {
                return Some((
                    kind,
                    format!(
                        "the venue takes a leverage from {} to {} in steps of {}, not {leverage}",
                        filter.min, filter.max, filter.step
                    ),
                ));
            }
        }
        self.capital_in(settle).is_none().then(|| {
            (
                PolicyProblemKind::NoCapital,
                format!("the profile has no capital in {settle}, which it settles in"),
            )
        })
    }
}
