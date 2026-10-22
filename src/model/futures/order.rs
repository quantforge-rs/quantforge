//! Futures orders (ADR 0002 §7, §9): the market entry, the reduce-only
//! close, what a venue answers when it accepts one, and what queries and
//! streams report afterwards. An acknowledgement carries no status and no
//! fills, so it can never stand in for a fill; only an order or a fill read
//! back from the venue can.

use super::{ClientOrderId, FuturesInstrument, OrderLeg, TriggerBasis};
use crate::model::{
    InstrumentId, ModelError, Money, PositionSide, Price, Quantity, Side, TimestampMs,
    VenueOrderId, VenueTradeId,
};
use serde::{Deserialize, Serialize};

/// A stop sent with the entry order, where the venue supports attaching
/// one; otherwise the stop is placed after the first fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedStop {
    pub trigger_price: Price,
    pub basis: TriggerBasis,
}

/// A market order that opens or adds to a position: the only order that
/// creates exposure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryOrder {
    instrument: InstrumentId,
    side: PositionSide,
    quantity: Quantity,
    client_order_id: ClientOrderId,
    attached_stop: Option<AttachedStop>,
}

impl EntryOrder {
    /// Refuses an instrument outside the universe, a quantity in the wrong
    /// unit or of zero, and a client ID that is not an entry's.
    pub fn new(
        instrument: &FuturesInstrument,
        side: PositionSide,
        quantity: Quantity,
        client_order_id: ClientOrderId,
        attached_stop: Option<AttachedStop>,
    ) -> Result<Self, ModelError> {
        instrument
            .universe()
            .map_err(|refusal| ModelError::UnsupportedInstrument {
                instrument: instrument.id().to_string(),
                reason: refusal.to_string(),
            })?;
        let quantity = positive(quantity.in_unit_of(instrument.category())?)?;
        expect_leg(&client_order_id, &[OrderLeg::Entry])?;
        Ok(Self {
            instrument: instrument.id().clone(),
            side,
            quantity,
            client_order_id,
            attached_stop,
        })
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub const fn side(&self) -> PositionSide {
        self.side
    }

    pub const fn quantity(&self) -> Quantity {
        self.quantity
    }

    pub fn client_order_id(&self) -> &ClientOrderId {
        &self.client_order_id
    }

    pub const fn attached_stop(&self) -> Option<AttachedStop> {
        self.attached_stop
    }
}

/// A reduce-only market order that closes some or all of a position. It
/// can never open or reverse one, so it needs no universe check: a
/// position on a delisting instrument must still close.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseOrder {
    instrument: InstrumentId,
    closes: PositionSide,
    quantity: Quantity,
    client_order_id: ClientOrderId,
}

impl CloseOrder {
    /// `closes` is the side of the position being closed.
    pub fn new(
        instrument: InstrumentId,
        closes: PositionSide,
        quantity: Quantity,
        client_order_id: ClientOrderId,
    ) -> Result<Self, ModelError> {
        let quantity = positive(quantity.in_unit_of(instrument.category())?)?;
        expect_leg(&client_order_id, &[OrderLeg::Close])?;
        Ok(Self {
            instrument,
            closes,
            quantity,
            client_order_id,
        })
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub const fn closes(&self) -> PositionSide {
        self.closes
    }

    pub const fn quantity(&self) -> Quantity {
        self.quantity
    }

    pub fn client_order_id(&self) -> &ClientOrderId {
        &self.client_order_id
    }
}

fn positive(quantity: Quantity) -> Result<Quantity, ModelError> {
    if quantity.is_zero() {
        return Err(ModelError::InvalidAmount {
            what: "order quantity",
            value: quantity.to_string(),
            rule: "positive",
        });
    }
    Ok(quantity)
}

/// Refuses a client ID whose leg is not one of `legs`.
pub(super) fn expect_leg(id: &ClientOrderId, legs: &[OrderLeg]) -> Result<(), ModelError> {
    if !legs.contains(&id.leg()) {
        return Err(ModelError::InvalidIdentifier {
            what: "client order id",
            value: id.to_string(),
            rule: "its leg does not match the order",
        });
    }
    Ok(())
}

/// What a venue answers when it accepts an order: the IDs, and nothing
/// about status or fills.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAck {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: Option<VenueOrderId>,
    pub acknowledged_at_ms: TimestampMs,
}

/// One attempt of an operation: the ID it was sent with and when. ADR 0002
/// §9's bounds are timed from `sent_at_ms`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAttempt {
    pub client_order_id: ClientOrderId,
    pub sent_at_ms: TimestampMs,
}

/// Who placed an order: an account holder (QuantForge or the operator), or
/// the venue itself when it liquidates or deleverages a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderOrigin {
    Client,
    Liquidation,
    Adl,
}

/// Who cancelled an order, where the venue says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelAttribution {
    /// The account's user, through any interface.
    ByUser,
    /// The venue, such as an expiry after the position closed.
    BySystem,
    /// The venue does not say.
    Unattributed,
}

/// An order's status, normalized across venues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FuturesOrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Canceled(CancelAttribution),
    Rejected,
    Expired,
}

impl FuturesOrderStatus {
    /// Whether the order can still fill.
    pub const fn is_final(self) -> bool {
        !matches!(self, Self::New | Self::PartiallyFilled)
    }
}

/// Where a record came from: a query answered, or a private stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecordSource {
    Query,
    Stream,
}

/// An order as the venue reports it. Its client ID stays raw text: one
/// that QuantForge did not issue is someone else's order, never an error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuturesOrder {
    pub instrument: InstrumentId,
    pub venue_order_id: VenueOrderId,
    pub client_id: Option<String>,
    pub side: Side,
    pub reduce_only: bool,
    pub origin: OrderOrigin,
    pub status: FuturesOrderStatus,
    pub requested: Quantity,
    pub filled: Quantity,
    pub average_price: Option<Price>,
    pub source: RecordSource,
    pub updated_at_ms: TimestampMs,
}

impl FuturesOrder {
    /// The client ID read as QuantForge's, or `None` for anyone else's.
    pub fn client_order_id(&self) -> Option<ClientOrderId> {
        self.client_id
            .as_deref()
            .and_then(ClientOrderId::parse_ours)
    }
}

/// The answer to a query by client ID. "Not found" proves nothing until
/// ADR 0002 §9's propagation delay has passed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderLookup {
    Found(Box<FuturesOrder>),
    NotFound { queried_at_ms: TimestampMs },
}

/// What a venue guarantees about finding an order by its client ID: whether
/// the propagation delay is enough for an accepted order to show up
/// (verified per venue, family and environment), and how long an order
/// that ended unfilled stays findable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrderLookupGuarantee {
    pub visibility_verified: bool,
    pub unfilled_retention_ms: i64,
}

/// One fill, as the venue's trade records or private stream report it. A
/// funding payment is never a fill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuturesFill {
    pub trade_id: VenueTradeId,
    pub venue_order_id: VenueOrderId,
    /// Absent where the venue's trade records carry no client ID.
    pub client_id: Option<String>,
    pub instrument: InstrumentId,
    pub origin: OrderOrigin,
    pub side: Side,
    pub price: Price,
    pub quantity: Quantity,
    pub fee: Money,
    /// The venue's own realized PnL for the fill, where it reports one.
    pub realized_pnl: Option<Money>,
    pub time_ms: TimestampMs,
    pub source: RecordSource,
}

impl FuturesFill {
    /// The client ID read as QuantForge's, or `None`.
    pub fn client_order_id(&self) -> Option<ClientOrderId> {
        self.client_id
            .as_deref()
            .and_then(ClientOrderId::parse_ours)
    }
}

/// A venue's definitive refusal of an order: it answered, and nothing was
/// executed. Only the ports build one, from an exchange error whose kind
/// says so; a transport failure or an unknown outcome never becomes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    kind: &'static str,
    message: String,
}

impl Rejection {
    pub(crate) fn new(kind: &'static str, message: String) -> Self {
        Self { kind, message }
    }

    /// The exchange error kind, such as `insufficient_balance`.
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::futures::{ActivationEpoch, OwnerTag};
    use crate::model::{Category, OperationId};
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
    fn a_close_refuses_the_wrong_unit_zero_and_another_legs_id() {
        let instrument = InstrumentId::new(
            crate::model::ExchangeId::BybitInverse,
            crate::model::Symbol::new("BTCUSD").expect("symbol"),
            crate::model::Asset::new("BTC").expect("asset"),
        );
        let contracts = Quantity::for_category(Category::Inverse, Decimal::from(10)).expect("qty");
        let base = Quantity::for_category(Category::Linear, Decimal::from(10)).expect("qty");
        let zero = Quantity::for_category(Category::Inverse, Decimal::ZERO).expect("qty");
        let close = |quantity, leg| {
            CloseOrder::new(
                instrument.clone(),
                PositionSide::Long,
                quantity,
                client_id(leg),
            )
        };
        assert!(close(contracts, OrderLeg::Close).is_ok());
        assert!(close(base, OrderLeg::Close).is_err());
        assert!(close(zero, OrderLeg::Close).is_err());
        assert!(close(contracts, OrderLeg::Entry).is_err());
    }

    #[test]
    fn only_new_and_partially_filled_orders_can_still_fill() {
        let open = [FuturesOrderStatus::New, FuturesOrderStatus::PartiallyFilled];
        assert!(open.iter().all(|status| !status.is_final()));
        let ended = [
            FuturesOrderStatus::Filled,
            FuturesOrderStatus::Canceled(CancelAttribution::ByUser),
            FuturesOrderStatus::Rejected,
            FuturesOrderStatus::Expired,
        ];
        assert!(ended.iter().all(|status| status.is_final()));
    }
}
