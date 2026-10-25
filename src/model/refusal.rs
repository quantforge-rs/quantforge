//! Why a request stops short of a venue, or never reaches one: the refusal
//! vocabulary the parity matrix freezes. Spot, configuration and automation
//! all refuse in these terms, each with a message that says what to change.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A reason QuantForge refuses a request before, or instead of, sending it
/// to a venue. The names are the parity matrix's, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Refusal {
    /// No capital is configured in the instrument's settlement asset.
    NoCapitalInSettlementAsset,
    /// The stop is on the wrong side of the entry.
    StopWrongSide,
    /// Neither the alert nor the asset's settings give a stop.
    NoStop,
    /// The quantity is below the instrument's minimum.
    BelowMinQty,
    /// The quantity is above the market-order maximum.
    AboveMarketMax,
    /// The notional is below the instrument's minimum.
    BelowMinNotional,
    /// The position would exceed the cap of its leverage bracket.
    AboveLeverageBracket,
    /// The margin does not fit the venue's pool, net of reservations.
    InsufficientMargin,
    /// The settlement wallet holds less than the risk budget.
    SettlementWalletBelowRisk,
    /// The mark or touch price is already at or past the stop.
    StopCrossed,
    /// The risk at the expected fill exceeds the drift limit.
    RiskDrift,
    /// The alert is older than the profile's age limit.
    Stale,
    /// The alert is timed too far in the future.
    FutureTimestamp,
    /// The profile is not armed for entries.
    NotArmed,
    /// The same signal ID arrived with a different payload.
    Conflict,
    /// An entry on the side of a position already open.
    Pyramiding,
    /// An entry on the side opposite an open position.
    Reversal,
    /// A position QuantForge does not own is open on the instrument.
    UnownedPosition,
    /// An earlier entry on the position is still unresolved.
    UnresolvedEntry,
    /// An open incident halts entries.
    IncidentOpen,
    /// The account is in hedge mode; one-way mode is required.
    HedgeMode,
    /// The account uses portfolio margin.
    PortfolioMargin,
    /// The account type is not supported.
    UnsupportedAccount,
    /// The venue, account, environment or product is ambiguous.
    AmbiguousSelection,
    /// A connector or profile is for another environment than the store.
    EnvironmentMismatch,
    /// The ticker resolves to no asset of the profile.
    UnknownTicker,
    /// The instrument is outside the product universe.
    OutsideUniverse,
}

impl Refusal {
    /// Every refusal, in the parity matrix's order.
    pub const ALL: [Self; 27] = [
        Self::NoCapitalInSettlementAsset,
        Self::StopWrongSide,
        Self::NoStop,
        Self::BelowMinQty,
        Self::AboveMarketMax,
        Self::BelowMinNotional,
        Self::AboveLeverageBracket,
        Self::InsufficientMargin,
        Self::SettlementWalletBelowRisk,
        Self::StopCrossed,
        Self::RiskDrift,
        Self::Stale,
        Self::FutureTimestamp,
        Self::NotArmed,
        Self::Conflict,
        Self::Pyramiding,
        Self::Reversal,
        Self::UnownedPosition,
        Self::UnresolvedEntry,
        Self::IncidentOpen,
        Self::HedgeMode,
        Self::PortfolioMargin,
        Self::UnsupportedAccount,
        Self::AmbiguousSelection,
        Self::EnvironmentMismatch,
        Self::UnknownTicker,
        Self::OutsideUniverse,
    ];

    /// The refusal's name as reports print it, such as `stale`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoCapitalInSettlementAsset => "no_capital_in_settlement_asset",
            Self::StopWrongSide => "stop_wrong_side",
            Self::NoStop => "no_stop",
            Self::BelowMinQty => "below_min_qty",
            Self::AboveMarketMax => "above_market_max",
            Self::BelowMinNotional => "below_min_notional",
            Self::AboveLeverageBracket => "above_leverage_bracket",
            Self::InsufficientMargin => "insufficient_margin",
            Self::SettlementWalletBelowRisk => "settlement_wallet_below_risk",
            Self::StopCrossed => "stop_crossed",
            Self::RiskDrift => "risk_drift",
            Self::Stale => "stale",
            Self::FutureTimestamp => "future_timestamp",
            Self::NotArmed => "not_armed",
            Self::Conflict => "conflict",
            Self::Pyramiding => "pyramiding",
            Self::Reversal => "reversal",
            Self::UnownedPosition => "unowned_position",
            Self::UnresolvedEntry => "unresolved_entry",
            Self::IncidentOpen => "incident_open",
            Self::HedgeMode => "hedge_mode",
            Self::PortfolioMargin => "portfolio_margin",
            Self::UnsupportedAccount => "unsupported_account",
            Self::AmbiguousSelection => "ambiguous_selection",
            Self::EnvironmentMismatch => "environment_mismatch",
            Self::UnknownTicker => "unknown_ticker",
            Self::OutsideUniverse => "outside_universe",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_refusal_has_one_distinct_snake_case_name() {
        let names: HashSet<&str> = Refusal::ALL
            .iter()
            .map(|refusal| refusal.as_str())
            .collect();
        assert_eq!(names.len(), Refusal::ALL.len());
        for name in names {
            assert!(
                name.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{name}"
            );
        }
    }
}
