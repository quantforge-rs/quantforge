//! An external signal's intent as the store records it (ADR 0002 §1, §5,
//! §9): what the signal asked for, where it stands, the policy an entry
//! was admitted under, and every order attempt sent for it. Evidence counts
//! only for the intent's own attempts, and §9's bounds decide when "not
//! found" means never placed.

use super::{
    ClientOrderId, FuturesFill, FuturesOrder, IntentState, OrderAttempt, OrderLeg,
    OrderLookupGuarantee, PolicySnapshot, PositionKey, PositionSnapshot, ProfileId, Rejection,
    SignalId,
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

    /// Whether it is admitted under `policy`: an entry under a snapshot of
    /// its own profile's policy for its own account and instrument, an exit
    /// under none.
    fn takes(&self, policy: Option<&PolicySnapshot>) -> bool {
        match (self, policy) {
            (Self::Entry(entry), Some(policy)) => {
                policy.profile() == &entry.profile
                    && policy.account() == entry.key.account()
                    && policy.instrument() == entry.key.instrument()
            }
            (Self::Exit(_), None) => true,
            _ => false,
        }
    }

    /// Whether `policy` leaves it a stop to be sized against: an entry's
    /// own, or the policy's fallback. Only an entry needs one.
    fn has_stop(&self, policy: Option<&PolicySnapshot>) -> bool {
        match (self, policy) {
            (Self::Entry(entry), Some(policy)) => policy.stop_for(entry.stop).is_ok(),
            _ => true,
        }
    }
}

/// An external intent and its progress: the record a store keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "stored::ExternalIntentParts")]
pub struct ExternalIntent {
    format: u32,
    kind: IntentKind,
    state: IntentState,
    /// The policy an entry was admitted under: taken once, at admission,
    /// and never changed. An exit has none.
    policy: Option<PolicySnapshot>,
    attempts: Vec<OrderAttempt>,
}

impl ExternalIntent {
    /// A signal just received.
    pub fn new(kind: IntentKind) -> Self {
        Self {
            format: INTENT_FORMAT,
            kind,
            state: IntentState::INITIAL,
            policy: None,
            attempts: Vec::new(),
        }
    }

    /// The policy the entry was admitted under: the settings it is sized
    /// from and their revisions, as they stood then. Later edits of the
    /// profile's policy never reach it.
    pub fn policy(&self) -> Option<&PolicySnapshot> {
        self.policy.as_ref()
    }

    /// Admits a received signal. An entry is admitted under a snapshot of
    /// its own profile's policy for its own account and instrument, which
    /// it keeps for good; an exit takes none, since no policy can hold back
    /// a close.
    ///
    /// An entry with no stop of its own, under a snapshot that gives no
    /// fallback, is never admitted: [`PolicySnapshot::stop_for`] refuses it
    /// as `no_stop`, which is what its caller records it with.
    pub fn admit(&mut self, policy: Option<PolicySnapshot>) -> Result<(), ModelError> {
        if !self.kind.takes(policy.as_ref()) {
            return Err(Self::unadmitted(
                "a snapshot of its own profile's policy for its account and instrument, for an \
                 entry, and none for an exit",
            ));
        }
        if !self.kind.has_stop(policy.as_ref()) {
            return Err(Self::unadmitted(
                "a stop, the alert's own or its policy's fallback; an entry with neither is \
                 refused as no_stop",
            ));
        }
        self.state = self.state.move_to(IntentState::Admitted)?;
        self.policy = policy;
        Ok(())
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

    /// A move that needs no venue evidence. An exit never expires, and
    /// nothing is admitted this way: [`ExternalIntent::admit`] does that.
    pub fn move_to(&mut self, to: IntentState) -> Result<(), ModelError> {
        if to == IntentState::Admitted {
            return Err(Self::unadmitted(
                "its admission, with an entry's policy snapshot",
            ));
        }
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
    /// Once an exit has retried, its latest attempt decides: an earlier
    /// attempt's order that ended without a fill says nothing and leaves
    /// the state alone. Late evidence that an earlier attempt filled after
    /// all contradicts the record, and is reported as an invalid
    /// transition rather than swallowed.
    pub fn on_order(&mut self, order: &FuturesOrder) -> Result<(), ModelError> {
        let id = order.client_order_id();
        self.own(id.as_ref(), &order.instrument)?;
        let nothing_executed = order.status.is_final() && order.filled.is_zero();
        if nothing_executed && id.as_ref().is_some_and(|id| self.superseded(id)) {
            return Ok(());
        }
        self.state = self.state.on_order(order)?;
        Ok(())
    }

    /// A fill for one of the intent's attempts.
    pub fn on_fill(&mut self, fill: &FuturesFill) -> Result<(), ModelError> {
        self.own(fill.client_order_id().as_ref(), &fill.instrument)?;
        self.state = self.state.on_fill(fill)?;
        Ok(())
    }

    /// The venue's definitive refusal of one of the intent's own attempts.
    /// A refused retry settles an exit as rejected; a refusal of an earlier,
    /// superseded attempt says nothing, like its order.
    pub fn on_rejection(&mut self, rejection: &Rejection) -> Result<(), ModelError> {
        let id = rejection.client_order_id();
        if !self.attempts.iter().any(|sent| &sent.client_order_id == id) {
            return Err(self.missing("a refusal of one of the intent's own attempts"));
        }
        if self.superseded(id) {
            return Ok(());
        }
        self.state = if self.attempts.len() > 1 {
            self.state.retry_rejected()?
        } else {
            self.state.on_rejection(rejection)?
        };
        Ok(())
    }

    /// Settles an entry whose outcome was unknown as never placed, on ADR
    /// 0002 §9's evidence: a lookup at least the propagation delay after the
    /// attempt was sent, inside the venue's retention, on a venue whose
    /// lookups are verified, and a snapshot from after that lookup that
    /// shows the position still flat. Never for an exit, and never a
    /// resubmission.
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
            && position.observed_at_ms >= evidence.queried_at_ms
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

    /// Whether `id` is an exit attempt that a later attempt superseded.
    fn superseded(&self, id: &ClientOrderId) -> bool {
        matches!(self.kind, IntentKind::Exit(_))
            && self
                .attempts
                .last()
                .is_some_and(|latest| &latest.client_order_id != id)
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

    /// Why an intent is not admitted.
    fn unadmitted(evidence: &str) -> ModelError {
        ModelError::MissingEvidence {
            machine: "intent",
            to: IntentState::Admitted.as_str(),
            evidence: evidence.to_string(),
        }
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
    /// A REST snapshot of the position, observed no earlier than the lookup.
    pub position: PositionSnapshot,
}

mod stored;

#[cfg(test)]
mod tests;
