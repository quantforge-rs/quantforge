//! An admitted signal's intent as the ledger keeps it, with the operation
//! that carries it out and the position cycle it belongs to.

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::futures::{
    ExternalIntent, IntentKind, PositionCycleId, PositionKey, ProfileId, SignalId,
};
use crate::model::{ModelError, OperationId};
use serde::{Deserialize, Serialize};

/// The version of [`IntentRecord`]'s stored form.
pub const INTENT_RECORD_FORMAT: u32 = 1;

const RECORD: &str = "intent record";

/// One signal's intent, its progress, and what it is tied to. The
/// operation and the cycle stand beside the intent, never inside its
/// identity: an intent is known by its profile and its signal alone, so a
/// delivery sent again finds it whatever position has opened or closed
/// since. The operation is the one its latest order was sent under: an
/// entry is sent once, under one, and an exit that is sent again stays
/// under its own until that one has no attempt left to number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "IntentRecordParts")]
pub struct IntentRecord {
    format: u32,
    scope: ExecutionScope,
    intent: ExternalIntent,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
}

impl IntentRecord {
    /// The record of `intent`, in no cycle yet. Its scope is its own
    /// profile's on its own position, and its operation the one its
    /// latest attempt names, if it has sent any.
    pub fn new(intent: ExternalIntent) -> Self {
        let (key, profile) = parts_of(intent.kind());
        Self {
            format: INTENT_RECORD_FORMAT,
            scope: ExecutionScope::of_position(key, profile),
            operation: carried_by(&intent),
            intent,
            cycle: None,
        }
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub fn intent(&self) -> &ExternalIntent {
        &self.intent
    }

    pub fn profile(&self) -> &ProfileId {
        parts_of(self.intent.kind()).1
    }

    pub fn signal(&self) -> &SignalId {
        match self.intent.kind() {
            IntentKind::Entry(entry) => &entry.signal,
            IntentKind::Exit(exit) => &exit.signal,
        }
    }

    /// The position the intent acts on.
    pub fn key(&self) -> &PositionKey {
        self.intent.kind().key()
    }

    /// The operation that carries the intent out, once an order was sent
    /// for it: the one its latest attempt's client order ID names.
    pub const fn operation(&self) -> Option<OperationId> {
        self.operation
    }

    /// The cycle the intent belongs to: the one an entry opened, or the
    /// one an exit closes.
    pub const fn cycle(&self) -> Option<PositionCycleId> {
        self.cycle
    }

    /// Whether the intent is an entry whose outcome is open, which blocks
    /// every other entry on its position (ADR 0002 §5).
    pub fn blocks_entries(&self) -> bool {
        matches!(self.intent.kind(), IntentKind::Entry(_)) && self.intent.state().is_unresolved()
    }

    /// The record with its intent moved on, carried out by the operation
    /// its latest attempt names.
    pub fn with_intent(&self, intent: ExternalIntent) -> Result<Self, ModelError> {
        let next = Self {
            operation: carried_by(&intent),
            intent,
            ..self.clone()
        };
        next.follows(self)?;
        Ok(next)
    }

    /// The record tied to `cycle`. A cycle, once named, is never replaced.
    pub fn in_cycle(&self, cycle: PositionCycleId) -> Result<Self, ModelError> {
        if self.cycle.is_some_and(|held| held != cycle) {
            return Err(not_the_same(RECORD, "position cycle"));
        }
        Ok(Self {
            cycle: Some(cycle),
            ..self.clone()
        })
    }

    /// Whether `self` may replace `previous`: the same signal's intent,
    /// moved along its state machine or not at all, under the policy it
    /// was admitted with, its attempts only added to, and the cycle it
    /// names never replaced by another.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        if self.scope != previous.scope || self.intent.kind() != previous.intent.kind() {
            return Err(not_the_same(RECORD, "signal"));
        }
        let (from, to) = (previous.intent.state(), self.intent.state());
        if from != to && !from.can_move_to(to) {
            return Err(ModelError::InvalidTransition {
                machine: "intent",
                from: from.as_str(),
                to: to.as_str(),
            });
        }
        if previous.intent.policy().is_some() && self.intent.policy() != previous.intent.policy() {
            return Err(not_the_same(RECORD, "policy snapshot"));
        }
        let (before, after) = (previous.intent.attempts(), self.intent.attempts());
        if after.len() < before.len() || after[..before.len()] != *before {
            return Err(not_the_same(RECORD, "list of attempts sent"));
        }
        if previous.cycle.is_some() && self.cycle != previous.cycle {
            return Err(not_the_same(RECORD, "position cycle"));
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, INTENT_RECORD_FORMAT, text)
    }
}

/// The operation `intent`'s latest attempt was sent under.
fn carried_by(intent: &ExternalIntent) -> Option<OperationId> {
    intent
        .attempts()
        .last()
        .map(|attempt| attempt.client_order_id.operation())
}

fn parts_of(kind: &IntentKind) -> (&PositionKey, &ProfileId) {
    match kind {
        IntentKind::Entry(entry) => (&entry.key, &entry.profile),
        IntentKind::Exit(exit) => (&exit.key, &exit.profile),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentRecordParts {
    format: u32,
    scope: ExecutionScope,
    intent: ExternalIntent,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
}

impl TryFrom<IntentRecordParts> for IntentRecord {
    type Error = ModelError;

    fn try_from(parts: IntentRecordParts) -> Result<Self, Self::Error> {
        if parts.format != INTENT_RECORD_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: INTENT_RECORD_FORMAT,
            });
        }
        let (key, profile) = parts_of(parts.intent.kind());
        if parts.scope != ExecutionScope::of_position(key, profile) {
            return Err(disagrees(
                RECORD,
                "its scope is not its own profile's on its own position",
            ));
        }
        if parts.operation != carried_by(&parts.intent) {
            return Err(disagrees(
                RECORD,
                "its operation is not the one its latest attempt was sent under",
            ));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            intent: parts.intent,
            operation: parts.operation,
            cycle: parts.cycle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry of profile `main`, sent once as an order of operation 1.
    const SENT: &str = r#"{"format":1,"scope":{"venue":"Bybit","category":"Linear","environment":"Demo","account":{"Venue":"71002233"},"profile":"main"},"intent":{"format":1,"kind":{"Entry":{"profile":"main","signal":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","signal_time_ms":1792454405000,"key":{"account":{"venue":"Bybit","environment":"Demo","venue_account_id":{"Venue":"71002233"}},"instrument":{"exchange":"BybitLinear","symbol":"SOLUSDT","settle":"USDT"}},"side":"Long","expected_price":"234.23","stop":"200.00"}},"state":"Submitting","policy":{"format":1,"profile":"main","account":{"venue":"Bybit","environment":"Demo","venue_account_id":{"Venue":"71002233"}},"revision":4,"instrument":{"exchange":"BybitLinear","symbol":"SOLUSDT","settle":"USDT"},"risk_percent":{"value":"1","source":"Defaults"},"stop_percent":{"value":"3","source":"Defaults"},"leverage":{"value":"5","source":"Defaults"},"capital":{"amount":"15000","asset":"USDT"},"capital_revision":4},"attempts":[{"client_order_id":"qf1-k3x9p2-1-1-e1","sent_at_ms":1792454408000}]},"operation":1,"cycle":null}"#;

    #[test]
    fn an_intents_operation_is_the_one_its_latest_attempt_was_sent_under() {
        let sent = IntentRecord::from_stored(SENT).expect("a record");
        assert_eq!(sent.operation().map(OperationId::get), Some(1));
        assert_eq!(sent.to_stored().expect("stored"), SENT);
        assert!(sent.blocks_entries(), "sent, and not known to have ended");

        // A record that names another operation than its attempt's, or
        // none, is not one any build wrote.
        for other in [r#""operation":2,"#, r#""operation":null,"#] {
            let changed = SENT.replace(r#""operation":1,"#, other);
            let refused = IntentRecord::from_stored(&changed).expect_err(other);
            assert!(
                refused
                    .to_string()
                    .contains("its operation is not the one its latest attempt was sent under"),
                "{refused}"
            );
        }
        // Before anything is sent for it, an intent names no operation.
        let admitted = SENT
            .replace(r#""state":"Submitting""#, r#""state":"Admitted""#)
            .replace(
                r#""attempts":[{"client_order_id":"qf1-k3x9p2-1-1-e1","sent_at_ms":1792454408000}]"#,
                r#""attempts":[]"#,
            );
        assert!(
            IntentRecord::from_stored(&admitted).is_err(),
            "it names one"
        );
        let admitted = admitted.replace(r#""operation":1,"#, r#""operation":null,"#);
        let record = IntentRecord::from_stored(&admitted).expect("a record");
        assert_eq!(record.operation(), None);
        assert!(!record.blocks_entries(), "admitted, and not sent");
    }

    #[test]
    fn a_successor_keeps_its_policy_its_attempts_and_its_cycle() {
        let sent = IntentRecord::from_stored(SENT).expect("a record");
        let cycle = PositionCycleId::new(1).expect("a cycle");
        let other = PositionCycleId::new(2).expect("a cycle");
        let bound = sent.in_cycle(cycle).expect("bound");
        assert!(bound.follows(&sent).is_ok());
        assert!(bound.in_cycle(cycle).is_ok(), "the same cycle again");
        assert!(bound.in_cycle(other).is_err(), "a cycle is named once");

        // The same intent with nothing sent for it is no successor: an
        // attempt, once recorded, stays.
        let unsent = SENT
            .replace(r#""state":"Submitting""#, r#""state":"Admitted""#)
            .replace(
                r#""attempts":[{"client_order_id":"qf1-k3x9p2-1-1-e1","sent_at_ms":1792454408000}]"#,
                r#""attempts":[]"#,
            )
            .replace(r#""operation":1,"#, r#""operation":null,"#);
        let unsent = IntentRecord::from_stored(&unsent).expect("a record");
        assert!(sent.follows(&unsent).is_ok(), "admitted, then sent");
        let back = unsent.follows(&sent).expect_err("sent, then admitted");
        assert!(
            back.to_string()
                .contains("cannot move from submitting to admitted")
        );
        // Another policy than the one it was admitted under.
        let repriced = SENT.replace(
            r#""revision":4,"instrument""#,
            r#""revision":5,"instrument""#,
        );
        let repriced = IntentRecord::from_stored(&repriced).expect("a record");
        let refused = repriced.follows(&sent).expect_err("another policy");
        assert!(refused.to_string().contains("policy snapshot"), "{refused}");
    }
}
