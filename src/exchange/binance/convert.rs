//! Wire-to-domain mapping: turns the [`super::types`] DTOs into `SymbolRules`,
//! `ExchangeOrder` and the decimal fields they carry, reporting every
//! malformed field as an `ExchangeError::InvalidResponse`.

use rust_decimal::Decimal;
use serde_json::Value;
use tracing::warn;

use super::types::{BinanceOrderResponse, BinanceSymbolInfo};
use crate::{ExchangeError, ExchangeOrder, Fill, OrderStatus, Side, Symbol, SymbolRules};

pub(super) fn parse_symbol_rules(info: BinanceSymbolInfo) -> Result<SymbolRules, ExchangeError> {
    let mut rules = SymbolRules {
        symbol: Symbol::new(info.symbol)?,
        base_asset: info.base_asset,
        quote_asset: info.quote_asset,
        min_qty: None,
        max_qty: None,
        step_size: None,
        market_min_qty: None,
        market_max_qty: None,
        market_step_size: None,
        min_notional: None,
        tick_size: None,
    };

    for filter in info.filters {
        let Some(filter_type) = filter.get("filterType").and_then(Value::as_str) else {
            warn!(
                symbol = %rules.symbol,
                filter = %filter,
                "ignoring malformed exchange filter without a filterType"
            );
            continue;
        };

        match filter_type {
            "LOT_SIZE" => {
                rules.min_qty = parse_optional_filter_decimal(&filter, "minQty")?;
                rules.max_qty = parse_optional_filter_decimal(&filter, "maxQty")?;
                rules.step_size = parse_optional_filter_decimal(&filter, "stepSize")?;
            }
            "MARKET_LOT_SIZE" => {
                rules.market_min_qty = parse_optional_filter_decimal(&filter, "minQty")?;
                rules.market_max_qty = parse_optional_filter_decimal(&filter, "maxQty")?;
                rules.market_step_size = parse_optional_filter_decimal(&filter, "stepSize")?;
            }
            "MIN_NOTIONAL" => {
                rules.min_notional = parse_optional_filter_decimal(&filter, "minNotional")?;
            }
            "NOTIONAL" => {
                if rules.min_notional.is_none() {
                    rules.min_notional = parse_optional_filter_decimal(&filter, "minNotional")?;
                }
            }
            "PRICE_FILTER" => {
                rules.tick_size = parse_optional_filter_decimal(&filter, "tickSize")?;
            }
            _ => {}
        }
    }

    Ok(rules)
}

fn parse_optional_filter_decimal(
    filter: &Value,
    field: &str,
) -> Result<Option<Decimal>, ExchangeError> {
    match filter.get(field).and_then(Value::as_str) {
        // Binance renders "no constraint" as "0.00000000" (observed on the
        // Spot testnet's MARKET_LOT_SIZE). A non-positive limit is an
        // absent rule, not a zero-step rule: keeping the zero would make
        // `effective_market_step_size` skip the LOT_SIZE fallback and feed
        // `round_down_to_step` a step it asserts against.
        Some(raw) => Ok(Some(parse_decimal(raw, field)?).filter(|value| *value > Decimal::ZERO)),
        None => Ok(None),
    }
}

pub(super) fn parse_order(raw: Value) -> Result<ExchangeOrder, ExchangeError> {
    let response: BinanceOrderResponse =
        serde_json::from_value(raw.clone()).map_err(|err| ExchangeError::InvalidResponse {
            message: format!("failed to decode order response: {err}; raw={raw}"),
        })?;

    let symbol = Symbol::new(response.symbol)?;
    let side = response.side.parse::<Side>()?;
    let fills = response
        .fills
        .map(|fills| {
            fills
                .into_iter()
                .map(|fill| {
                    Ok(Fill {
                        price: parse_decimal(&fill.price, "fill.price")?,
                        qty: parse_decimal(&fill.qty, "fill.qty")?,
                        commission: parse_decimal(&fill.commission, "fill.commission")?,
                        commission_asset: fill.commission_asset,
                        trade_id: fill.trade_id,
                    })
                })
                .collect::<Result<Vec<_>, ExchangeError>>()
        })
        .transpose()?;

    let requested_qty = match response.orig_qty {
        Some(value) => Some(parse_decimal(&value, "origQty")?),
        None => None,
    };
    let requested_quote_qty = match response.orig_quote_order_qty {
        Some(value) => {
            let parsed = parse_decimal(&value, "origQuoteOrderQty")?;
            if parsed > Decimal::ZERO {
                Some(parsed)
            } else {
                None
            }
        }
        None => None,
    };

    let executed_qty = response
        .executed_qty
        .as_deref()
        .map(|value| parse_decimal(value, "executedQty"))
        .transpose()?;
    let cumulative_quote_qty = response
        .cumulative_quote_qty
        .as_deref()
        .map(|value| parse_decimal(value, "cumulativeQuoteQty"))
        .transpose()?;
    let avg_price = match (executed_qty, cumulative_quote_qty) {
        (Some(executed), Some(cumulative)) if executed > Decimal::ZERO => {
            Some(cumulative / executed)
        }
        _ => None,
    };

    Ok(ExchangeOrder {
        symbol,
        side,
        order_type: response.order_type,
        status: OrderStatus::from_exchange(response.status.as_deref().unwrap_or("UNKNOWN")),
        order_id: response.order_id,
        client_order_id: response.client_order_id,
        requested_qty,
        requested_quote_qty,
        executed_qty,
        cumulative_quote_qty,
        avg_price,
        transact_time_ms: response.transact_time,
        fills,
        raw,
    })
}

pub(super) fn parse_decimal(raw: &str, field: &str) -> Result<Decimal, ExchangeError> {
    raw.parse::<Decimal>()
        .map_err(|err| ExchangeError::InvalidResponse {
            message: format!("failed to parse decimal field `{field}`: {err}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_order_maps_side_and_status() {
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 7,
            "clientOrderId": "abc",
            "side": "BUY",
            "type": "MARKET",
            "status": "FILLED",
            "origQty": "0.01000000",
            "executedQty": "0.01000000",
            "cummulativeQuoteQty": "100.00000000",
            "transactTime": 1,
            "fills": []
        });

        let order = parse_order(raw).expect("order");
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.order_id, Some(7));
        assert_eq!(
            order.executed_qty,
            Some(Decimal::from_str_exact("0.01000000").expect("decimal"))
        );
        assert_eq!(order.fills, Some(Vec::new()));
    }

    // A FULL response reports executed quantity, quote quantity, and fills;
    // everything downstream may rely on Some values here.
    #[test]
    fn parse_order_full_response_reports_fills_and_quantities() {
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 7,
            "clientOrderId": "abc",
            "side": "BUY",
            "type": "MARKET",
            "status": "FILLED",
            "origQty": "0.01000000",
            "executedQty": "0.01000000",
            "cummulativeQuoteQty": "100.00000000",
            "transactTime": 1,
            "fills": [
                {"price": "10000.0", "qty": "0.01", "commission": "0.00001", "commissionAsset": "BTC", "tradeId": 42}
            ]
        });

        let order = parse_order(raw).expect("order");
        let fills = order.fills.as_ref().expect("fills reported");
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].commission_asset.as_deref(), Some("BTC"));
        assert_eq!(
            order.net_base_qty_after_base_fees("BTC"),
            Some(Decimal::from_str_exact("0.00999").expect("decimal"))
        );
        assert_eq!(
            order.average_price(),
            Some(Decimal::from_str_exact("10000").expect("decimal"))
        );
    }

    // An ACK response carries no fill data at all: quantities and fills must
    // surface as None (not zero) so callers cannot mistake it for a zero fill.
    #[test]
    fn parse_order_ack_response_reports_missing_fields_as_none() {
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 7,
            "clientOrderId": "abc",
            "side": "BUY",
            "type": "MARKET",
            "transactTime": 1
        });

        let order = parse_order(raw).expect("order");
        assert_eq!(order.executed_qty, None);
        assert_eq!(order.cumulative_quote_qty, None);
        assert_eq!(order.fills, None);
        assert_eq!(order.average_price(), None);
        assert_eq!(order.net_base_qty_after_base_fees("BTC"), None);
        assert_eq!(order.status, OrderStatus::Unknown);
    }

    #[test]
    fn parse_symbol_rules_reads_known_filters_and_skips_malformed_ones() {
        let info = BinanceSymbolInfo {
            symbol: "BTCUSDT".to_string(),
            base_asset: "BTC".to_string(),
            quote_asset: "USDT".to_string(),
            filters: vec![
                serde_json::json!({"filterType": "LOT_SIZE", "minQty": "0.001", "maxQty": "10", "stepSize": "0.001"}),
                serde_json::json!({"filterType": "NOTIONAL", "minNotional": "5"}),
                serde_json::json!({"minQty": "9.9"}),
                serde_json::json!({"filterType": "SOME_FUTURE_FILTER", "value": "1"}),
            ],
        };

        let rules = parse_symbol_rules(info).expect("rules");
        assert_eq!(
            rules.min_qty,
            Some(Decimal::from_str_exact("0.001").expect("decimal"))
        );
        assert_eq!(rules.max_qty, Some(Decimal::from(10)));
        assert_eq!(rules.min_notional, Some(Decimal::from(5)));
        // The malformed filter (no filterType) must not leak its minQty in.
        assert_eq!(
            rules.effective_market_min_qty(),
            Some(Decimal::from_str_exact("0.001").expect("decimal"))
        );
    }

    // The Spot testnet renders MARKET_LOT_SIZE's "no constraint" limits as
    // "0.00000000": zero-valued limits must read as absent so the
    // effective_* helpers fall back to LOT_SIZE instead of handing a zero
    // step to quantity rounding.
    #[test]
    fn zero_valued_filter_limits_are_treated_as_absent() {
        let info = BinanceSymbolInfo {
            symbol: "BTCUSDT".to_string(),
            base_asset: "BTC".to_string(),
            quote_asset: "USDT".to_string(),
            filters: vec![
                serde_json::json!({"filterType": "LOT_SIZE", "minQty": "0.00001000", "maxQty": "9000.00000000", "stepSize": "0.00001000"}),
                serde_json::json!({"filterType": "MARKET_LOT_SIZE", "minQty": "0.00000000", "maxQty": "141.67845966", "stepSize": "0.00000000"}),
            ],
        };

        let rules = parse_symbol_rules(info).expect("rules");
        assert_eq!(rules.market_min_qty, None);
        assert_eq!(rules.market_step_size, None);
        assert_eq!(
            rules.market_max_qty,
            Some(Decimal::from_str_exact("141.67845966").expect("decimal"))
        );
        assert_eq!(
            rules.effective_market_step_size(),
            Some(Decimal::from_str_exact("0.00001000").expect("decimal"))
        );
        assert_eq!(
            rules.effective_market_min_qty(),
            Some(Decimal::from_str_exact("0.00001000").expect("decimal"))
        );
    }

    // Query/openOrders-style responses report quantities but never fills:
    // net quantity must be None instead of silently skipping fee deduction.
    #[test]
    fn parse_order_query_response_without_fills_reports_none_net_qty() {
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 7,
            "clientOrderId": "abc",
            "side": "SELL",
            "type": "MARKET",
            "status": "FILLED",
            "executedQty": "0.01000000",
            "cummulativeQuoteQty": "100.00000000"
        });

        let order = parse_order(raw).expect("order");
        assert_eq!(
            order.executed_qty,
            Some(Decimal::from_str_exact("0.01000000").expect("decimal"))
        );
        assert_eq!(order.fills, None);
        assert_eq!(order.net_base_qty_after_base_fees("BTC"), None);
        assert_eq!(
            order.average_price(),
            Some(Decimal::from_str_exact("10000").expect("decimal"))
        );
    }
}
