//! An intent read back from a store: every check a stored record passes
//! before it is an [`ExternalIntent`] again. A record of another format, an
//! attempt on the wrong leg, a state its attempts cannot explain, or an
//! entry without the policy it was admitted under is refused, never
//! repaired.

use super::{ExternalIntent, INTENT_FORMAT, IntentKind};
use crate::model::ModelError;
use crate::model::futures::{IntentState, OrderAttempt, PolicySnapshot};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ExternalIntentParts {
    format: u32,
    kind: IntentKind,
    state: IntentState,
    policy: Option<PolicySnapshot>,
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
            super::super::order::expect_leg(&attempt.client_order_id, &[parts.kind.leg()])?;
        }
        if matches!(
            parts.state,
            IntentState::Submitting
                | IntentState::OutcomeUnknown
                | IntentState::PartiallyFilled
                | IntentState::Filled
                | IntentState::Unfilled
                | IntentState::NotPlaced
        ) && parts.attempts.is_empty()
        {
            return Err(ModelError::MissingEvidence {
                machine: "intent",
                to: parts.state.as_str(),
                evidence: "an owned order attempt".to_string(),
            });
        }
        if matches!(parts.kind, IntentKind::Entry(_)) && parts.attempts.len() > 1 {
            return Err(ModelError::InvalidIdentifier {
                what: "entry intent",
                value: format!("{} attempts", parts.attempts.len()),
                rule: "an entry is sent once",
            });
        }
        // An entry past its admission keeps the policy it was admitted
        // under; one refused or dropped may have been either side of it.
        let expected = match (&parts.kind, parts.state) {
            (IntentKind::Exit(_), _)
            | (IntentKind::Entry(_), IntentState::Received | IntentState::Duplicate) => Some(false),
            (IntentKind::Entry(_), IntentState::Rejected | IntentState::Noop) => None,
            (IntentKind::Entry(_), _) => Some(true),
        };
        let policy = parts.policy.as_ref();
        // Whatever policy it holds is one it could have been admitted under.
        let own = policy.is_none() || (parts.kind.takes(policy) && parts.kind.has_stop(policy));
        if expected.is_some_and(|expected| expected != policy.is_some()) || !own {
            return Err(ModelError::MissingEvidence {
                machine: "intent",
                to: parts.state.as_str(),
                evidence: "the policy snapshot an entry was admitted under, of its own profile, \
                           account and instrument, with a stop of the entry's own or the \
                           snapshot's fallback, and none before admission or for an exit"
                    .to_string(),
            });
        }
        Ok(Self {
            format: parts.format,
            kind: parts.kind,
            state: parts.state,
            policy: parts.policy,
            attempts: parts.attempts,
        })
    }
}
