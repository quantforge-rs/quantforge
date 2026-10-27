//! The connector implementations on top of the [`super::client`] transport:
//! `ExchangeConnector` (identity, capabilities, endpoint),
//! `MarketDataConnector` (klines, symbol rules, server time),
//! `TradingConnector` (orders), and `AccountConnector` (balances and trades).

use async_trait::async_trait;
use reqwest::Method;
use serde_json::Value;

use super::client::BinanceSpotClient;
use super::convert::{parse_decimal, parse_order, parse_symbol_rules};
use super::types::{
    BinanceAccountResponse, BinanceAccountTrade, BinanceExchangeInfoResponse, BinanceKlineRow,
    BinanceServerTime,
};
use crate::ports::{
    AccountConnector, ExchangeCapabilities, ExchangeConnector, MarketDataCapabilities,
    MarketDataConnector, OrderCapabilities, StreamCapabilities, TradingConnector,
};
use crate::{
    AccountTrade, AssetBalance, CancelOrderRequest, Candle, ExchangeError, ExchangeId,
    ExchangeOrder, KlineRequest, MarketOrderRequest, OrderQueryRequest, Side, Symbol, SymbolRules,
    TimestampMs, VenueOrderId, VenueTradeId,
};

/// What Binance Spot supports through this adapter: market orders sized
/// either way, client order ids, and the REST market data. Streams come
/// later.
const CAPABILITIES: ExchangeCapabilities = ExchangeCapabilities {
    orders: OrderCapabilities {
        market_by_base_quantity: true,
        market_by_quote_quantity: true,
        client_order_ids: true,
    },
    market_data: MarketDataCapabilities {
        max_klines_per_request: 1000,
        symbol_rules: true,
        server_time: true,
    },
    streams: StreamCapabilities {
        klines: false,
        user_data: false,
    },
};

impl ExchangeConnector for BinanceSpotClient {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::BinanceSpot
    }

    fn capabilities(&self) -> ExchangeCapabilities {
        CAPABILITIES
    }

    fn is_production(&self) -> bool {
        self.production_endpoint()
    }
}

#[async_trait]
impl MarketDataConnector for BinanceSpotClient {
    async fn fetch_klines(&self, request: &KlineRequest) -> Result<Vec<Candle>, ExchangeError> {
        let mut params = vec![
            ("symbol", request.symbol.as_str().to_string()),
            ("interval", request.interval.as_str().to_string()),
        ];
        if let Some(start_time_ms) = request.start_time_ms {
            params.push(("startTime", start_time_ms.to_string()));
        }
        if let Some(end_time_ms) = request.end_time_ms {
            params.push(("endTime", end_time_ms.to_string()));
        }
        if let Some(limit) = request.limit {
            params.push((
                "limit",
                limit
                    .min(CAPABILITIES.market_data.max_klines_per_request)
                    .to_string(),
            ));
        }

        let raw = self
            .send_public(Method::GET, "api/v3/klines", params)
            .await?;
        let rows: Vec<BinanceKlineRow> =
            serde_json::from_value(raw).map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to decode klines response: {err}"),
            })?;

        let mut candles = Vec::with_capacity(rows.len());
        for row in rows {
            candles.push(Candle {
                open_time_ms: row.0,
                open: parse_decimal(&row.1, "open")?,
                high: parse_decimal(&row.2, "high")?,
                low: parse_decimal(&row.3, "low")?,
                close: parse_decimal(&row.4, "close")?,
                volume: parse_decimal(&row.5, "volume")?,
                close_time_ms: row.6,
                trades: Some(row.8),
            });
        }

        Ok(candles)
    }

    async fn fetch_symbol_rules(&self, symbol: &Symbol) -> Result<SymbolRules, ExchangeError> {
        let raw = self
            .send_public(
                Method::GET,
                "api/v3/exchangeInfo",
                vec![("symbol", symbol.as_str().to_string())],
            )
            .await?;

        let response: BinanceExchangeInfoResponse =
            serde_json::from_value(raw).map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to decode exchangeInfo response: {err}"),
            })?;

        let info =
            response
                .symbols
                .into_iter()
                .next()
                .ok_or_else(|| ExchangeError::InvalidResponse {
                    message: format!(
                        "exchangeInfo returned no symbol rules for {}",
                        symbol.as_str()
                    ),
                })?;

        Ok(parse_symbol_rules(info)?)
    }

    async fn server_time(&self) -> Result<TimestampMs, ExchangeError> {
        let raw = self
            .send_public(Method::GET, "api/v3/time", Vec::new())
            .await?;
        let response: BinanceServerTime =
            serde_json::from_value(raw).map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to decode time response: {err}"),
            })?;
        Ok(response.server_time)
    }
}

#[async_trait]
impl TradingConnector for BinanceSpotClient {
    async fn submit_market_order(
        &self,
        request: &MarketOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        if request.quantity.is_none() && request.quote_order_qty.is_none() {
            return Err(ExchangeError::InvalidRequest {
                message: "submit_market_order requires quantity or quote_order_qty".to_string(),
            });
        }

        let mut params = vec![
            ("symbol", request.symbol.as_str().to_string()),
            ("side", request.side.as_str().to_string()),
            ("type", "MARKET".to_string()),
            ("newOrderRespType", "FULL".to_string()),
        ];

        if let Some(quantity) = request.quantity {
            params.push(("quantity", quantity.to_string()));
        }
        if let Some(quote_order_qty) = request.quote_order_qty {
            params.push(("quoteOrderQty", quote_order_qty.to_string()));
        }
        if let Some(client_order_id) = &request.new_client_order_id {
            params.push(("newClientOrderId", client_order_id.clone()));
        }

        let raw = self
            .send_signed(Method::POST, "api/v3/order", params)
            .await?;
        parse_order(raw)
    }

    async fn cancel_order(
        &self,
        request: &CancelOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        let mut params = vec![("symbol", request.symbol.as_str().to_string())];
        if let Some(order_id) = &request.order_id {
            params.push(("orderId", order_id.as_str().to_string()));
        }
        if let Some(client_order_id) = &request.client_order_id {
            params.push(("origClientOrderId", client_order_id.clone()));
        }
        if request.order_id.is_none() && request.client_order_id.is_none() {
            return Err(ExchangeError::InvalidRequest {
                message: "cancel_order requires order_id or client_order_id".to_string(),
            });
        }

        let raw = self
            .send_signed(Method::DELETE, "api/v3/order", params)
            .await?;
        parse_order(raw)
    }

    async fn query_order(
        &self,
        request: &OrderQueryRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        let mut params = vec![("symbol", request.symbol.as_str().to_string())];
        if let Some(order_id) = &request.order_id {
            params.push(("orderId", order_id.as_str().to_string()));
        }
        if let Some(client_order_id) = &request.client_order_id {
            params.push(("origClientOrderId", client_order_id.clone()));
        }
        if request.order_id.is_none() && request.client_order_id.is_none() {
            return Err(ExchangeError::InvalidRequest {
                message: "query_order requires order_id or client_order_id".to_string(),
            });
        }

        let raw = self
            .send_signed(Method::GET, "api/v3/order", params)
            .await?;
        parse_order(raw)
    }

    async fn open_orders(
        &self,
        symbol: Option<&Symbol>,
    ) -> Result<Vec<ExchangeOrder>, ExchangeError> {
        let mut params = Vec::new();
        if let Some(symbol) = symbol {
            params.push(("symbol", symbol.as_str().to_string()));
        }

        let raw = self
            .send_signed(Method::GET, "api/v3/openOrders", params)
            .await?;
        let raw_items: Vec<Value> =
            serde_json::from_value(raw).map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to decode openOrders response: {err}"),
            })?;

        raw_items.into_iter().map(parse_order).collect()
    }
}

#[async_trait]
impl AccountConnector for BinanceSpotClient {
    async fn account_balances(&self) -> Result<Vec<AssetBalance>, ExchangeError> {
        let raw = self
            .send_signed(
                Method::GET,
                "api/v3/account",
                vec![("omitZeroBalances", "true".to_string())],
            )
            .await?;

        let response: BinanceAccountResponse =
            serde_json::from_value(raw).map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to decode account response: {err}"),
            })?;

        let mut balances = Vec::with_capacity(response.balances.len());
        for balance in response.balances {
            balances.push(AssetBalance {
                asset: balance.asset,
                free: parse_decimal(&balance.free, "free")?,
                locked: parse_decimal(&balance.locked, "locked")?,
            });
        }
        Ok(balances)
    }

    async fn recent_trades(
        &self,
        symbol: &Symbol,
        limit: usize,
    ) -> Result<Vec<AccountTrade>, ExchangeError> {
        let raw = self
            .send_signed(
                Method::GET,
                "api/v3/myTrades",
                vec![
                    ("symbol", symbol.as_str().to_string()),
                    ("limit", limit.min(1000).to_string()),
                ],
            )
            .await?;

        let rows: Vec<BinanceAccountTrade> =
            serde_json::from_value(raw).map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to decode myTrades response: {err}"),
            })?;

        let mut trades = Vec::with_capacity(rows.len());
        for row in rows {
            trades.push(AccountTrade {
                symbol: Symbol::new(row.symbol)?,
                trade_id: VenueTradeId::new(row.id.to_string())?,
                order_id: VenueOrderId::new(row.order_id.to_string())?,
                side: if row.is_buyer { Side::Buy } else { Side::Sell },
                price: parse_decimal(&row.price, "trade.price")?,
                qty: parse_decimal(&row.qty, "trade.qty")?,
                quote_qty: parse_decimal(&row.quote_qty, "trade.quoteQty")?,
                commission: parse_decimal(&row.commission, "trade.commission")?,
                commission_asset: Some(row.commission_asset),
                time_ms: row.time,
            });
        }

        Ok(trades)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    // A full literal, so a capability added to the descriptor fails to
    // compile here until Binance's value for it is decided.
    #[test]
    fn declared_capabilities_are_pinned() {
        let client = BinanceSpotClient::new(Url::parse("https://api.binance.com/").expect("url"));
        assert_eq!(
            client.capabilities(),
            ExchangeCapabilities {
                orders: OrderCapabilities {
                    market_by_base_quantity: true,
                    market_by_quote_quantity: true,
                    client_order_ids: true,
                },
                market_data: MarketDataCapabilities {
                    max_klines_per_request: 1000,
                    symbol_rules: true,
                    server_time: true,
                },
                streams: StreamCapabilities {
                    klines: false,
                    user_data: false,
                },
            }
        );
        assert_eq!(client.exchange_id(), ExchangeId::BinanceSpot);
    }
}
