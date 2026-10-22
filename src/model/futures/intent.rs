//! An external signal's intent as the store records it (ADR 0002 §1, §5,
//! §9): what the signal asked for, where it stands, and every order
//! attempt sent for it. Evidence counts only for the intent's own attempts,
//! and §9's bounds decide when "not found" means never placed.

use super::{
    ClientOrderId, FuturesFill, FuturesOrder, IntentState, OrderAttempt, OrderLeg,
    OrderLookupGuarantee, PositionKey, PositionSnapshot, ProfileId, Rejection, SignalId,
};
use crate::model::{InstrumentId, ModelError, PositionSide, Price, TimestampMs};
use serde::{Deserialize, Serialize};

/// The version of [`ExternalIntent`]'s stored form. A record of another
/// version is refused, never guessed at.
pub const INTENT_FORMAT: u32 = 1;

/// How long after sending an order a query that does not find it proves
/// nothing (ADR 0002 §9). The same on every venue, family and environment.
pub const PROPAGATION_DELAY_MS: i64 = 30_000;

/// How long after sending an entry it may stay unsettled before its key
/// raises an incident and halts (ADR 0002 §5, §9).
pub const SETTLE_WINDOW_MS: i64 = 60_000;

/// An entry signal, resolved against its profile: open `side` on `key`'s
/// instrument, sized at the alert's expected price.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryIntent {
    pub profile: ProfileId,
    pub signal: SignalId,
    /// The alert's `{{timenow}}`.
    pub signal_time_ms: TimestampMs,
    pub key: PositionKey,
    pub side: PositionSide,
    pub expected_price: Price,
    /// The alert's absolute stop, if it gave one.
    pub stop: Option<Price>,
}

/// An exit signal: close the owned `side` cycle on `key`'s instrument whose
/// entry came strictly earlier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitIntent {
    pub profile: ProfileId,
    pub signal: SignalId,
    pub signal_time_ms: TimestampMs,
    pub key: PositionKey,
    pub side: PositionSide,
}

/// What a signal asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentKind {
    Entry(EntryIntent),
    Exit(ExitIntent),
}

impl IntentKind {
    pub fn key(&self) -> &PositionKey {
        match self {
            Self::Entry(entry) => &entry.key,
            Self::Exit(exit) => &exit.key,
        }
    }

    /// The leg its orders are sent as.
    pub const fn leg(&self) -> OrderLeg {
        match self {
            Self::Entry(_) => OrderLeg::Entry,
            Self::Exit(_) => OrderLeg::Close,
        }
    }
}

/// An external intent and its progress: the record a store keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ExternalIntentParts")]
pub struct ExternalIntent {
    format: u32,
    kind: IntentKind,
    state: IntentState,
    attempts: Vec<OrderAttempt>,
}

impl ExternalIntent {
    /// A signal just received.
    pub fn new(kind: IntentKind) -> Self {
        Self {
            format: INTENT_FORMAT,
            kind,
            state: IntentState::INITIAL,
            attempts: Vec::new(),
        }
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn kind(&self) -> &IntentKind {
        &self.kind
    }

    pub const fn state(&self) -> IntentState {
        self.state
    }

    pub fn attempts(&self) -> &[OrderAttempt] {
        &self.attempts
    }

    /// A move that needs no venue evidence. An exit never expires.
    pub fn move_to(&mut self, to: IntentState) -> Result<(), ModelError> {
        if to == IntentState::Expired && matches!(self.kind, IntentKind::Exit(_)) {
            return Err(ModelError::InvalidTransition {
                machine: "exit intent",
                from: self.state.as_str(),
                to: to.as_str(),
            });
        }
        self.state = self.state.move_to(to)?;
        Ok(())
    }

    /// Records an order sent for the intent. The first moves an admitted
    /// intent to submitting. An entry is sent once and never again; an exit
    /// may retry with a new attempt while its outcome is open.
    pub fn record_attempt(&mut self, attempt: OrderAttempt) -> Result<(), ModelError> {
        super::order::expect_leg(&attempt.client_order_id, &[self.kind.leg()])?;
        if self
            .attempts
            .iter()
            .any(|sent| sent.client_order_id == attempt.client_order_id)
        {
            return Err(ModelError::InvalidIdentifier {
                what: "order attempt",
                value: attempt.client_order_id.to_string(),
                rule: "each attempt has its own client order id",
            });
        }
        let retry = matches!(self.kind, IntentKind::Exit(_)) && self.state.is_unresolved();
        if self.state == IntentState::Admitted && self.attempts.is_empty() {
            self.state = self.state.move_to(IntentState::Submitting)?;
        } else if !retry {
            return Err(ModelError::InvalidTransition {
                machine: "intent",
                from: self.state.as_str(),
                to: "another attempt",
            });
        }
        self.attempts.push(attempt);
        Ok(())
    }

    /// An order read back from the venue for one of the intent's attempts.
    pub fn on_order(&mut self, order: &FuturesOrder) -> Result<(), ModelError> {
        self.own(order.client_order_id().as_ref(), &order.instrument)?;
        self.state = self.state.on_order(order)?;
        Ok(())
    }

    /// A fill for one of the intent's attempts.
    pub fn on_fill(&mut self, fill: &FuturesFill) -> Result<(), ModelError> {
        self.own(fill.client_order_id().as_ref(), &fill.instrument)?;
        self.state = self.state.on_fill(fill)?;
        Ok(())
    }

    /// The venue's definitive refusal of the attempt just sent.
    pub fn on_rejection(&mut self, rejection: &Rejection) -> Result<(), ModelError> {
        if self.attempts.is_empty() {
            return Err(self.missing("an attempt the venue refused"));
        }
        self.state = self.state.on_rejection(rejection)?;
        Ok(())
    }

    /// Settles an entry whose outcome was unknown as never placed, on ADR
    /// 0002 §9's evidence: a lookup at least the propagation delay after the
    /// attempt was sent, inside the venue's retention, on a venue whose
    /// lookups are verified, and a snapshot from after the delay that shows
    /// the position still flat. Never for an exit, and never a resubmission.
    pub fn entry_not_placed(&mut self, evidence: &NotPlacedEvidence) -> Result<(), ModelError> {
        if !matches!(self.kind, IntentKind::Entry(_)) {
            return Err(self.missing("an entry; an exit retries instead"));
        }
        let sent_at = self
            .attempts
            .iter()
            .find(|attempt| attempt.client_order_id == evidence.client_order_id)
            .map(|attempt| attempt.sent_at_ms)
            .ok_or_else(|| self.missing("a lookup of the intent's own attempt"))?;
        let waited = evidence.queried_at_ms.saturating_sub(sent_at);
        let guarantee = evidence.guarantee;
        let position = &evidence.position;
        let proven = guarantee.visibility_verified
            && waited >= PROPAGATION_DELAY_MS
            && waited <= guarantee.unfilled_retention_ms
            && position.instrument == *self.kind.key().instrument()
            && position.observed_at_ms.saturating_sub(sent_at) >= PROPAGATION_DELAY_MS
            && position.is_flat();
        if !proven {
            return Err(self.missing(
                "a verified lookup past the propagation delay, inside retention, \
                 and a flat snapshot from after it",
            ));
        }
        self.state = self.state.settle_not_placed()?;
        Ok(())
    }

    /// Whether the intent is still unresolved once ADR 0002 §9's settle
    /// window has passed since its first attempt: its key then raises an
    /// incident.
    pub fn unsettled_past_window(&self, now_ms: TimestampMs) -> bool {
        self.state.is_unresolved()
            && self
                .attempts
                .first()
                .is_some_and(|first| now_ms.saturating_sub(first.sent_at_ms) >= SETTLE_WINDOW_MS)
    }

    fn own(&self, id: Option<&ClientOrderId>, instrument: &InstrumentId) -> Result<(), ModelError> {
        let ours = id.is_some_and(|id| {
            self.attempts
                .iter()
                .any(|attempt| &attempt.client_order_id == id)
        });
        if !ours || instrument != self.kind.key().instrument() {
            return Err(self.missing("evidence for one of the intent's own attempts"));
        }
        Ok(())
    }

    fn missing(&self, evidence: &str) -> ModelError {
        ModelError::MissingEvidence {
            machine: "intent",
            to: "its next state",
            evidence: evidence.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct ExternalIntentParts {
    format: u32,
    kind: IntentKind,
    state: IntentState,
    attempts: Vec<OrderAttempt>,
}

impl TryFrom<ExternalIntentParts> for ExternalIntent {
    type Error = ModelError;

    fn try_from(parts: ExternalIntentParts) -> Result<Self, Self::Error> {
        if parts.format != INTENT_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: "intent",
                found: parts.format,
                expected: INTENT_FORMAT,
            });
        }
        for attempt in &parts.attempts {
            super::order::expect_leg(&attempt.client_order_id, &[parts.kind.leg()])?;
        }
        if matches!(parts.kind, IntentKind::Entry(_)) && parts.attempts.len() > 1 {
            return Err(ModelError::InvalidIdentifier {
                what: "entry intent",
                value: format!("{} attempts", parts.attempts.len()),
                rule: "an entry is sent once",
            });
        }
        Ok(Self {
            format: parts.format,
            kind: parts.kind,
            state: parts.state,
            attempts: parts.attempts,
        })
    }
}

/// What it takes to call an entry never placed (ADR 0002 §9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotPlacedEvidence {
    /// The attempt the lookup was for.
    pub client_order_id: ClientOrderId,
    /// When the lookup by client ID answered "not found".
    pub queried_at_ms: TimestampMs,
    /// What the venue guarantees about such lookups.
    pub guarantee: OrderLookupGuarantee,
    /// A REST snapshot of the position, taken after the lookup.
    pub position: PositionSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::futures::{ActivationEpoch, OwnerTag};
    use crate::model::{
        AccountRef, Asset, Environment, ExchangeId, OperationId, Symbol, Venue, VenueAccountId,
    };

    fn exit() -> ExternalIntent {
        let id = VenueAccountId::issued("1").expect("account id");
        let account = AccountRef::new(Venue::Bybit, Environment::Demo, id).expect("account");
        let symbol = Symbol::new("BTCUSDT").expect("symbol");
        let usdt = Asset::new("USDT").expect("asset");
        let instrument = InstrumentId::new(ExchangeId::BybitLinear, symbol, usdt);
        ExternalIntent::new(IntentKind::Exit(ExitIntent {
            profile: ProfileId::new("main").expect("profile"),
            signal: SignalId::new("exit").expect("signal"),
            signal_time_ms: 0,
            key: PositionKey::new(account, instrument).expect("key"),
            side: PositionSide::Short,
        }))
    }

    #[test]
    fn a_stored_intent_of_another_format_is_refused_never_guessed_at() {
        let mut json = serde_json::to_value(exit()).expect("json");
        assert_eq!(json["format"], INTENT_FORMAT);
        let back: ExternalIntent = serde_json::from_value(json.clone()).expect("round trip");
        assert_eq!(back, exit());
        json["format"] = serde_json::json!(INTENT_FORMAT + 1);
        let error = serde_json::from_value::<ExternalIntent>(json).expect_err("format");
        assert!(error.to_string().contains("intent format 2"), "{error}");
    }

    #[test]
    fn a_stored_attempt_on_the_wrong_leg_is_refused() {
        let mut json = serde_json::to_value(exit()).expect("json");
        let entry_leg = ClientOrderId::new(
            OwnerTag::new("k3x9p2").expect("tag"),
            ActivationEpoch(1),
            OperationId::new(1).expect("operation"),
            OrderLeg::Entry,
            1,
        )
        .expect("client order id");
        json["attempts"] = serde_json::json!([{"client_order_id": entry_leg, "sent_at_ms": 0}]);
        assert!(serde_json::from_value::<ExternalIntent>(json).is_err());
    }
}
