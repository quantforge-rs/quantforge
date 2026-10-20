//! Exchange-agnostic requests a connector serves: candle history, market
//! orders, and order cancels and lookups. Each adapter maps them onto its own
//! wire format; nothing here names an exchange.

use super::{Interval, Side, Symbol, TimestampMs};
use rust_decimal::Decimal;

#[derive(Clone, Debug)]
pub struct KlineRequest {
    pub symbol: Symbol,
    pub interval: Interval,
    pub start_time_ms: Option<TimestampMs>,
    pub end_time_ms: Option<TimestampMs>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct MarketOrderRequest {
    pub symbol: Symbol,
    pub side: Side,
    pub quantity: Option<Decimal>,
    pub quote_order_qty: Option<Decimal>,
    pub new_client_order_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CancelOrderRequest {
    pub symbol: Symbol,
    pub order_id: Option<i64>,
    pub client_order_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OrderQueryRequest {
    pub symbol: Symbol,
    pub order_id: Option<i64>,
    pub client_order_id: Option<String>,
}
