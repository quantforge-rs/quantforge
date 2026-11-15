//! Two venues' product lines, as the policy cases list them
//! (`tests/fixtures/policy_cases.json`): on each a profile named `main`
//! that trades `SOLUSDT`, so that everything a signal names is the same
//! on both, and only the venue and the account tell them apart. With
//! them, the signals, intents, orders and fills the tests record.

use std::path::Path;

use quantforge::model::execution::{
    Change, CycleRecord, EventKey, EventKind, ExecutionScope, ExecutionUnit, Fence, FillRecord,
    IntentRecord, Lease, LeaseAnswer, LeaseHolder, LeaseRequest, NewOperation, OperationKey,
    OperationKind, OrderRecord, OutboxEvent, Record, SignalRecord,
};
use quantforge::model::futures::{
    ActivationEpoch, ClientOrderId, EntryIntent, ExitIntent, ExternalIntent, ExternalSignal,
    FuturesFill, FuturesInstrument, IntentKind, OrderAttempt, OrderLeg, OrderOrigin,
    PolicySnapshot, PositionCycle, PositionCycleId, PositionKey, ProfileIdentity, RecordSource,
    SignalAction,
};
use quantforge::model::{
    Asset, InstrumentId, Money, OperationId, PositionSide, Price, Quantity, VenueOrderId,
    VenueTradeId,
};
use quantforge::protocol::signal::Signal;
use quantforge::server::external_signal;
use quantforge::{ExecutionStore, Side};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::support::T;

/// One product line: who its profile is, what its venue lists, and the
/// symbol its records are about.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Line {
    pub(crate) name: String,
    pub(crate) identity: ProfileIdentity,
    pub(crate) instruments: Vec<FuturesInstrument>,
    #[serde(skip, default = "sol")]
    pub(crate) symbol: String,
}

fn sol() -> String {
    "SOLUSDT".to_string()
}

/// Every product line of the policy cases.
pub(crate) fn lines() -> Vec<Line> {
    #[derive(Deserialize)]
    struct Fixture {
        lines: Vec<Line>,
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("policy_cases.json");
    let text = std::fs::read_to_string(&path).expect("read tests/fixtures/policy_cases.json");
    let fixture: Fixture = serde_json::from_str(&text).expect("the policy cases");
    fixture.lines
}

/// The line named `name`, such as `bybit linear`.
pub(crate) fn line(name: &str) -> Line {
    lines()
        .into_iter()
        .find(|line| line.name == name)
        .unwrap_or_else(|| panic!("the fixture has no line {name}"))
}

/// The two linear lines: one profile name, one symbol, two venues.
pub(crate) fn linear_pair() -> [Line; 2] {
    [line("bybit linear"), line("binance linear")]
}

/// What one delivery brings: the signal, and its canonical payload.
#[derive(Clone, Debug)]
pub(crate) struct Delivered {
    pub(crate) signal: ExternalSignal,
    pub(crate) payload: String,
}

impl Line {
    pub(crate) fn scope(&self) -> ExecutionScope {
        ExecutionScope::of_profile(&self.identity)
    }

    /// The instrument the venue lists under `symbol`.
    pub(crate) fn listed(&self, symbol: &str) -> &FuturesInstrument {
        self.instruments
            .iter()
            .find(|listed| listed.id().symbol.as_str() == symbol)
            .unwrap_or_else(|| panic!("{} lists no {symbol}", self.name))
    }

    /// The same line, about another of its symbols.
    pub(crate) fn trading(&self, symbol: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            ..self.clone()
        }
    }

    pub(crate) fn instrument(&self) -> InstrumentId {
        self.listed(&self.symbol).id().clone()
    }

    pub(crate) fn key(&self) -> PositionKey {
        PositionKey::new(self.identity.account_ref().clone(), self.instrument()).expect("a key")
    }

    /// The policy an entry on the line's instrument is admitted under.
    pub(crate) fn snapshot(&self) -> PolicySnapshot {
        serde_json::from_value(json!({
            "format": 1,
            "profile": self.identity.id(),
            "account": self.identity.account_ref(),
            "revision": 4,
            "instrument": self.instrument(),
            "risk_percent": {"value": "1", "source": "Defaults"},
            "stop_percent": {"value": "3", "source": "Defaults"},
            "leverage": {"value": "5", "source": "Defaults"},
            "capital": {"amount": "15000", "asset": "USDT"},
            "capital_revision": 4,
        }))
        .expect("a policy snapshot")
    }

    /// A delivery read as the webhook route reads one: a `kind` signal of
    /// the alert `source`, on the bar `minute` minutes after [`T`], at
    /// `price`. Both lines' deliveries of one alert are the same bytes.
    pub(crate) fn delivery(&self, kind: &str, source: &str, minute: u32, price: &str) -> Delivered {
        let time = |seconds: u32| {
            format!(
                "2026-10-20T00:{:02}:{:02}Z",
                minute + seconds / 60,
                seconds % 60
            )
        };
        let mut body = json!({
            "version": 1,
            "kind": kind,
            "profile": self.identity.id(),
            "source": source,
            "ticker": format!("{}.P", self.symbol),
            "side": "long",
            "bar_time": time(0),
            "fire_time": time(5),
        });
        if kind == "entry" {
            body["price"] = json!(price);
            body["stop"] = json!("200.00");
        }
        let read = Signal::read(body.to_string().as_bytes()).expect("a signal");
        Delivered {
            signal: external_signal(&read).expect("the model's signal"),
            payload: read.canonical(),
        }
    }

    /// The delivery as its inbox first records it, at `at_ms`.
    pub(crate) fn received(&self, delivered: &Delivered, at_ms: i64) -> SignalRecord {
        SignalRecord::received(
            self.scope(),
            delivered.signal.clone(),
            delivered.payload.clone(),
            at_ms,
        )
        .expect("a signal record")
    }

    /// The intent `delivered` resolves to on the line's instrument,
    /// admitted: an entry under the line's snapshot, an exit under none.
    pub(crate) fn admitted(&self, delivered: &Delivered) -> IntentRecord {
        let signal = &delivered.signal;
        let (kind, policy) = match signal.action {
            SignalAction::Entry {
                expected_price,
                stop,
            } => (
                IntentKind::Entry(EntryIntent {
                    profile: self.identity.id().clone(),
                    signal: signal.id.clone(),
                    signal_time_ms: signal.fire_time_ms,
                    key: self.key(),
                    side: signal.side,
                    expected_price,
                    stop,
                }),
                Some(self.snapshot()),
            ),
            SignalAction::Exit => (
                IntentKind::Exit(ExitIntent {
                    profile: self.identity.id().clone(),
                    signal: signal.id.clone(),
                    signal_time_ms: signal.fire_time_ms,
                    key: self.key(),
                    side: signal.side,
                }),
                None,
            ),
        };
        let mut intent = ExternalIntent::new(kind);
        intent.admit(policy).expect("admitted");
        IntentRecord::new(intent)
    }

    /// What the outbox is told if `record`'s delivery conflicts.
    pub(crate) fn conflict_event(&self, record: &SignalRecord) -> OutboxEvent {
        let word = format!("conflict:{}", record.payload_hash());
        OutboxEvent::pending(
            self.scope(),
            EventKey::of_signal(record.profile(), &word, record.id()),
            EventKind::SignalConflict,
            record.received_at_ms(),
        )
        .of_signal(record.id())
    }

    /// An event about `record`'s signal: `word` names what happened.
    pub(crate) fn signal_event(
        &self,
        record: &SignalRecord,
        kind: EventKind,
        at_ms: i64,
    ) -> OutboxEvent {
        OutboxEvent::pending(
            self.scope(),
            EventKey::of_signal(record.profile(), kind.as_str(), record.id()),
            kind,
            at_ms,
        )
        .of_signal(record.id())
        .on(&self.instrument())
    }

    /// The unit that records `delivered` and admits it, with its intent
    /// and the event that tells of it, in one write.
    pub(crate) fn admission(&self, delivered: &Delivered, at_ms: i64) -> ExecutionUnit {
        let received = self.received(delivered, at_ms);
        let admitted = received.admitted(at_ms).expect("admitted");
        let event = self.signal_event(&admitted, EventKind::SignalAdmitted, at_ms);
        let mut unit = ExecutionUnit::at(at_ms)
            .with(Change::new(Record::Intent(self.admitted(delivered))))
            .with(Change::new(Record::Event(event)));
        unit.delivery = Some(quantforge::model::execution::DeliveryWrite {
            conflict_event: self.conflict_event(&received),
            record: admitted,
        });
        unit
    }

    /// The operation that carries `delivered`'s intent out.
    pub(crate) fn operation(&self, delivered: &Delivered, kind: OperationKind) -> NewOperation {
        NewOperation {
            scope: self.scope(),
            kind,
            key: Some(OperationKey::of_signal(
                self.identity.id(),
                &delivered.signal.id,
                1,
            )),
            instrument: self.instrument(),
            signal: Some(delivered.signal.id.clone()),
            cycle: None,
            replacement: None,
        }
    }

    /// The client order ID of `operation`'s `attempt`th order of `leg`,
    /// under the line's owner tag at `epoch`.
    pub(crate) fn client_order_id(
        &self,
        epoch: ActivationEpoch,
        operation: OperationId,
        leg: OrderLeg,
        attempt: u16,
    ) -> ClientOrderId {
        ClientOrderId::new(
            self.identity.owner_tag().clone(),
            epoch,
            operation,
            leg,
            attempt,
        )
        .expect("a client order id")
    }

    /// The order `id` is sent as, recorded just before it is.
    pub(crate) fn order(
        &self,
        operation: OperationId,
        id: &ClientOrderId,
        at_ms: i64,
    ) -> OrderRecord {
        OrderRecord::recorded(
            self.scope(),
            operation,
            id.to_string(),
            self.instrument(),
            at_ms,
        )
        .expect("an order record")
    }

    /// A fill of `id`'s order: `quantity` SOL at `price`.
    pub(crate) fn fill(
        &self,
        trade: &str,
        id: &ClientOrderId,
        quantity: &str,
        price: &str,
        operation: OperationId,
        cycle: Option<PositionCycleId>,
    ) -> FillRecord {
        let decimal = |text: &str| Decimal::from_str_exact(text).expect("a decimal");
        let usdt = Asset::new("USDT").expect("an asset");
        let fill = FuturesFill {
            trade_id: VenueTradeId::new(trade).expect("a trade id"),
            venue_order_id: VenueOrderId::new("8389765519").expect("an order id"),
            client_id: Some(id.to_string()),
            instrument: self.instrument(),
            origin: OrderOrigin::Client,
            side: Side::Buy,
            price: Price::new(decimal(price)).expect("a price"),
            quantity: Quantity::for_category(self.identity.category(), decimal(quantity))
                .expect("a quantity"),
            fee: Money::new(decimal("0.0512"), usdt),
            realized_pnl: None,
            time_ms: T + 9_000,
            source: RecordSource::Query,
        };
        FillRecord::new(self.scope(), fill, Some(operation), cycle).expect("a fill record")
    }

    /// The cycle `operation`'s entry opened, waiting for its stop.
    pub(crate) fn cycle(&self, operation: OperationId, at_ms: i64) -> CycleRecord {
        let id = PositionCycleId::new(operation.get()).expect("a cycle id");
        CycleRecord::new(PositionCycle::open(
            id,
            self.key(),
            self.identity.id().clone(),
            PositionSide::Long,
            operation,
            at_ms,
        ))
        .expect("a cycle record")
    }

    /// The line's position leased to `holder` for 30 seconds from
    /// `now_ms`, which must be granted.
    pub(crate) fn lease(&self, store: &impl ExecutionStore, holder: &str, now_ms: i64) -> Lease {
        match store.acquire_lease(&self.lease_request(holder, now_ms)) {
            Ok(LeaseAnswer::Granted(lease)) => lease,
            other => panic!("a lease for {holder}: {other:?}"),
        }
    }

    pub(crate) fn lease_request(&self, holder: &str, now_ms: i64) -> LeaseRequest {
        LeaseRequest {
            key: self.key(),
            profile: self.identity.id().clone(),
            holder: LeaseHolder::new(holder).expect("a holder"),
            ttl_ms: 30_000,
            now_ms,
        }
    }
}

/// `intent` with its first order attempt recorded: sent as `id` at
/// `at_ms`, carried out by the operation `id` names.
pub(crate) fn submitting(intent: &IntentRecord, id: &ClientOrderId, at_ms: i64) -> IntentRecord {
    let mut next = intent.intent().clone();
    next.record_attempt(OrderAttempt {
        client_order_id: id.clone(),
        sent_at_ms: at_ms,
    })
    .expect("an attempt");
    intent.with_intent(next).expect("a successor")
}

/// A unit written under `fence` at `at_ms`.
pub(crate) fn fenced(fence: &Fence, at_ms: i64) -> ExecutionUnit {
    ExecutionUnit::at(at_ms).under(fence.clone())
}

impl Line {
    /// What `delivered` asks of the line's profile, as the signal service
    /// resolves it: an entry with the line's snapshot, or an exit.
    pub(crate) fn resolved(
        &self,
        delivered: &Delivered,
    ) -> Result<quantforge::model::futures::ResolvedSignal, quantforge::model::futures::SignalRefusal>
    {
        let admitted = self.admitted(delivered);
        Ok(quantforge::model::futures::ResolvedSignal {
            intent: admitted.intent().kind().clone(),
            policy: admitted.intent().policy().cloned(),
            spelling: quantforge::model::futures::TickerSpelling::TradingView,
        })
    }
}

/// The order `id` was sent as, as the venue reports it: `filled` of the
/// `requested` it was for.
pub(crate) fn reported(
    line: &Line,
    id: &ClientOrderId,
    status: quantforge::model::futures::FuturesOrderStatus,
    requested: &str,
    filled: &str,
) -> quantforge::model::futures::FuturesOrder {
    let quantity = |text: &str| {
        Quantity::for_category(
            line.identity.category(),
            Decimal::from_str_exact(text).expect("a decimal"),
        )
        .expect("a quantity")
    };
    quantforge::model::futures::FuturesOrder {
        instrument: line.instrument(),
        venue_order_id: VenueOrderId::new("8389765519").expect("an order id"),
        client_id: Some(id.to_string()),
        side: Side::Buy,
        reduce_only: false,
        origin: OrderOrigin::Client,
        status,
        requested: quantity(requested),
        filled: quantity(filled),
        average_price: None,
        source: RecordSource::Query,
        updated_at_ms: T + 9_000,
    }
}

impl Line {
    /// What an entry carried out by `operation` reserves: `margin` USDT
    /// from the account's USDT pool, for a risk of 149.9274 USDT.
    pub(crate) fn reservation(
        &self,
        operation: OperationId,
        margin: &str,
        at_ms: i64,
    ) -> quantforge::model::execution::Reservation {
        let usdt = |text: &str| {
            Money::new(
                Decimal::from_str_exact(text).expect("a decimal"),
                Asset::new("USDT").expect("an asset"),
            )
        };
        quantforge::model::execution::Reservation::held(
            self.scope(),
            operation,
            self.instrument(),
            quantforge::model::futures::PoolScope::Asset(Asset::new("USDT").expect("an asset")),
            usdt(margin),
            usdt("149.9274"),
            at_ms,
        )
        .expect("a reservation")
    }
}
