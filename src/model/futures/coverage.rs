//! The coverage check (ADR 0002 §7): the only way to call a position
//! protected is a read-back that shows an active stop covering all of it.
//! Deciding whose stop it is, QuantForge's from the ledger or the
//! operator's, belongs to the protection orchestrator, which picks the
//! check to apply.

use super::{PositionSnapshot, PriceProtection, StopOrder, StopScope, StopStatus, TriggerBasis};
use crate::model::Price;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The stop QuantForge recorded for a position: what its read-back must
/// show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedStop {
    pub trigger_price: Price,
    pub basis: TriggerBasis,
    pub price_protection: PriceProtection,
}

/// Whose stop covers a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopOwner {
    QuantForge,
    Operator,
}

/// Why a read-back does not show a stop covering the position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverageGap {
    OtherInstrument,
    Flat,
    WrongSide,
    NotActive,
    TooSmall,
    BasisUnreported,
    NotAsRecorded,
}

impl fmt::Display for CoverageGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OtherInstrument => "the stop is on another instrument",
            Self::Flat => "the position is flat",
            Self::WrongSide => "the stop closes the other side",
            Self::NotActive => "the stop is not active",
            Self::TooSmall => "the stop covers less than the whole position",
            Self::BasisUnreported => "the venue does not report the stop's trigger basis",
            Self::NotAsRecorded => "the stop's price, basis or protection differ from the record",
        })
    }
}

/// Evidence that a read-back shows an active stop covering the whole
/// position: the only way a position becomes protected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoveredStop {
    owner: StopOwner,
    stop: StopOrder,
}

impl CoveredStop {
    /// QuantForge's stop: it must also show the recorded price, the mark
    /// basis and the recorded price protection.
    pub fn ours(
        stop: &StopOrder,
        position: &PositionSnapshot,
        recorded: &RecordedStop,
    ) -> Result<Self, CoverageGap> {
        covers(stop, position)?;
        let as_recorded = stop.trigger_price == recorded.trigger_price
            && stop.basis == Some(TriggerBasis::Mark)
            && recorded.basis == TriggerBasis::Mark
            && stop.price_protection == recorded.price_protection;
        if !as_recorded {
            return Err(CoverageGap::NotAsRecorded);
        }
        Ok(Self {
            owner: StopOwner::QuantForge,
            stop: stop.clone(),
        })
    }

    /// The operator's stop: any price, on a basis the venue reports.
    pub fn operators(stop: &StopOrder, position: &PositionSnapshot) -> Result<Self, CoverageGap> {
        covers(stop, position)?;
        Ok(Self {
            owner: StopOwner::Operator,
            stop: stop.clone(),
        })
    }

    pub const fn owner(&self) -> StopOwner {
        self.owner
    }

    pub fn stop(&self) -> &StopOrder {
        &self.stop
    }
}

fn covers(stop: &StopOrder, position: &PositionSnapshot) -> Result<(), CoverageGap> {
    if stop.instrument != position.instrument {
        return Err(CoverageGap::OtherInstrument);
    }
    let Some(exposure) = position
        .exposure
        .filter(|exposure| !exposure.size.is_zero())
    else {
        return Err(CoverageGap::Flat);
    };
    if stop.position_side != exposure.side {
        return Err(CoverageGap::WrongSide);
    }
    if stop.status != StopStatus::Active {
        return Err(CoverageGap::NotActive);
    }
    if let StopScope::Quantity(quantity) = stop.scope {
        let same_unit = quantity.unit() == exposure.size.unit();
        if !same_unit || quantity.value() < exposure.size.value() {
            return Err(CoverageGap::TooSmall);
        }
    }
    if stop.basis.is_none() {
        return Err(CoverageGap::BasisUnreported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::futures::{Exposure, Liquidation, LiquidationGap, MarginMode, StopRef};
    use crate::model::{
        Asset, Category, ExchangeId, InstrumentId, Leverage, PositionSide, Quantity, Symbol,
    };
    use rust_decimal::Decimal;

    fn instrument() -> InstrumentId {
        InstrumentId::new(
            ExchangeId::BybitLinear,
            Symbol::new("SOLUSDT").expect("symbol"),
            Asset::new("USDT").expect("asset"),
        )
    }

    fn price(value: i64) -> Price {
        Price::new(Decimal::from(value)).expect("price")
    }

    fn base(value: i64) -> Quantity {
        Quantity::for_category(Category::Linear, Decimal::from(value)).expect("qty")
    }

    fn position(size: i64) -> PositionSnapshot {
        PositionSnapshot {
            instrument: instrument(),
            exposure: Some(Exposure {
                side: PositionSide::Long,
                size: base(size),
                entry_price: price(234),
            }),
            notional: None,
            leverage: Leverage::new(Decimal::from(5)).expect("leverage"),
            margin_mode: MarginMode::Cross,
            mark_price: None,
            liquidation: Liquidation::Unavailable(LiquidationGap::ReportedEmpty),
            unrealized_pnl: None,
            margin: None,
            observed_at_ms: 1,
        }
    }

    fn stop(basis: Option<TriggerBasis>, scope: StopScope, status: StopStatus) -> StopOrder {
        StopOrder {
            stop: StopRef::Position {
                instrument: instrument(),
            },
            instrument: instrument(),
            position_side: PositionSide::Long,
            trigger_price: price(200),
            basis,
            price_protection: PriceProtection::NotApplicable,
            scope,
            status,
            triggered_order: None,
            updated_at_ms: 1,
        }
    }

    fn recorded() -> RecordedStop {
        RecordedStop {
            trigger_price: price(200),
            basis: TriggerBasis::Mark,
            price_protection: PriceProtection::NotApplicable,
        }
    }

    #[test]
    fn a_whole_position_mark_stop_as_recorded_covers_the_position() {
        let full = stop(
            Some(TriggerBasis::Mark),
            StopScope::WholePosition,
            StopStatus::Active,
        );
        let covered = CoveredStop::ours(&full, &position(4), &recorded()).expect("covered");
        assert_eq!(covered.owner(), StopOwner::QuantForge);
        let bridge = stop(
            Some(TriggerBasis::Mark),
            StopScope::Quantity(base(4)),
            StopStatus::Active,
        );
        assert!(CoveredStop::ours(&bridge, &position(4), &recorded()).is_ok());
    }

    #[test]
    fn a_read_back_that_proves_less_covers_nothing() {
        let cases = [
            (
                stop(None, StopScope::WholePosition, StopStatus::Active),
                4,
                CoverageGap::BasisUnreported,
            ),
            (
                stop(
                    Some(TriggerBasis::Mark),
                    StopScope::Quantity(base(3)),
                    StopStatus::Active,
                ),
                4,
                CoverageGap::TooSmall,
            ),
            (
                stop(
                    Some(TriggerBasis::Mark),
                    StopScope::WholePosition,
                    StopStatus::Triggered,
                ),
                4,
                CoverageGap::NotActive,
            ),
            (
                stop(
                    Some(TriggerBasis::Mark),
                    StopScope::WholePosition,
                    StopStatus::Active,
                ),
                0,
                CoverageGap::Flat,
            ),
            (
                stop(
                    Some(TriggerBasis::Last),
                    StopScope::WholePosition,
                    StopStatus::Active,
                ),
                4,
                CoverageGap::NotAsRecorded,
            ),
        ];
        for (read_back, size, gap) in cases {
            assert_eq!(
                CoveredStop::ours(&read_back, &position(size), &recorded()),
                Err(gap)
            );
        }
        let last = stop(
            Some(TriggerBasis::Last),
            StopScope::WholePosition,
            StopStatus::Active,
        );
        let operators = CoveredStop::operators(&last, &position(4)).expect("operator's stop");
        assert_eq!(operators.owner(), StopOwner::Operator);
    }
}
