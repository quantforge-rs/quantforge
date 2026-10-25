//! A position's protection state machine (ADR 0002 §1, §7, §8, §10), as
//! the parity matrix freezes it as `futures_position`. Nothing is called
//! protected without a read-back that shows the stop, and nothing is
//! called closed without a snapshot that shows the position gone; those
//! moves take their evidence, and [`ProtectionState::move_to`] refuses
//! them.

use super::{CoveredStop, OperatorRemoval, PositionSnapshot, StopOwner};
use crate::model::{ModelError, PositionSide};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Where a position's protection stands. Nothing is called protected
/// without a read-back that shows the stop, and nothing is called closed
/// without a snapshot that shows the position gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProtectionState {
    ProtectionPending,
    Protected,
    ProtectedByOperator,
    ProtectionUnknown,
    Unprotected,
    UnprotectedByOperator,
    Closing,
    Closed,
}

impl ProtectionState {
    pub const INITIAL: Self = Self::ProtectionPending;

    pub const ALL: [Self; 8] = [
        Self::ProtectionPending,
        Self::Protected,
        Self::ProtectedByOperator,
        Self::ProtectionUnknown,
        Self::Unprotected,
        Self::UnprotectedByOperator,
        Self::Closing,
        Self::Closed,
    ];

    /// Every allowed move, as the parity matrix lists them.
    pub const TRANSITIONS: [(Self, Self); 36] = [
        (Self::ProtectionPending, Self::Protected),
        (Self::ProtectionPending, Self::ProtectedByOperator),
        (Self::ProtectionPending, Self::ProtectionUnknown),
        (Self::ProtectionPending, Self::Unprotected),
        (Self::ProtectionPending, Self::Closing),
        (Self::ProtectionPending, Self::Closed),
        (Self::Protected, Self::ProtectionPending),
        (Self::Protected, Self::ProtectedByOperator),
        (Self::Protected, Self::ProtectionUnknown),
        (Self::Protected, Self::Unprotected),
        (Self::Protected, Self::UnprotectedByOperator),
        (Self::Protected, Self::Closing),
        (Self::Protected, Self::Closed),
        (Self::ProtectedByOperator, Self::ProtectionPending),
        (Self::ProtectedByOperator, Self::ProtectionUnknown),
        (Self::ProtectedByOperator, Self::UnprotectedByOperator),
        (Self::ProtectedByOperator, Self::Closing),
        (Self::ProtectedByOperator, Self::Closed),
        (Self::ProtectionUnknown, Self::ProtectionPending),
        (Self::ProtectionUnknown, Self::Protected),
        (Self::ProtectionUnknown, Self::ProtectedByOperator),
        (Self::ProtectionUnknown, Self::Unprotected),
        (Self::ProtectionUnknown, Self::UnprotectedByOperator),
        (Self::ProtectionUnknown, Self::Closing),
        (Self::ProtectionUnknown, Self::Closed),
        (Self::Unprotected, Self::ProtectionPending),
        (Self::Unprotected, Self::Protected),
        (Self::Unprotected, Self::Closing),
        (Self::Unprotected, Self::Closed),
        (Self::UnprotectedByOperator, Self::ProtectedByOperator),
        (Self::UnprotectedByOperator, Self::Closing),
        (Self::UnprotectedByOperator, Self::Closed),
        (Self::Closing, Self::Protected),
        (Self::Closing, Self::ProtectionUnknown),
        (Self::Closing, Self::Unprotected),
        (Self::Closing, Self::Closed),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProtectionPending => "protection_pending",
            Self::Protected => "protected",
            Self::ProtectedByOperator => "protected_by_operator",
            Self::ProtectionUnknown => "protection_unknown",
            Self::Unprotected => "unprotected",
            Self::UnprotectedByOperator => "unprotected_by_operator",
            Self::Closing => "closing",
            Self::Closed => "closed",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed)
    }

    pub fn can_move_to(self, to: Self) -> bool {
        Self::TRANSITIONS.contains(&(self, to))
    }

    /// Moves to `to` where no evidence is needed: a pending check, an
    /// unknown read-back, a failed protection, or a close in progress.
    pub fn move_to(self, to: Self) -> Result<Self, ModelError> {
        let evidence = match to {
            Self::Protected | Self::ProtectedByOperator => "a read-back showing a covering stop",
            Self::UnprotectedByOperator => "the operator removing the stop",
            Self::Closed => "a snapshot showing the position gone",
            _ => return self.checked(to),
        };
        Err(ModelError::MissingEvidence {
            machine: "position protection",
            to: to.as_str(),
            evidence: evidence.to_string(),
        })
    }

    /// A read-back showed a stop covering the whole position.
    pub fn verified(self, covered: &CoveredStop) -> Result<Self, ModelError> {
        let to = match covered.owner() {
            StopOwner::QuantForge => Self::Protected,
            StopOwner::Operator => Self::ProtectedByOperator,
        };
        if to == self {
            return Ok(self);
        }
        self.checked(to)
    }

    /// The operator removed the stop on purpose: it is not restored.
    pub fn removed_by_operator(self, _removal: &OperatorRemoval) -> Result<Self, ModelError> {
        if self == Self::UnprotectedByOperator {
            return Ok(self);
        }
        self.checked(Self::UnprotectedByOperator)
    }

    /// A REST snapshot of the same instrument showed the `side` position
    /// gone: flat, or reversed by hand.
    pub fn closed(
        self,
        snapshot: &PositionSnapshot,
        side: PositionSide,
    ) -> Result<Self, ModelError> {
        let gone = snapshot.is_flat() || snapshot.side() == Some(side.opposite());
        if !gone {
            return Err(ModelError::MissingEvidence {
                machine: "position protection",
                to: Self::Closed.as_str(),
                evidence: format!("a snapshot without the {side} position"),
            });
        }
        self.checked(Self::Closed)
    }

    fn checked(self, to: Self) -> Result<Self, ModelError> {
        if !self.can_move_to(to) {
            return Err(ModelError::InvalidTransition {
                machine: "position protection",
                from: self.as_str(),
                to: to.as_str(),
            });
        }
        Ok(to)
    }
}

impl fmt::Display for ProtectionState {
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
            ProtectionState::Protected,
            ProtectionState::ProtectedByOperator,
            ProtectionState::UnprotectedByOperator,
            ProtectionState::Closed,
        ] {
            for from in ProtectionState::ALL {
                assert!(from.move_to(to).is_err(), "{from} -> {to}");
            }
        }
    }

    #[test]
    fn closed_is_the_only_terminal_state_and_every_other_state_has_a_way_out() {
        for state in ProtectionState::ALL {
            let leaves = ProtectionState::TRANSITIONS
                .iter()
                .any(|(from, _)| *from == state);
            assert_eq!(leaves, !state.is_terminal(), "{state}");
        }
        assert_eq!(
            ProtectionState::ALL
                .iter()
                .filter(|state| state.is_terminal())
                .count(),
            1
        );
    }

    #[test]
    fn a_close_in_progress_moves_without_evidence_only_where_the_table_allows() {
        let closing = ProtectionState::INITIAL
            .move_to(ProtectionState::Closing)
            .expect("closing");
        assert_eq!(
            closing.move_to(ProtectionState::Unprotected).ok(),
            Some(ProtectionState::Unprotected)
        );
        assert!(closing.move_to(ProtectionState::ProtectionPending).is_err());
    }
}
