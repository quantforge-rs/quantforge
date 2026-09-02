//! Binance wire DTOs: the exact shapes the REST API returns. Field names are
//! camelCase on the wire and always bridged with `#[serde(rename)]`; the
//! mapping onto domain types lives in [`super::convert`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct BinanceApiError {
    pub(super) code: i64,
    pub(super) msg: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceExchangeInfoResponse {
    pub(super) symbols: Vec<BinanceSymbolInfo>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceSymbolInfo {
    pub(super) symbol: String,
    #[serde(rename = "baseAsset")]
    pub(super) base_asset: String,
    #[serde(rename = "quoteAsset")]
    pub(super) quote_asset: String,
    pub(super) filters: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceAccountResponse {
    pub(super) balances: Vec<BinanceBalance>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceBalance {
    pub(super) asset: String,
    pub(super) free: String,
    pub(super) locked: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceAccountTrade {
    pub(super) symbol: String,
    pub(super) id: i64,
    #[serde(rename = "orderId")]
    pub(super) order_id: i64,
    pub(super) price: String,
    pub(super) qty: String,
    #[serde(rename = "quoteQty")]
    pub(super) quote_qty: String,
    pub(super) commission: String,
    #[serde(rename = "commissionAsset")]
    pub(super) commission_asset: String,
    pub(super) time: i64,
    #[serde(rename = "isBuyer")]
    pub(super) is_buyer: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceFill {
    pub(super) price: String,
    pub(super) qty: String,
    pub(super) commission: String,
    #[serde(rename = "commissionAsset")]
    pub(super) commission_asset: Option<String>,
    #[serde(rename = "tradeId")]
    pub(super) trade_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BinanceOrderResponse {
    pub(super) symbol: String,
    #[serde(rename = "orderId")]
    pub(super) order_id: Option<i64>,
    #[serde(rename = "clientOrderId")]
    pub(super) client_order_id: Option<String>,
    pub(super) side: String,
    #[serde(rename = "type")]
    pub(super) order_type: String,
    pub(super) status: Option<String>,
    #[serde(rename = "origQty")]
    pub(super) orig_qty: Option<String>,
    #[serde(rename = "origQuoteOrderQty")]
    pub(super) orig_quote_order_qty: Option<String>,
    // Absent fields stay `None`: an ACK-style response that omits fill data
    // must remain distinguishable from a response reporting a zero fill.
    #[serde(rename = "executedQty")]
    pub(super) executed_qty: Option<String>,
    #[serde(rename = "cummulativeQuoteQty", alias = "cumulativeQuoteQty")]
    pub(super) cumulative_quote_qty: Option<String>,
    #[serde(rename = "transactTime")]
    pub(super) transact_time: Option<i64>,
    pub(super) fills: Option<Vec<BinanceFill>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct BinanceKlineRow(
    pub(super) i64,
    pub(super) String,
    pub(super) String,
    pub(super) String,
    pub(super) String,
    pub(super) String,
    pub(super) i64,
    pub(super) String,
    pub(super) u64,
    pub(super) String,
    pub(super) String,
    pub(super) String,
);
