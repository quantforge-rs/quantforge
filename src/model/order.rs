//! Exchange order lifecycle: order side and status, individual fills, and
//! the exchange-reported order and account-trade records.

use super::{ModelError, Symbol, TimestampMs};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Side {
    type Err = ModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_uppercase().as_str() {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => Err(ModelError::InvalidSide(other.to_string())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Expired,
    PendingNew,
    Unknown,
}

impl OrderStatus {
    pub fn from_exchange(value: &str) -> Self {
        match value.trim().to_ascii_uppercase().as_str() {
            "NEW" => Self::New,
            "PARTIALLY_FILLED" => Self::PartiallyFilled,
            "FILLED" => Self::Filled,
            "CANCELED" => Self::Canceled,
            "REJECTED" => Self::Rejected,
            "EXPIRED" => Self::Expired,
            "PENDING_NEW" => Self::PendingNew,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::PartiallyFilled => "PARTIALLY_FILLED",
            Self::Filled => "FILLED",
            Self::Canceled => "CANCELED",
            Self::Rejected => "REJECTED",
            Self::Expired => "EXPIRED",
            Self::PendingNew => "PENDING_NEW",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Canceled | Self::Rejected | Self::Expired
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub price: Decimal,
    pub qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: Option<String>,
    pub trade_id: Option<i64>,
}

/// Exchange-reported order state.
///
/// `executed_qty`, `cumulative_quote_qty`, and `fills` are `None` when the
/// exchange response did not report them (e.g. ACK-style responses, or the
/// open-orders endpoint which never includes fills). `None` means
/// "not reported" and is deliberately distinct from a reported zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExchangeOrder {
    pub symbol: Symbol,
    pub side: Side,
    pub order_type: String,
    pub status: OrderStatus,
    pub order_id: Option<i64>,
    pub client_order_id: Option<String>,
    pub requested_qty: Option<Decimal>,
    pub requested_quote_qty: Option<Decimal>,
    pub executed_qty: Option<Decimal>,
    pub cumulative_quote_qty: Option<Decimal>,
    pub avg_price: Option<Decimal>,
    pub transact_time_ms: Option<TimestampMs>,
    pub fills: Option<Vec<Fill>>,
    pub raw: serde_json::Value,
}

impl ExchangeOrder {
    pub fn average_price(&self) -> Option<Decimal> {
        if let Some(price) = self.avg_price {
            return Some(price);
        }
        match (self.executed_qty, self.cumulative_quote_qty) {
            (Some(executed), Some(cumulative)) if executed > Decimal::ZERO => {
                Some(cumulative / executed)
            }
            _ => None,
        }
    }

    /// Executed quantity minus fees charged in the base asset.
    ///
    /// Returns `None` when the exchange did not report the executed quantity
    /// or the fills, so callers must decide explicitly instead of assuming a
    /// fee-free fill. Fills without a `commission_asset` are treated as not
    /// charged in the base asset (under-deduction is surfaced by the fill
    /// itself, never invented).
    pub fn net_base_qty_after_base_fees(&self, base_asset: &str) -> Option<Decimal> {
        let executed = self.executed_qty?;
        let fills = self.fills.as_ref()?;
        let mut qty = executed;
        for fill in fills {
            if fill
                .commission_asset
                .as_deref()
                .map(|asset| asset.eq_ignore_ascii_case(base_asset))
                .unwrap_or(false)
            {
                qty -= fill.commission;
            }
        }
        Some(qty.max(Decimal::ZERO))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountTrade {
    pub symbol: Symbol,
    pub trade_id: i64,
    pub order_id: i64,
    pub side: Side,
    pub price: Decimal,
    pub qty: Decimal,
    pub quote_qty: Decimal,
    pub commission: Decimal,
    pub commission_asset: Option<String>,
    pub time_ms: TimestampMs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_average_price_falls_back_to_ratio() {
        let order = ExchangeOrder {
            symbol: Symbol::new("BTCUSDT").expect("symbol"),
            side: Side::Buy,
            order_type: "MARKET".to_string(),
            status: OrderStatus::Filled,
            order_id: Some(1),
            client_order_id: Some("abc".to_string()),
            requested_qty: None,
            requested_quote_qty: Some(Decimal::from_str("100").expect("decimal")),
            executed_qty: Some(Decimal::from_str("0.01").expect("decimal")),
            cumulative_quote_qty: Some(Decimal::from_str("100").expect("decimal")),
            avg_price: None,
            transact_time_ms: Some(1),
            fills: Some(Vec::new()),
            raw: serde_json::json!({}),
        };

        assert_eq!(
            order.average_price(),
            Some(Decimal::from_str("10000").expect("decimal"))
        );
    }
}
