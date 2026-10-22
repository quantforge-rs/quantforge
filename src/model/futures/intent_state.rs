//! An external signal's intent state machine (ADR 0002 §1, §9), as the
//! parity matrix freezes it as `signal_intent`. A move that needs the
//! venue's evidence has its own method taking that evidence, and
//! [`IntentState::move_to`] refuses it: an acknowledgement is never
//! evidence of a fill.

use super::{FuturesFill, FuturesOrder, FuturesOrderStatus, Rejection};
use crate::model::ModelError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Where an external signal's intent stands. `filled` means its order is
/// final with at least one fill; the filled quantity is recorded beside it
/// and may be less than requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IntentState {
    Received,
    Admitted,
    Rejected,
    Duplicate,
    Noop,
    Expired,
    Submitting,
    OutcomeUnknown,
    NotPlaced,
    PartiallyFilled,
    Filled,
    Unfilled,
}

impl IntentState {
    pub const INITIAL: Self = Self::Received;

    pub const ALL: [Self; 12] = [
        Self::Received,
        Self::Admitted,
        Self::Rejected,
        Self::Duplicate,
        Self::Noop,
        Self::Expired,
        Self::Submitting,
        Self::OutcomeUnknown,
        Self::NotPlaced,
        Self::PartiallyFilled,
        Self::Filled,
        Self::Unfilled,
    ];

    /// Every allowed move, as the parity matrix lists them.
    pub const TRANSITIONS: [(Self, Self); 19] = [
        (Self::Received, Self::Admitted),
        (Self::Received, Self::Rejected),
        (Self::Received, Self::Duplicate),
        (Self::Received, Self::Noop),
        (Self::Admitted, Self::Submitting),
        (Self::Admitted, Self::Expired),
        (Self::Admitted, Self::Rejected),
        (Self::Admitted, Self::Noop),
        (Self::Submitting, Self::PartiallyFilled),
        (Self::Submitting, Self::Filled),
        (Self::Submitting, Self::Unfilled),
        (Self::Submitting, Self::Rejected),
        (Self::Submitting, Self::OutcomeUnknown),
        (Self::OutcomeUnknown, Self::PartiallyFilled),
        (Self::OutcomeUnknown, Self::Filled),
        (Self::OutcomeUnknown, Self::Unfilled),
        (Self::OutcomeUnknown, Self::Rejected),
        (Self::OutcomeUnknown, Self::NotPlaced),
        (Self::PartiallyFilled, Self::Filled),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Admitted => "admitted",
            Self::Rejected => "rejected",
            Self::Duplicate => "duplicate",
            Self::Noop => "noop",
            Self::Expired => "expired",
            Self::Submitting => "submitting",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::NotPlaced => "not_placed",
            Self::PartiallyFilled => "partially_filled",
            Self::Filled => "filled",
            Self::Unfilled => "unfilled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Rejected
                | Self::Duplicate
                | Self::Noop
                | Self::Expired
                | Self::NotPlaced
                | Self::Filled
                | Self::Unfilled
        )
    }

    /// Whether an entry in this state blocks every other entry on its
    /// position (ADR 0002 §5).
    pub const fn is_unresolved(self) -> bool {
        matches!(self, Self::Submitting | Self::OutcomeUnknown)
    }

    pub fn can_move_to(self, to: Self) -> bool {
        Self::TRANSITIONS.contains(&(self, to))
    }

    /// Moves to `to` where no venue evidence is needed: admission's
    /// outcomes, submitting, expiry, and an unknown outcome.
    pub fn move_to(self, to: Self) -> Result<Self, ModelError> {
        let gated = matches!(
            to,
            Self::PartiallyFilled | Self::Filled | Self::Unfilled | Self::NotPlaced
        ) || (to == Self::Rejected && self.is_unresolved());
        if gated {
            return Err(ModelError::MissingEvidence {
                machine: "intent",
                to: to.as_str(),
                evidence: "an order, a fill or a refusal read back from the venue".to_string(),
            });
        }
        self.checked(to)
    }

    /// What an order read back from a query or the stream proves.
    pub fn on_order(self, order: &FuturesOrder) -> Result<Self, ModelError> {
        let filled = !order.filled.is_zero();
        let to = match (order.status, order.status.is_final(), filled) {
            (FuturesOrderStatus::Rejected, _, false) => Self::Rejected,
            (_, true, true) => Self::Filled,
            (_, true, false) => Self::Unfilled,
            (_, false, true) => Self::PartiallyFilled,
            // Still working, with nothing filled: no outcome yet.
            (_, false, false) => return Ok(self),
        };
        if to == self {
            return Ok(self);
        }
        self.checked(to)
    }

    /// A fill for the intent's order: exposure exists from that moment.
    pub fn on_fill(self, _fill: &FuturesFill) -> Result<Self, ModelError> {
        match self {
            Self::PartiallyFilled | Self::Filled => Ok(self),
            _ => self.checked(Self::PartiallyFilled),
        }
    }

    /// The venue's definitive refusal of the order just sent. Once an
    /// outcome is unknown, only an order, a fill or ADR 0002 §9's "not
    /// found" settles it.
    pub fn on_rejection(self, _rejection: &Rejection) -> Result<Self, ModelError> {
        if self != Self::Submitting {
            return Err(self.invalid(Self::Rejected));
        }
        Ok(Self::Rejected)
    }

    /// `not_placed`, once the intent record has checked §9's evidence.
    pub(super) fn settle_not_placed(self) -> Result<Self, ModelError> {
        self.checked(Self::NotPlaced)
    }

    fn checked(self, to: Self) -> Result<Self, ModelError> {
        if !self.can_move_to(to) {
            return Err(self.invalid(to));
        }
        Ok(to)
    }

    fn invalid(self, to: Self) -> ModelError {
        ModelError::InvalidTransition {
            machine: "intent",
            from: self.as_str(),
            to: to.as_str(),
        }
    }
}

impl fmt::Display for IntentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_gated_states_are_never_reached_by_a_plain_move() {
        for to in [
            IntentState::PartiallyFilled,
            IntentState::Filled,
            IntentState::Unfilled,
            IntentState::NotPlaced,
        ] {
            for from in IntentState::ALL {
                assert!(from.move_to(to).is_err(), "{from} -> {to}");
            }
        }
        for unresolved in [IntentState::Submitting, IntentState::OutcomeUnknown] {
            assert!(
                unresolved.move_to(IntentState::Rejected).is_err(),
                "{unresolved}"
            );
        }
        assert_eq!(
            IntentState::Received.move_to(IntentState::Rejected).ok(),
            Some(IntentState::Rejected),
            "admission refuses without venue evidence"
        );
    }

    #[test]
    fn terminal_states_have_no_way_out_and_every_other_state_has_one() {
        for state in IntentState::ALL {
            let leaves = IntentState::TRANSITIONS
                .iter()
                .any(|(from, _)| *from == state);
            assert_eq!(leaves, !state.is_terminal(), "{state}");
        }
    }

    #[test]
    fn plain_moves_follow_the_transition_table() {
        let admitted = IntentState::INITIAL
            .move_to(IntentState::Admitted)
            .expect("admitted");
        let submitting = admitted
            .move_to(IntentState::Submitting)
            .expect("submitting");
        assert_eq!(
            submitting.move_to(IntentState::OutcomeUnknown).ok(),
            Some(IntentState::OutcomeUnknown)
        );
        assert!(
            submitting.move_to(IntentState::Expired).is_err(),
            "only before submitting"
        );
    }
}
