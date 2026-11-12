//! What sizing answers: a sized entry with every figure it was worked out
//! from, or a refusal with the figures reached before it. Each amount
//! carries its asset: the settlement asset for the budget, the risk and
//! the margin, the quote asset for the notional, and the pool's own unit
//! for what the pool is asked for.

use super::request::UsdIndex;
use crate::model::exact::Exact;
use crate::model::futures::{EntryStop, PoolScope};
use crate::model::{
    FaceValue, InstrumentId, Leverage, Money, PositionSide, Price, Quantity, Refusal,
};
use std::fmt;

/// Sizing's answer about an entry it could judge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sizing {
    /// The entry may be sent with this quantity, once
    /// [`SizedEntry::at_submission`] clears it against fresh prices.
    Sized(Box<SizedEntry>),
    /// The entry is refused. Nothing is shrunk to fit.
    Refused(Box<SizingRefusal>),
}

impl Sizing {
    /// Every figure worked out, as far as sizing got.
    pub fn figures(&self) -> SizingFigures {
        match self {
            Self::Sized(entry) => entry.figures(),
            Self::Refused(refusal) => refusal.figures.clone(),
        }
    }
}

/// The stop an entry is sized against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizedStop {
    /// S′: the stop asked for, rounded to the tick away from the entry, so
    /// it is never tighter than asked.
    pub price: Price,
    /// What was asked: the alert's own stop, or the policy's fallback
    /// percentage.
    pub asked: EntryStop,
}

/// A quantity and what it risks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizedPosition {
    /// On the market-order step, floored: never rounded up.
    pub quantity: Quantity,
    /// What one unit loses from E to the stop, in the settlement asset.
    pub unit_loss: Money,
    /// What the quantity loses from E to the stop: at or below the budget.
    /// It is not a loss limit: gaps, fees and funding can exceed it.
    pub risk: Money,
}

/// What a quantity is worth at E.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notional {
    /// In the quote asset: quantity × E for linear, contracts × face value
    /// for inverse.
    pub quote: Money,
    /// In the settlement asset: the same for linear, that ÷ E for inverse.
    pub settle: Money,
}

/// The margin an entry needs, in the settlement asset. Leverage changes
/// this, never the quantity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarginNeed {
    /// The notional in the settlement asset ÷ the leverage.
    pub initial: Money,
    /// Taker fees for opening and for closing.
    pub fees: Money,
    /// Both together.
    pub required: Money,
}

/// The margin an entry needs, measured against the pool that funds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolFit {
    pub scope: PoolScope,
    /// What the entry needs, in the pool's unit.
    pub required: Money,
    /// What the pool has free: its available margin less what is reserved.
    /// Below zero when the reservations exceed it.
    pub free: Money,
    /// The index price a USD pool was measured at, with its source and
    /// time; `None` for an asset's own pool, which needs no conversion.
    pub index: Option<UsdIndex>,
}

/// Every figure of one sizing, as far as it got. A refusal holds the
/// figures reached before it; a sized entry holds them all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizingFigures {
    /// R: the capital × the risk percentage.
    pub budget: Money,
    pub stop: Option<SizedStop>,
    pub position: Option<SizedPosition>,
    pub notional: Option<Notional>,
    pub margin: Option<MarginNeed>,
    /// The largest position, in the settlement asset, that the leverage's
    /// brackets allow. `None` too when no bracket allows the leverage.
    pub bracket_cap: Option<Money>,
    pub pool: Option<PoolFit>,
}

/// Why an entry is refused: one of the parity matrix's refusals, what
/// would have to change, and the figures reached before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizingRefusal {
    pub refusal: Refusal,
    pub message: String,
    pub figures: SizingFigures,
}

impl fmt::Display for SizingRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.refusal, self.message)
    }
}

impl std::error::Error for SizingRefusal {}

/// Something an operator should know about a sized entry that does not
/// refuse it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SizingWarning {
    /// The settlement asset's wallet holds the risk budget, but less than
    /// the capital the policy sizes from.
    WalletBelowCapital,
}

impl SizingWarning {
    pub const ALL: [Self; 1] = [Self::WalletBelowCapital];

    /// The warning's name as records spell it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WalletBelowCapital => "wallet_below_capital",
        }
    }
}

impl fmt::Display for SizingWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An entry that passed every rule of ADR 0002 §6: the quantity to send
/// and what it was worked out from. Only sizing makes one. Before it is
/// sent, [`SizedEntry::at_submission`] holds it to fresh prices; once it
/// has filled, [`SizedEntry::fill_risk`] reports the risk actually taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizedEntry {
    pub(super) instrument: InstrumentId,
    pub(super) side: PositionSide,
    pub(super) expected_price: Price,
    pub(super) face_value: Option<FaceValue>,
    pub(super) leverage: Leverage,
    /// R before any rounding, for the comparisons that come later.
    pub(super) exact_budget: Exact,
    pub(super) budget: Money,
    pub(super) stop: SizedStop,
    pub(super) position: SizedPosition,
    pub(super) notional: Notional,
    pub(super) margin: MarginNeed,
    pub(super) bracket_cap: Money,
    pub(super) pool: PoolFit,
    pub(super) warnings: Vec<SizingWarning>,
}

impl SizedEntry {
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub const fn side(&self) -> PositionSide {
        self.side
    }

    /// E, the alert's expected price the quantity was sized from.
    pub const fn expected_price(&self) -> Price {
        self.expected_price
    }

    pub const fn leverage(&self) -> Leverage {
        self.leverage
    }

    /// R: the capital × the risk percentage, in the settlement asset.
    pub fn budget(&self) -> &Money {
        &self.budget
    }

    pub const fn stop(&self) -> SizedStop {
        self.stop
    }

    /// The quantity to send.
    pub const fn quantity(&self) -> Quantity {
        self.position.quantity
    }

    pub fn position(&self) -> &SizedPosition {
        &self.position
    }

    pub fn notional(&self) -> &Notional {
        &self.notional
    }

    pub fn margin(&self) -> &MarginNeed {
        &self.margin
    }

    /// The largest position, in the settlement asset, that the leverage's
    /// brackets allow.
    pub fn bracket_cap(&self) -> &Money {
        &self.bracket_cap
    }

    pub fn pool(&self) -> &PoolFit {
        &self.pool
    }

    pub fn warnings(&self) -> &[SizingWarning] {
        &self.warnings
    }

    /// Every figure, in the form a refusal reports the ones it reached.
    pub fn figures(&self) -> SizingFigures {
        SizingFigures {
            budget: self.budget.clone(),
            stop: Some(self.stop),
            position: Some(self.position.clone()),
            notional: Some(self.notional.clone()),
            margin: Some(self.margin.clone()),
            bracket_cap: Some(self.bracket_cap.clone()),
            pool: Some(self.pool.clone()),
        }
    }
}
