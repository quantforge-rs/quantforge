//! The exchange-held stop (ADR 0002 §7, §8, §10): what is asked for, what a
//! read-back shows, and what placing one did. The check that a read-back
//! covers a position is in [`CoveredStop`](super::CoveredStop).

use super::order::{expect_leg, positive};
use super::{CancelAttribution, ClientOrderId, OrderLeg};
use crate::model::{
    InstrumentId, ModelError, PositionSide, Price, Quantity, TimestampMs, VenueAlgoId, VenueOrderId,
};
use serde::{Deserialize, Serialize};

/// The price a stop triggers on. QuantForge's stops use the mark price,
/// which also drives liquidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerBasis {
    Mark,
    Last,
    Index,
}

/// A venue setting that can stop a trigger in a fast market. Where a venue
/// has it, QuantForge sets it explicitly; elsewhere it does not apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PriceProtection {
    Enabled,
    Disabled,
    NotApplicable,
}

/// What a stop closes when it triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopScope {
    /// The whole position, whatever its size when the stop triggers.
    WholePosition,
    /// A fixed quantity: the reduce-only bridge a replacement places first.
    Quantity(Quantity),
}

/// A stop to place: always reduce-only, on the side opposite the position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRequest {
    instrument: InstrumentId,
    position_side: PositionSide,
    trigger_price: Price,
    basis: TriggerBasis,
    price_protection: PriceProtection,
    scope: StopScope,
    client_order_id: ClientOrderId,
}

impl StopRequest {
    /// A whole-position stop takes a stop leg's ID, a bridge a bridge leg's,
    /// and a bridge's quantity is positive and in the instrument's unit.
    pub fn new(
        instrument: InstrumentId,
        position_side: PositionSide,
        trigger_price: Price,
        basis: TriggerBasis,
        price_protection: PriceProtection,
        scope: StopScope,
        client_order_id: ClientOrderId,
    ) -> Result<Self, ModelError> {
        match scope {
            StopScope::WholePosition => expect_leg(&client_order_id, &[OrderLeg::Stop])?,
            StopScope::Quantity(quantity) => {
                positive("stop quantity", quantity.in_unit_of(instrument.category())?)?;
                expect_leg(&client_order_id, &[OrderLeg::Bridge])?;
            }
        }
        Ok(Self {
            instrument,
            position_side,
            trigger_price,
            basis,
            price_protection,
            scope,
            client_order_id,
        })
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub const fn position_side(&self) -> PositionSide {
        self.position_side
    }

    pub const fn trigger_price(&self) -> Price {
        self.trigger_price
    }

    pub const fn basis(&self) -> TriggerBasis {
        self.basis
    }

    pub const fn price_protection(&self) -> PriceProtection {
        self.price_protection
    }

    pub const fn scope(&self) -> StopScope {
        self.scope
    }

    pub fn client_order_id(&self) -> &ClientOrderId {
        &self.client_order_id
    }
}

/// How a venue identifies a stop. Client IDs stay raw text, as for orders.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopRef {
    /// A conditional order on the venue's algo API.
    Algo {
        algo_id: VenueAlgoId,
        client_id: Option<String>,
    },
    /// A conditional order among the venue's ordinary orders.
    Order {
        order_id: VenueOrderId,
        client_id: Option<String>,
    },
    /// The stop set on the position itself, which has no ID of its own.
    Position { instrument: InstrumentId },
}

/// A stop's status, normalized across venues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StopStatus {
    /// Waiting for its trigger.
    Active,
    /// Triggered; its order is working.
    Triggered,
    /// Triggered and done.
    Finished,
    Canceled(CancelAttribution),
    /// Refused, including when it triggered: a protection failure.
    Rejected,
    /// Ended by the venue, as after the position closed.
    Expired,
}

/// A stop as a read-back shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopOrder {
    pub stop: StopRef,
    pub instrument: InstrumentId,
    pub position_side: PositionSide,
    pub trigger_price: Price,
    /// `None` when the venue does not report it; such a stop never counts
    /// as covering a position.
    pub basis: Option<TriggerBasis>,
    pub price_protection: PriceProtection,
    pub scope: StopScope,
    pub status: StopStatus,
    /// The order a triggered stop became, which links its fill to it.
    pub triggered_order: Option<VenueOrderId>,
    pub updated_at_ms: TimestampMs,
}

/// The answer to a query for one stop, finished ones included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopLookup {
    Found(Box<StopOrder>),
    NotFound { queried_at_ms: TimestampMs },
}

/// What placing a stop did. A stop the price has already crossed is not
/// placed, and that is final: the position goes to its protective close.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopPlacement {
    Placed(StopRef),
    PriceCrossed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::futures::{ActivationEpoch, OwnerTag};
    use crate::model::{Asset, Category, ExchangeId, OperationId, Symbol};
    use rust_decimal::Decimal;

    fn client_id(leg: OrderLeg) -> ClientOrderId {
        ClientOrderId::new(
            OwnerTag::new("k3x9p2").expect("tag"),
            ActivationEpoch(1),
            OperationId::new(5).expect("operation"),
            leg,
            1,
        )
        .expect("client order id")
    }

    #[test]
    fn a_bridge_stop_refuses_a_zero_quantity_the_wrong_unit_and_another_legs_id() {
        let instrument = InstrumentId::new(
            ExchangeId::BybitInverse,
            Symbol::new("BTCUSD").expect("symbol"),
            Asset::new("BTC").expect("asset"),
        );
        let contracts = |n| Quantity::for_category(Category::Inverse, Decimal::from(n));
        let base = Quantity::for_category(Category::Linear, Decimal::from(10)).expect("qty");
        let stop = |scope, leg| {
            StopRequest::new(
                instrument.clone(),
                PositionSide::Long,
                Price::new(Decimal::from(60_000)).expect("price"),
                TriggerBasis::Mark,
                PriceProtection::NotApplicable,
                scope,
                client_id(leg),
            )
        };
        let ten = StopScope::Quantity(contracts(10).expect("qty"));
        assert!(stop(StopScope::WholePosition, OrderLeg::Stop).is_ok());
        assert!(stop(ten, OrderLeg::Bridge).is_ok());
        let zero = StopScope::Quantity(contracts(0).expect("qty"));
        let error = stop(zero, OrderLeg::Bridge).expect_err("a zero bridge");
        assert!(
            matches!(
                error,
                ModelError::InvalidAmount {
                    what: "stop quantity",
                    ..
                }
            ),
            "{error}"
        );
        assert!(stop(StopScope::Quantity(base), OrderLeg::Bridge).is_err());
        assert!(stop(ten, OrderLeg::Stop).is_err());
        assert!(stop(StopScope::WholePosition, OrderLeg::Bridge).is_err());
    }
}
