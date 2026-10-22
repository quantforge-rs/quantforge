//! A futures account and its positions as the venue reports them (ADR 0002
//! §4, §6, §7): account type, margin pools and wallets, each instrument's
//! settings, complete REST position snapshots and sparse stream updates,
//! and the venue's liquidation price or why it gave none.

use crate::model::units::decimal_text;
use crate::model::{
    AccountRef, Asset, InstrumentId, Leverage, Money, PositionSide, Price, Quantity, TimestampMs,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What kind of account the keys reach. Only `Standard` can arm.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountKind {
    /// A futures account v0.3 supports, such as a unified account 2.0.
    Standard,
    /// Portfolio margin, refused on both venues.
    PortfolioMargin,
    /// A unified account 1.0, which keeps inverse contracts apart; refused.
    BybitUnifiedV1,
    /// Any other kind, with the venue's own word for it; refused.
    Other(String),
}

impl AccountKind {
    /// The kind's name as the parity matrix's refused modes spell it.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::PortfolioMargin => "portfolio_margin",
            Self::BybitUnifiedV1 => "bybit_unified_1_0",
            Self::Other(_) => "other",
        }
    }
}

/// How the venue holds positions: one per instrument, or one per side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PositionMode {
    /// One position per instrument: the only mode v0.3 trades.
    OneWay,
    /// A long and a short position per instrument; refused.
    Hedge,
}

impl PositionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneWay => "one-way",
            Self::Hedge => "hedge",
        }
    }
}

/// How a position's margin is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarginMode {
    Isolated,
    Cross,
}

impl MarginMode {
    pub const ALL: [Self; 2] = [Self::Isolated, Self::Cross];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Cross => "cross",
        }
    }
}

/// One asset's wallet: its balance, and what is free to use. Both in the
/// wallet's asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletBalance {
    pub balance: Money,
    pub available: Money,
}

/// Which collateral a margin pool holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolScope {
    /// One asset's own pool, such as USDT or a coin.
    Asset(Asset),
    /// The account's USD pool, under cross or multi-asset margin. Amounts
    /// convert into it at the venue's index price, with that price's time.
    AccountUsd,
}

/// Margin available in one of the venue's pools, before reservations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginPool {
    pub scope: PoolScope,
    pub available: Money,
}

/// The account as the venue reports it. Configured risk capital is a
/// setting and never comes from here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub account: AccountRef,
    pub kind: AccountKind,
    /// Whether multi-asset margin pools the account's collateral in USD,
    /// where the venue has that mode.
    pub multi_asset_margin: Option<bool>,
    pub wallets: Vec<WalletBalance>,
    pub pools: Vec<MarginPool>,
    pub observed_at_ms: TimestampMs,
}

/// An instrument's settings on the account. Both venues keep the margin
/// mode and leverage per instrument, and one keeps the position mode per
/// instrument as well.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentSettings {
    pub instrument: InstrumentId,
    pub position_mode: PositionMode,
    pub margin_mode: MarginMode,
    pub leverage: Leverage,
}

/// The account's fee rates on an instrument, as fractions: a taker rate of
/// 0.0005 is 0.05 %. A maker rebate is negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeRates {
    #[serde(with = "decimal_text")]
    pub maker: Decimal,
    #[serde(with = "decimal_text")]
    pub taker: Decimal,
}

/// One tier of a venue's leverage brackets: the largest position, in the
/// settlement asset, that this leverage allows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeverageBracket {
    pub max_leverage: Leverage,
    pub cap: Money,
}

/// What a position holds: its side, its size in the family's unit, and its
/// average entry price.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Exposure {
    pub side: PositionSide,
    pub size: Quantity,
    pub entry_price: Price,
}

/// Why a venue gave no liquidation price.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LiquidationGap {
    /// The venue reported 0, its way of saying there is none.
    ReportedZero,
    /// The venue left the field empty, as it does under portfolio margin
    /// or outside its price limits.
    ReportedEmpty,
}

/// The venue's own liquidation price, or why it gave none. It is never
/// estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Liquidation {
    Price(Price),
    Unavailable(LiquidationGap),
}

impl fmt::Display for Liquidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Price(price) => write!(f, "{price}"),
            Self::Unavailable(LiquidationGap::ReportedZero) => {
                f.write_str("N/A (the venue reports none)")
            }
            Self::Unavailable(LiquidationGap::ReportedEmpty) => {
                f.write_str("N/A (the venue left it empty)")
            }
        }
    }
}

/// A position as a REST snapshot reports it: complete, so a snapshot with
/// no exposure is the venue's evidence that the position is flat. Its stop
/// comes from the protection port, never from here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub instrument: InstrumentId,
    /// `None` when flat.
    pub exposure: Option<Exposure>,
    /// The position's value as the venue reports it.
    pub notional: Option<Money>,
    pub leverage: Leverage,
    pub margin_mode: MarginMode,
    pub mark_price: Option<Price>,
    pub liquidation: Liquidation,
    pub unrealized_pnl: Option<Money>,
    pub margin: Option<Money>,
    pub observed_at_ms: TimestampMs,
}

impl PositionSnapshot {
    /// Whether the snapshot shows no exposure.
    pub fn is_flat(&self) -> bool {
        self.exposure.is_none_or(|exposure| exposure.size.is_zero())
    }

    /// The side held, or `None` when flat.
    pub fn side(&self) -> Option<PositionSide> {
        self.exposure
            .filter(|exposure| !exposure.size.is_zero())
            .map(|exposure| exposure.side)
    }
}

/// A position as a private stream reports it: sparse. A field the event
/// left out is unknown, and a position missing from an event says nothing,
/// so an update is never evidence that a position is flat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub instrument: InstrumentId,
    pub side: Option<PositionSide>,
    pub size: Option<Quantity>,
    pub entry_price: Option<Price>,
    pub unrealized_pnl: Option<Money>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, ExchangeId, Symbol};

    fn snapshot(exposure: Option<Exposure>) -> PositionSnapshot {
        let instrument = InstrumentId::new(
            ExchangeId::BinanceLinear,
            Symbol::new("SOLUSDT").expect("symbol"),
            Asset::new("USDT").expect("asset"),
        );
        PositionSnapshot {
            instrument,
            exposure,
            notional: None,
            leverage: Leverage::new(Decimal::from(5)).expect("leverage"),
            margin_mode: MarginMode::Isolated,
            mark_price: None,
            liquidation: Liquidation::Unavailable(LiquidationGap::ReportedZero),
            unrealized_pnl: None,
            margin: None,
            observed_at_ms: 1,
        }
    }

    fn exposure(size: u32) -> Exposure {
        Exposure {
            side: PositionSide::Short,
            size: Quantity::for_category(Category::Linear, Decimal::from(size)).expect("size"),
            entry_price: Price::new(Decimal::from(200)).expect("price"),
        }
    }

    #[test]
    fn a_snapshot_is_flat_without_exposure_or_with_none_left() {
        assert!(snapshot(None).is_flat());
        assert!(snapshot(Some(exposure(0))).is_flat());
        assert_eq!(snapshot(Some(exposure(0))).side(), None);
        assert!(!snapshot(Some(exposure(4))).is_flat());
        assert_eq!(
            snapshot(Some(exposure(4))).side(),
            Some(PositionSide::Short)
        );
    }

    #[test]
    fn a_missing_liquidation_shows_as_na_with_its_reason() {
        let zero = Liquidation::Unavailable(LiquidationGap::ReportedZero);
        assert_eq!(zero.to_string(), "N/A (the venue reports none)");
        let empty = Liquidation::Unavailable(LiquidationGap::ReportedEmpty);
        assert_eq!(empty.to_string(), "N/A (the venue left it empty)");
    }

    #[test]
    fn modes_and_account_kinds_use_the_policys_names() {
        assert_eq!(PositionMode::OneWay.as_str(), "one-way");
        assert_eq!(PositionMode::Hedge.as_str(), "hedge");
        let margins: Vec<&str> = MarginMode::ALL.iter().map(|mode| mode.as_str()).collect();
        assert_eq!(margins, ["isolated", "cross"]);
        assert_eq!(AccountKind::BybitUnifiedV1.as_str(), "bybit_unified_1_0");
        assert_eq!(AccountKind::Other("CLASSIC".into()).as_str(), "other");
    }
}
