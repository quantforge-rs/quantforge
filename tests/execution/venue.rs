//! A venue that knows the orders it is told it holds, and nothing else:
//! it has no way to take an order, so a test that resumes through it can
//! only have asked.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use quantforge::model::futures::{
    AccountSnapshot, ClientOrderId, FeeRates, FuturesFill, FuturesOrder, InstrumentSettings,
    LedgerRecord, LeverageBracket, Liquidation, LiquidationGap, MarginMode, OrderLookup,
    OrderLookupGuarantee, PositionSnapshot, TimeWindow,
};
use quantforge::model::{Environment, ExchangeId, InstrumentId, Leverage};
use quantforge::ports::futures::{
    AccountStateConnector, FuturesCapabilities, FuturesConnector, OrderQueryConnector,
};
use quantforge::ports::{Clock, ExchangeError};
use rust_decimal::Decimal;

use crate::support::Manual;

/// See the module docs.
#[derive(Debug)]
pub(crate) struct Venue<'c> {
    pub(crate) exchange: ExchangeId,
    pub(crate) clock: &'c Manual,
    /// Whether a lookup that finds nothing proves anything here.
    pub(crate) verified: bool,
    pub(crate) orders: Mutex<Vec<FuturesOrder>>,
    pub(crate) fills: Mutex<Vec<FuturesFill>>,
    /// The position the venue reports, flat unless set.
    pub(crate) position: Mutex<Option<PositionSnapshot>>,
    pub(crate) lookups: AtomicUsize,
}

impl<'c> Venue<'c> {
    pub(crate) fn new(exchange: ExchangeId, clock: &'c Manual) -> Self {
        Self {
            exchange,
            clock,
            verified: true,
            orders: Mutex::new(Vec::new()),
            fills: Mutex::new(Vec::new()),
            position: Mutex::new(None),
            lookups: AtomicUsize::new(0),
        }
    }

    pub(crate) fn holds(&self, order: FuturesOrder, fills: Vec<FuturesFill>) {
        self.orders.lock().expect("orders").push(order);
        self.fills.lock().expect("fills").extend(fills);
    }

    pub(crate) fn asked(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }

    fn no_test_asks(&self, what: &str) -> ExchangeError {
        ExchangeError::InvalidRequest {
            message: format!("no test asks {what}"),
        }
    }
}

impl FuturesConnector for Venue<'_> {
    fn exchange_id(&self) -> ExchangeId {
        self.exchange
    }

    fn environment(&self) -> Environment {
        Environment::Demo
    }

    fn capabilities(&self) -> FuturesCapabilities {
        let mut capabilities = FuturesCapabilities::default();
        capabilities.order_lookup = OrderLookupGuarantee {
            visibility_verified: self.verified,
            unfilled_retention_ms: 7 * 86_400_000,
        };
        capabilities
    }
}

#[async_trait]
impl OrderQueryConnector for Venue<'_> {
    async fn order(
        &self,
        instrument: &InstrumentId,
        client_order_id: &ClientOrderId,
    ) -> Result<OrderLookup, ExchangeError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let wanted = client_order_id.to_string();
        let found = self
            .orders
            .lock()
            .expect("orders")
            .iter()
            .find(|order| {
                &order.instrument == instrument && order.client_id.as_deref() == Some(&wanted)
            })
            .cloned();
        Ok(match found {
            Some(order) => OrderLookup::Found(Box::new(order)),
            None => OrderLookup::NotFound {
                queried_at_ms: self.clock.now_ms(),
            },
        })
    }

    async fn open_orders(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<Vec<FuturesOrder>, ExchangeError> {
        Err(self.no_test_asks("the open orders"))
    }

    async fn fills(
        &self,
        instrument: &InstrumentId,
        window: TimeWindow,
    ) -> Result<Vec<FuturesFill>, ExchangeError> {
        let fills = self.fills.lock().expect("fills");
        Ok(fills
            .iter()
            .filter(|fill| {
                &fill.instrument == instrument
                    && (window.start_ms()..=window.end_ms()).contains(&fill.time_ms)
            })
            .cloned()
            .collect())
    }

    async fn ledger(&self, _window: TimeWindow) -> Result<Vec<LedgerRecord>, ExchangeError> {
        Err(self.no_test_asks("the ledger"))
    }
}

#[async_trait]
impl AccountStateConnector for Venue<'_> {
    async fn account(&self) -> Result<AccountSnapshot, ExchangeError> {
        Err(self.no_test_asks("the account"))
    }

    async fn positions(&self) -> Result<Vec<PositionSnapshot>, ExchangeError> {
        Err(self.no_test_asks("every position"))
    }

    async fn position(&self, instrument: &InstrumentId) -> Result<PositionSnapshot, ExchangeError> {
        let held = self.position.lock().expect("position").clone();
        Ok(held.unwrap_or(PositionSnapshot {
            instrument: instrument.clone(),
            exposure: None,
            notional: None,
            leverage: Leverage::new(Decimal::from(5)).expect("a leverage"),
            margin_mode: MarginMode::Cross,
            mark_price: None,
            liquidation: Liquidation::Unavailable(LiquidationGap::ReportedEmpty),
            unrealized_pnl: None,
            margin: None,
            observed_at_ms: self.clock.now_ms(),
        }))
    }

    async fn instrument_settings(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<InstrumentSettings, ExchangeError> {
        Err(self.no_test_asks("the instrument's settings"))
    }

    async fn leverage_brackets(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<Vec<LeverageBracket>, ExchangeError> {
        Err(self.no_test_asks("the leverage brackets"))
    }

    async fn fee_rates(&self, _instrument: &InstrumentId) -> Result<FeeRates, ExchangeError> {
        Err(self.no_test_asks("the fee rates"))
    }
}
