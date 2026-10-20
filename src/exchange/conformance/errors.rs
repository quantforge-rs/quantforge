//! The error kinds: each refusal the contract names, provoked on the
//! exchange where it can be, and scripted where it cannot.

use rust_decimal::Decimal;

use super::support::{Ctx, Fault, Verdict, client_order_id, ensure, free};
use crate::exchange::{ApiCredentials, ConnectorConfig};
use crate::ports::{ExchangeError, SignedConnectors};
use crate::{
    CancelOrderRequest, ExchangeErrorKind, ExchangeOrder, KlineRequest, MarketOrderRequest,
    OrderQueryRequest, Side, Symbol,
};

/// A symbol no exchange lists.
const UNLISTED: &str = "QFNOSUCHSYMBOL";
/// Credentials no exchange issued. Short and plain, so that no secret
/// scanner mistakes them for real ones.
const REJECTED_KEY: &str = "invalid";
const REJECTED_SECRET: &str = "invalid";
/// The quote amount of the orders the scripted faults answer, which no
/// exchange ever sees.
const SCRIPTED_QUOTE: Decimal = Decimal::TEN;

pub(super) async fn insufficient_balance(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    let rules = ctx.rules().await?;
    let min_notional = rules
        .min_notional
        .ok_or_else(|| Fault::Failed("the symbol rules report no minimum notional".to_string()))?;
    let free_quote = free(&ctx.balances().await?, &rules.quote_asset);
    // Just past the balance, so that no maximum-quantity rule can refuse
    // the order first.
    let amount = min_notional
        .checked_mul(Decimal::TWO)
        .and_then(|margin| free_quote.checked_add(margin))
        .ok_or_else(|| Fault::Failed("the order amount overflows".to_string()))?;
    let result = signed
        .trading
        .submit_market_order(&buy_for(ctx, amount))
        .await;
    ctx.expect_kind(
        &format!(
            "a market buy for {amount} {}, more than the free {free_quote}",
            rules.quote_asset
        ),
        result,
        ExchangeErrorKind::InsufficientBalance,
    )?;
    Ok(None)
}

pub(super) async fn unknown_symbol(ctx: &Ctx<'_>) -> Verdict {
    let unlisted = Symbol::new(UNLISTED).map_err(|err| Fault::Failed(err.to_string()))?;
    let klines = ctx
        .market_data
        .fetch_klines(&KlineRequest {
            symbol: unlisted.clone(),
            interval: ctx.target.interval,
            start_time_ms: None,
            end_time_ms: None,
            limit: Some(1),
        })
        .await;
    ctx.expect_kind(
        "fetching klines for an unlisted symbol",
        klines,
        ExchangeErrorKind::UnknownSymbol,
    )?;
    if ctx.capabilities.market_data.symbol_rules {
        let rules = ctx.market_data.fetch_symbol_rules(&unlisted).await;
        ctx.expect_kind(
            "fetching the rules of an unlisted symbol",
            rules,
            ExchangeErrorKind::UnknownSymbol,
        )?;
    }
    Ok(None)
}

pub(super) async fn unknown_order(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    let anchor = ctx.anchor().await?;
    let never_used = client_order_id(&anchor, "none");
    let query = signed
        .trading
        .query_order(&OrderQueryRequest {
            symbol: ctx.target.symbol.clone(),
            order_id: None,
            client_order_id: Some(never_used.clone()),
        })
        .await;
    ctx.expect_kind(
        "looking up a client order id never used",
        query,
        ExchangeErrorKind::UnknownOrder,
    )?;
    let cancel = signed
        .trading
        .cancel_order(&CancelOrderRequest {
            symbol: ctx.target.symbol.clone(),
            order_id: None,
            client_order_id: Some(never_used),
        })
        .await;
    ctx.expect_kind(
        "cancelling a client order id never used",
        cancel,
        ExchangeErrorKind::UnknownOrder,
    )?;
    Ok(None)
}

pub(super) async fn unauthorized(ctx: &Ctx<'_>) -> Verdict {
    let connectors = (ctx.target.factory)(ConnectorConfig {
        base_url: ctx.target.base_url.clone(),
        credentials: Some(ApiCredentials {
            api_key: REJECTED_KEY.to_string(),
            secret: REJECTED_SECRET.to_string(),
        }),
    })
    .map_err(|err| {
        Fault::Failed(format!(
            "building connectors with rejected credentials failed: {err}"
        ))
    })?;
    ensure!(
        !connectors.market_data.is_production(),
        "the rejected credentials' connectors report a production endpoint; refusing to use them"
    );
    let Some(rejected) = connectors.signed() else {
        return Err(Fault::Failed(
            "credentials built no signed connectors".to_string(),
        ));
    };
    let result = rejected.account.account_balances().await;
    ctx.expect_kind(
        "reading the balances with credentials the exchange never issued",
        result,
        ExchangeErrorKind::Unauthorized,
    )?;
    Ok(None)
}

// The scripted faults. Each transcript answers the requests below, in this
// order, with the fault the message names.

pub(super) async fn clock_skew(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    for when in [
        "outside the exchange's receive window",
        "ahead of the exchange clock",
    ] {
        let result = signed.account.account_balances().await;
        ctx.expect_kind(
            &format!("a signed request timestamped {when}"),
            result,
            ExchangeErrorKind::ClockSkew,
        )?;
    }
    Ok(None)
}

pub(super) async fn rate_limited(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    expect_kinds(
        ctx,
        signed,
        &[
            (
                Request::Klines,
                "fetching klines over the request-weight limit",
            ),
            (Request::Klines, "fetching klines from a banned address"),
            (
                Request::Balances,
                "reading the balances past the firewall's limit",
            ),
            (Request::Order, "placing an order over the order-rate limit"),
        ],
        ExchangeErrorKind::RateLimited,
    )
    .await
}

pub(super) async fn unavailable(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    expect_kinds(
        ctx,
        signed,
        &[
            (
                Request::Klines,
                "fetching klines the exchange could not process",
            ),
            (
                Request::Balances,
                "reading the balances from an overloaded exchange",
            ),
        ],
        ExchangeErrorKind::Unavailable,
    )
    .await
}

pub(super) async fn outcome_unknown(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    expect_kinds(
        ctx,
        signed,
        &[
            (
                Request::Klines,
                "fetching klines from an exchange failing mid-request",
            ),
            (Request::Order, "placing an order the exchange timed out on"),
            (
                Request::Balances,
                "reading the balances when a server error carries a code asking for a retry",
            ),
        ],
        ExchangeErrorKind::OutcomeUnknown,
    )
    .await
}

pub(super) async fn malformed_response(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    expect_kinds(
        ctx,
        signed,
        &[
            (Request::Klines, "reading klines of the wrong shape"),
            (Request::Balances, "reading balances that are not JSON"),
        ],
        ExchangeErrorKind::InvalidResponse,
    )
    .await
}

/// A request a scripted fault answers.
#[derive(Clone, Copy)]
enum Request {
    /// The latest candle.
    Klines,
    /// The account's balances.
    Balances,
    /// A market buy for [`SCRIPTED_QUOTE`].
    Order,
}

/// Sends each request in turn, expecting each to fail as `kind`.
async fn expect_kinds(
    ctx: &Ctx<'_>,
    signed: SignedConnectors<'_>,
    requests: &[(Request, &str)],
    kind: ExchangeErrorKind,
) -> Verdict {
    for &(request, what) in requests {
        let result: Result<(), ExchangeError> = match request {
            Request::Klines => ctx
                .market_data
                .fetch_klines(&KlineRequest {
                    symbol: ctx.target.symbol.clone(),
                    interval: ctx.target.interval,
                    start_time_ms: None,
                    end_time_ms: None,
                    limit: Some(1),
                })
                .await
                .map(drop),
            Request::Balances => signed.account.account_balances().await.map(drop),
            Request::Order => signed
                .trading
                .submit_market_order(&buy_for(ctx, SCRIPTED_QUOTE))
                .await
                .map(|_: ExchangeOrder| ()),
        };
        ctx.expect_kind(what, result, kind)?;
    }
    Ok(None)
}

fn buy_for(ctx: &Ctx<'_>, quote: Decimal) -> MarketOrderRequest {
    MarketOrderRequest {
        symbol: ctx.target.symbol.clone(),
        side: Side::Buy,
        quantity: None,
        quote_order_qty: Some(quote),
        new_client_order_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // Reports and failure messages name kinds by these; two kinds sharing
    // a name would read as one.
    #[test]
    fn every_error_kind_has_its_own_name() {
        let kinds = [
            ExchangeErrorKind::Transport,
            ExchangeErrorKind::InvalidResponse,
            ExchangeErrorKind::InvalidRequest,
            ExchangeErrorKind::MissingCredentials,
            ExchangeErrorKind::Unauthorized,
            ExchangeErrorKind::ClockSkew,
            ExchangeErrorKind::RateLimited,
            ExchangeErrorKind::Unavailable,
            ExchangeErrorKind::OutcomeUnknown,
            ExchangeErrorKind::UnknownSymbol,
            ExchangeErrorKind::UnknownOrder,
            ExchangeErrorKind::InsufficientBalance,
            ExchangeErrorKind::RuleViolation,
            ExchangeErrorKind::Rejected,
            ExchangeErrorKind::Other,
        ];
        let names: HashSet<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(names.len(), kinds.len());
        for kind in kinds {
            // Exhaustive on purpose: a new kind stops this module from
            // compiling until it is listed above.
            match kind {
                ExchangeErrorKind::Transport
                | ExchangeErrorKind::InvalidResponse
                | ExchangeErrorKind::InvalidRequest
                | ExchangeErrorKind::MissingCredentials
                | ExchangeErrorKind::Unauthorized
                | ExchangeErrorKind::ClockSkew
                | ExchangeErrorKind::RateLimited
                | ExchangeErrorKind::Unavailable
                | ExchangeErrorKind::OutcomeUnknown
                | ExchangeErrorKind::UnknownSymbol
                | ExchangeErrorKind::UnknownOrder
                | ExchangeErrorKind::InsufficientBalance
                | ExchangeErrorKind::RuleViolation
                | ExchangeErrorKind::Rejected
                | ExchangeErrorKind::Other => {}
            }
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn every_error_variant_reports_its_kind() {
        let io = std::io::Error::other("refused");
        for (err, kind) in [
            (ExchangeError::transport(io), ExchangeErrorKind::Transport),
            (
                ExchangeError::api(ExchangeErrorKind::ClockSkew, Some(-1), "late"),
                ExchangeErrorKind::ClockSkew,
            ),
            (
                ExchangeError::MissingCredentials,
                ExchangeErrorKind::MissingCredentials,
            ),
            (
                ExchangeError::InvalidResponse {
                    message: "garbled".to_string(),
                },
                ExchangeErrorKind::InvalidResponse,
            ),
            (
                ExchangeError::InvalidRequest {
                    message: "no quantity".to_string(),
                },
                ExchangeErrorKind::InvalidRequest,
            ),
        ] {
            assert_eq!(err.kind(), kind, "{err}");
        }
        // The kind changes nothing that callers already print.
        assert_eq!(
            ExchangeError::api(ExchangeErrorKind::RateLimited, Some(429), "slow down").to_string(),
            "api error: code=Some(429) message=slow down"
        );
    }
}
