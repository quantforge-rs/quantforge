//! The suite's own controls. A check that compared a list with itself
//! would pass forever, so every check is also run, from its own fixture,
//! against connectors corrupted in the one way that check exists to catch,
//! and must fail naming what broke. The corruptions work on domain values,
//! so they hold for every exchange without code of their own.

use async_trait::async_trait;
use quantforge::exchange::conformance::{Check, Factory, Outcome};
use quantforge::exchange::{self, ExchangeConnectors};
use quantforge::{
    AccountConnector, AccountTrade, AssetBalance, CancelOrderRequest, Candle, ExchangeCapabilities,
    ExchangeConnector, ExchangeError, ExchangeErrorKind, ExchangeId, ExchangeOrder, KlineRequest,
    MarketDataConnector, MarketOrderRequest, OrderQueryRequest, Symbol, SymbolRules, TimestampMs,
    TradingConnector,
};
use rust_decimal::Decimal;
use std::sync::{Arc, Mutex};

use crate::{fixture, replay_check, skip_while_recording};

/// One way to break a connector.
#[derive(Clone, Copy, Debug)]
enum Corruption {
    /// The second clock reading is earlier than the first.
    ClockRunsBackwards,
    /// Every candle closes one millisecond late.
    CloseTimeOffByOne,
    /// The newest of the latest candles opens ten bars in the future.
    FutureBar,
    /// A window bounded at both ends loses its last candle.
    DropLastOfBoundedWindow,
    /// Every page with a limit loses its first candle.
    DropFirstOfEachPage,
    /// A window opening before 2001 comes back empty.
    EmptyBeforeListing,
    /// The minimum quantity exceeds the maximum.
    MinAboveMax,
    /// A placed order reports no fills.
    DropFills,
    /// Errors of one kind are reported as another.
    Rekind {
        from: ExchangeErrorKind,
        to: ExchangeErrorKind,
    },
}

/// The corruption each check exists to catch, and the words its failure
/// must contain.
fn control(check: Check) -> (Corruption, String) {
    use ExchangeErrorKind::*;

    let rekind = |from: ExchangeErrorKind, to: ExchangeErrorKind| {
        (Corruption::Rekind { from, to }, format!("not {from}"))
    };
    let (corruption, expect) = match check {
        Check::ServerTime => (Corruption::ClockRunsBackwards, "ran backwards"),
        Check::CandleNormalization => (
            Corruption::CloseTimeOffByOne,
            "its close time must be its last millisecond",
        ),
        Check::ClosedBars => (Corruption::FutureBar, "opens after the exchange clock"),
        Check::KlineBounds => (
            Corruption::DropLastOfBoundedWindow,
            "both bounds are inclusive",
        ),
        Check::KlinePaging => (Corruption::DropFirstOfEachPage, "differ from the"),
        Check::KlineGaps => (
            Corruption::EmptyBeforeListing,
            "a leading gap must be skipped",
        ),
        Check::SymbolRules => (Corruption::MinAboveMax, "exceeds"),
        Check::OrderLifecycle => (Corruption::DropFills, "reported no fills"),
        Check::RuleViolations => return rekind(RuleViolation, Rejected),
        Check::InsufficientBalance => return rekind(InsufficientBalance, Rejected),
        Check::UnknownSymbol => return rekind(UnknownSymbol, Other),
        Check::UnknownOrder => return rekind(UnknownOrder, Other),
        Check::Unauthorized => return rekind(Unauthorized, Other),
        Check::ClockSkew => return rekind(ClockSkew, Other),
        Check::RateLimited => return rekind(RateLimited, Other),
        Check::Unavailable => return rekind(Unavailable, Other),
        Check::OutcomeUnknown => return rekind(OutcomeUnknown, Other),
        Check::MalformedResponse => return rekind(InvalidResponse, Other),
        // Check is non-exhaustive across crates; a new one fails the test
        // below until it has a control here.
        _ => panic!("{} has no control", check.id()),
    };
    (corruption, expect.to_string())
}

/// `exchange`'s factory, with every connector it builds corrupted.
fn corrupting(exchange: ExchangeId, corruption: Corruption) -> Factory {
    Arc::new(move |config| {
        let inner = exchange::connect(exchange, config)?;
        let corrupted = Arc::new(Corrupted {
            market_data: Arc::clone(&inner.market_data),
            trading: inner.trading.clone(),
            account: inner.account.clone(),
            corruption,
            first_reading: Mutex::new(None),
        });
        Ok(ExchangeConnectors {
            market_data: corrupted.clone(),
            trading: inner
                .trading
                .is_some()
                .then(|| corrupted.clone() as Arc<dyn TradingConnector>),
            account: inner
                .account
                .is_some()
                .then_some(corrupted as Arc<dyn AccountConnector>),
        })
    })
}

struct Corrupted {
    market_data: Arc<dyn MarketDataConnector>,
    trading: Option<Arc<dyn TradingConnector>>,
    account: Option<Arc<dyn AccountConnector>>,
    corruption: Corruption,
    first_reading: Mutex<Option<TimestampMs>>,
}

/// 2001-01-01T00:00:00Z.
const LISTED_BY: TimestampMs = 978_307_200_000;

impl Corrupted {
    fn rekind<T>(&self, result: Result<T, ExchangeError>) -> Result<T, ExchangeError> {
        match (self.corruption, result) {
            (Corruption::Rekind { from, to }, Err(err)) if err.kind() == from => {
                Err(ExchangeError::api(to, None, err.to_string()))
            }
            (_, result) => result,
        }
    }

    fn candles(&self, request: &KlineRequest, mut candles: Vec<Candle>) -> Vec<Candle> {
        let step = request.interval.step_ms();
        match self.corruption {
            Corruption::CloseTimeOffByOne => {
                for candle in &mut candles {
                    candle.close_time_ms += 1;
                }
            }
            Corruption::FutureBar if request.start_time_ms.is_none() => {
                if let Some(newest) = candles.last_mut() {
                    newest.open_time_ms += 10 * step;
                    newest.close_time_ms += 10 * step;
                }
            }
            Corruption::DropLastOfBoundedWindow
                if request.start_time_ms.is_some()
                    && request.end_time_ms.is_some()
                    && request.limit.is_none() =>
            {
                candles.pop();
            }
            Corruption::DropFirstOfEachPage if request.limit.is_some() && !candles.is_empty() => {
                candles.remove(0);
            }
            Corruption::EmptyBeforeListing
                if request.start_time_ms.is_some_and(|start| start < LISTED_BY) =>
            {
                candles.clear();
            }
            _ => {}
        }
        candles
    }

    fn trading(&self) -> Result<&dyn TradingConnector, ExchangeError> {
        self.trading
            .as_deref()
            .ok_or(ExchangeError::MissingCredentials)
    }

    fn account(&self) -> Result<&dyn AccountConnector, ExchangeError> {
        self.account
            .as_deref()
            .ok_or(ExchangeError::MissingCredentials)
    }
}

impl ExchangeConnector for Corrupted {
    fn exchange_id(&self) -> ExchangeId {
        self.market_data.exchange_id()
    }

    fn capabilities(&self) -> ExchangeCapabilities {
        self.market_data.capabilities()
    }

    fn is_production(&self) -> bool {
        self.market_data.is_production()
    }
}

#[async_trait]
impl MarketDataConnector for Corrupted {
    async fn fetch_klines(&self, request: &KlineRequest) -> Result<Vec<Candle>, ExchangeError> {
        let result = self.market_data.fetch_klines(request).await;
        self.rekind(result.map(|candles| self.candles(request, candles)))
    }

    async fn fetch_symbol_rules(&self, symbol: &Symbol) -> Result<SymbolRules, ExchangeError> {
        let result = self.market_data.fetch_symbol_rules(symbol).await;
        self.rekind(result.map(|mut rules| {
            if let Corruption::MinAboveMax = self.corruption {
                rules.max_qty = Some(Decimal::ONE);
                rules.min_qty = Some(Decimal::TWO);
            }
            rules
        }))
    }

    async fn server_time(&self) -> Result<TimestampMs, ExchangeError> {
        let result = self.market_data.server_time().await;
        self.rekind(result.map(|reading| {
            let Corruption::ClockRunsBackwards = self.corruption else {
                return reading;
            };
            let mut first = self.first_reading.lock().expect("first reading");
            match *first {
                Some(first) => first - 1,
                None => {
                    *first = Some(reading);
                    reading
                }
            }
        }))
    }
}

#[async_trait]
impl TradingConnector for Corrupted {
    async fn submit_market_order(
        &self,
        request: &MarketOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        let result = self.trading()?.submit_market_order(request).await;
        self.rekind(result.map(|mut order| {
            if let Corruption::DropFills = self.corruption {
                order.fills = None;
            }
            order
        }))
    }

    async fn cancel_order(
        &self,
        request: &CancelOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        let result = self.trading()?.cancel_order(request).await;
        self.rekind(result)
    }

    async fn query_order(
        &self,
        request: &OrderQueryRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        let result = self.trading()?.query_order(request).await;
        self.rekind(result)
    }

    async fn open_orders(
        &self,
        symbol: Option<&Symbol>,
    ) -> Result<Vec<ExchangeOrder>, ExchangeError> {
        let result = self.trading()?.open_orders(symbol).await;
        self.rekind(result)
    }
}

#[async_trait]
impl AccountConnector for Corrupted {
    async fn account_balances(&self) -> Result<Vec<AssetBalance>, ExchangeError> {
        let result = self.account()?.account_balances().await;
        self.rekind(result)
    }

    async fn recent_trades(
        &self,
        symbol: &Symbol,
        limit: usize,
    ) -> Result<Vec<AccountTrade>, ExchangeError> {
        let result = self.account()?.recent_trades(symbol, limit).await;
        self.rekind(result)
    }
}

#[tokio::test]
async fn every_check_fails_when_its_control_corrupts_the_connector() {
    if skip_while_recording("every_check_fails_when_its_control_corrupts_the_connector") {
        return;
    }
    let mut problems = Vec::new();
    for &exchange in exchange::registered() {
        let fixtures = fixture::load_all(exchange);
        for &check in Check::ALL {
            // A check the exchange cannot run has no fixture to corrupt;
            // the fixtures test holds that skip to account.
            let Some(fixture) = fixtures.get(check.id()) else {
                continue;
            };
            let (corruption, expect) = control(check);
            let replayed = replay_check(
                exchange,
                check,
                fixture.interactions.clone(),
                corrupting(exchange, corruption),
            )
            .await;
            match &replayed.outcome {
                Outcome::Failed(reason) if reason.contains(&expect) => {}
                outcome => problems.push(format!(
                    "{} {}: under {corruption:?} it must fail with \"{expect}\", but came to \
                     {outcome:?}",
                    exchange::name(exchange),
                    check.id()
                )),
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn every_check_has_a_control() {
    for &check in Check::ALL {
        let (_, expect) = control(check);
        assert!(!expect.is_empty(), "{}", check.id());
    }
}
