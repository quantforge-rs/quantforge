//! The checks: their names, what each one asserts, and what each needs to
//! run.

use super::support::{Ctx, Verdict};
use super::{clock, errors, klines, orders, rules};
use crate::ExchangeCapabilities;

/// One check of the contract. [`Check::ALL`] lists them in report order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Check {
    ServerTime,
    CandleNormalization,
    ClosedBars,
    KlineBounds,
    KlinePaging,
    KlineGaps,
    SymbolRules,
    OrderLifecycle,
    RuleViolations,
    InsufficientBalance,
    UnknownSymbol,
    UnknownOrder,
    Unauthorized,
    ClockSkew,
    RateLimited,
    Unavailable,
    OutcomeUnknown,
    MalformedResponse,
}

/// A capability a check needs, under the name a skipped check reports.
type Need = (&'static str, fn(&ExchangeCapabilities) -> bool);

const SERVER_TIME: Need = ("server time", |caps| caps.market_data.server_time);
const SYMBOL_RULES: Need = ("symbol rules", |caps| caps.market_data.symbol_rules);
const BY_QUOTE: Need = ("market orders sized by quote quantity", |caps| {
    caps.orders.market_by_quote_quantity
});
const BY_BASE: Need = ("market orders sized by base quantity", |caps| {
    caps.orders.market_by_base_quantity
});
const CLIENT_IDS: Need = ("client order ids", |caps| caps.orders.client_order_ids);

impl Check {
    /// Every check, in the order reports list them.
    pub const ALL: &'static [Check] = &[
        Self::ServerTime,
        Self::CandleNormalization,
        Self::ClosedBars,
        Self::KlineBounds,
        Self::KlinePaging,
        Self::KlineGaps,
        Self::SymbolRules,
        Self::OrderLifecycle,
        Self::RuleViolations,
        Self::InsufficientBalance,
        Self::UnknownSymbol,
        Self::UnknownOrder,
        Self::Unauthorized,
        Self::ClockSkew,
        Self::RateLimited,
        Self::Unavailable,
        Self::OutcomeUnknown,
        Self::MalformedResponse,
    ];

    /// The check's stable name, such as `market_data.kline_paging`; the
    /// repository names each check's fixture file after it.
    pub const fn id(self) -> &'static str {
        match self {
            Self::ServerTime => "market_data.server_time",
            Self::CandleNormalization => "market_data.candle_normalization",
            Self::ClosedBars => "market_data.closed_bars",
            Self::KlineBounds => "market_data.kline_bounds",
            Self::KlinePaging => "market_data.kline_paging",
            Self::KlineGaps => "market_data.kline_gaps",
            Self::SymbolRules => "market_data.symbol_rules",
            Self::OrderLifecycle => "orders.lifecycle",
            Self::RuleViolations => "orders.rule_violations",
            Self::InsufficientBalance => "errors.insufficient_balance",
            Self::UnknownSymbol => "errors.unknown_symbol",
            Self::UnknownOrder => "errors.unknown_order",
            Self::Unauthorized => "errors.unauthorized",
            Self::ClockSkew => "errors.clock_skew",
            Self::RateLimited => "errors.rate_limited",
            Self::Unavailable => "errors.unavailable",
            Self::OutcomeUnknown => "errors.outcome_unknown",
            Self::MalformedResponse => "errors.malformed_response",
        }
    }

    /// The check with this [`Check::id`].
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|check| check.id() == id)
    }

    /// What the check asserts, in one line.
    pub const fn summary(self) -> &'static str {
        match self {
            Self::ServerTime => {
                "the exchange clock reads UTC milliseconds and never runs backwards"
            }
            Self::CandleNormalization => {
                "candles sit on the interval grid with `close = open + step - 1`, ascending and \
                 unique, with consistent prices on the tick grid"
            }
            Self::ClosedBars => {
                "only the newest candle may still be forming, and it keeps its full-interval close \
                 time"
            }
            Self::KlineBounds => {
                "both window bounds are inclusive on open time, `limit` is honored, and a limit \
                 above the maximum is capped"
            }
            Self::KlinePaging => {
                "paging by last open + step reproduces a window exactly, decimals included"
            }
            Self::KlineGaps => {
                "a window without bars is empty, and one opening before the first bar skips to the \
                 oldest"
            }
            Self::SymbolRules => {
                "rule limits are positive, minimums stay within maximums, and a quantity step \
                 and a price tick are present"
            }
            Self::OrderLifecycle => {
                "a market buy fills with its fills reported, reads back by both ids, is not open, \
                 refuses a cancel, shows in trades and balances, and sells back rounded to the step"
            }
            Self::RuleViolations => {
                "an order below the minimum notional or off the quantity step fails as \
                 `rule_violation`"
            }
            Self::InsufficientBalance => {
                "an order beyond the free balance fails as `insufficient_balance`"
            }
            Self::UnknownSymbol => "an unlisted symbol fails as `unknown_symbol`",
            Self::UnknownOrder => {
                "looking up or cancelling an order that never existed fails as `unknown_order`"
            }
            Self::Unauthorized => "credentials the exchange never issued fail as `unauthorized`",
            Self::ClockSkew => "a request outside the exchange's time window fails as `clock_skew`",
            Self::RateLimited => "rate-limit and ban responses fail as `rate_limited`",
            Self::Unavailable => {
                "an exchange refusing work it did not start fails as `unavailable`, safe to retry"
            }
            Self::OutcomeUnknown => {
                "an exchange failing mid-request fails as `outcome_unknown`, even when its code \
                 asks for a retry, so an order is looked up before it is sent again"
            }
            Self::MalformedResponse => "a response that cannot be read fails as `invalid_response`",
        }
    }

    /// Whether the check can run against a live exchange. The others script
    /// faults the exchange cannot be made to produce, so they run only
    /// against a replay.
    pub const fn runs_live(self) -> bool {
        !matches!(
            self,
            Self::ClockSkew
                | Self::RateLimited
                | Self::Unavailable
                | Self::OutcomeUnknown
                | Self::MalformedResponse
        )
    }

    /// Whether the check needs the account's credentials.
    pub const fn signed(self) -> bool {
        matches!(
            self,
            Self::OrderLifecycle
                | Self::RuleViolations
                | Self::InsufficientBalance
                | Self::UnknownOrder
                | Self::ClockSkew
                | Self::RateLimited
                | Self::Unavailable
                | Self::OutcomeUnknown
                | Self::MalformedResponse
        )
    }

    fn needs(self) -> &'static [Need] {
        match self {
            Self::ServerTime
            | Self::ClosedBars
            | Self::KlineBounds
            | Self::KlinePaging
            | Self::KlineGaps => &[SERVER_TIME],
            Self::CandleNormalization => &[SERVER_TIME, SYMBOL_RULES],
            Self::SymbolRules => &[SYMBOL_RULES],
            Self::OrderLifecycle => &[SERVER_TIME, SYMBOL_RULES, BY_QUOTE, BY_BASE],
            Self::RuleViolations => &[SYMBOL_RULES, BY_QUOTE, BY_BASE],
            Self::InsufficientBalance => &[SYMBOL_RULES, BY_QUOTE],
            Self::UnknownOrder => &[SERVER_TIME, CLIENT_IDS],
            Self::RateLimited | Self::OutcomeUnknown => &[BY_QUOTE],
            Self::UnknownSymbol
            | Self::Unauthorized
            | Self::ClockSkew
            | Self::Unavailable
            | Self::MalformedResponse => &[],
        }
    }

    /// The first capability the check needs that `capabilities` lacks.
    pub(super) fn missing_capability(
        self,
        capabilities: &ExchangeCapabilities,
    ) -> Option<&'static str> {
        self.needs()
            .iter()
            .find(|(_, supported)| !supported(capabilities))
            .map(|(name, _)| *name)
    }

    pub(super) async fn verdict(self, ctx: &Ctx<'_>) -> Verdict {
        match self {
            Self::ServerTime => clock::server_time(ctx).await,
            Self::CandleNormalization => klines::candle_normalization(ctx).await,
            Self::ClosedBars => clock::closed_bars(ctx).await,
            Self::KlineBounds => klines::kline_bounds(ctx).await,
            Self::KlinePaging => klines::kline_paging(ctx).await,
            Self::KlineGaps => klines::kline_gaps(ctx).await,
            Self::SymbolRules => rules::symbol_rules(ctx).await,
            Self::OrderLifecycle => orders::lifecycle(ctx).await,
            Self::RuleViolations => rules::rule_violations(ctx).await,
            Self::InsufficientBalance => errors::insufficient_balance(ctx).await,
            Self::UnknownSymbol => errors::unknown_symbol(ctx).await,
            Self::UnknownOrder => errors::unknown_order(ctx).await,
            Self::Unauthorized => errors::unauthorized(ctx).await,
            Self::ClockSkew => errors::clock_skew(ctx).await,
            Self::RateLimited => errors::rate_limited(ctx).await,
            Self::Unavailable => errors::unavailable(ctx).await,
            Self::OutcomeUnknown => errors::outcome_unknown(ctx).await,
            Self::MalformedResponse => errors::malformed_response(ctx).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_check_is_listed_once_under_a_unique_id() {
        let listed: HashSet<Check> = Check::ALL.iter().copied().collect();
        assert_eq!(listed.len(), Check::ALL.len(), "a check is listed twice");
        assert_eq!(listed.len(), 18);
        let ids: HashSet<&str> = Check::ALL.iter().map(|check| check.id()).collect();
        assert_eq!(ids.len(), Check::ALL.len(), "two checks share an id");

        for check in listed {
            assert_eq!(Check::from_id(check.id()), Some(check));
            // Exhaustive on purpose: a new check stops this module from
            // compiling until it is listed in ALL and counted above.
            match check {
                Check::ServerTime
                | Check::CandleNormalization
                | Check::ClosedBars
                | Check::KlineBounds
                | Check::KlinePaging
                | Check::KlineGaps
                | Check::SymbolRules
                | Check::OrderLifecycle
                | Check::RuleViolations
                | Check::InsufficientBalance
                | Check::UnknownSymbol
                | Check::UnknownOrder
                | Check::Unauthorized
                | Check::ClockSkew
                | Check::RateLimited
                | Check::Unavailable
                | Check::OutcomeUnknown
                | Check::MalformedResponse => {}
            }
        }
        assert_eq!(Check::from_id("market_data.nosuch"), None);
    }

    // The matrix doc is a table, so a summary must not break a cell.
    #[test]
    fn summaries_fit_a_table_cell() {
        for check in Check::ALL {
            assert!(!check.summary().contains('|'), "{check:?}");
            assert!(!check.summary().ends_with('.'), "{check:?}");
        }
    }

    #[test]
    fn an_exchange_supporting_nothing_skips_every_check_that_needs_a_capability() {
        let nothing = ExchangeCapabilities::default();
        for check in Check::ALL {
            assert_eq!(
                check.missing_capability(&nothing).is_some(),
                !check.needs().is_empty(),
                "{check:?}"
            );
        }
        assert_eq!(
            Check::OrderLifecycle.missing_capability(&nothing),
            Some("server time")
        );
    }
}
