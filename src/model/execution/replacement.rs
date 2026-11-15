//! A Spot replacement: the order it replaces, the order that replaces it,
//! and where the change stands.

use super::not_the_same;
use crate::model::identity::plain_text;
use crate::model::{ModelError, VenueOrderId};
use serde::{Deserialize, Serialize};
use std::fmt;

const RECORD: &str = "operation record";

/// Where a Spot replacement stands, as the parity matrix freezes it as
/// `spot_replace`. An acknowledgement is `pending` until a query or the
/// private stream confirms the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpotReplaceState {
    Pending,
    Accepted,
    /// One of the cancel and the new order took effect, and not the other.
    PartiallyApplied,
    Rejected,
    OutcomeUnknown,
}

impl SpotReplaceState {
    pub const INITIAL: Self = Self::Pending;

    pub const ALL: [Self; 5] = [
        Self::Pending,
        Self::Accepted,
        Self::PartiallyApplied,
        Self::Rejected,
        Self::OutcomeUnknown,
    ];

    /// Every allowed move, as the parity matrix lists them.
    pub const TRANSITIONS: [(Self, Self); 7] = [
        (Self::Pending, Self::Accepted),
        (Self::Pending, Self::PartiallyApplied),
        (Self::Pending, Self::Rejected),
        (Self::Pending, Self::OutcomeUnknown),
        (Self::OutcomeUnknown, Self::Accepted),
        (Self::OutcomeUnknown, Self::PartiallyApplied),
        (Self::OutcomeUnknown, Self::Rejected),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::PartiallyApplied => "partially_applied",
            Self::Rejected => "rejected",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::PartiallyApplied | Self::Rejected
        )
    }

    pub fn can_move_to(self, to: Self) -> bool {
        Self::TRANSITIONS.contains(&(self, to))
    }
}

impl fmt::Display for SpotReplaceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How an order is known: by the client ID it was sent with, by the ID
/// its venue gave it, or by both. A client ID stays raw text, as a venue
/// reports it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "OrderRefParts")]
pub struct OrderRef {
    client_order_id: Option<String>,
    venue_order_id: Option<VenueOrderId>,
}

impl OrderRef {
    /// Refuses a reference that names nothing, and a client ID that is
    /// empty, longer than 64 characters or holds spaces.
    pub fn new(
        client_order_id: Option<String>,
        venue_order_id: Option<VenueOrderId>,
    ) -> Result<Self, ModelError> {
        let named = client_order_id.is_some() || venue_order_id.is_some();
        let readable = client_order_id
            .as_deref()
            .is_none_or(|id| plain_text(id, 64));
        if !named || !readable {
            return Err(ModelError::InvalidIdentifier {
                what: "order reference",
                value: client_order_id.unwrap_or_default(),
                rule: "a client order id of 1 to 64 characters without spaces, a venue order \
                       id, or both",
            });
        }
        Ok(Self {
            client_order_id,
            venue_order_id,
        })
    }

    pub fn client_order_id(&self) -> Option<&str> {
        self.client_order_id.as_deref()
    }

    pub fn venue_order_id(&self) -> Option<&VenueOrderId> {
        self.venue_order_id.as_ref()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderRefParts {
    client_order_id: Option<String>,
    venue_order_id: Option<VenueOrderId>,
}

impl TryFrom<OrderRefParts> for OrderRef {
    type Error = ModelError;

    fn try_from(parts: OrderRefParts) -> Result<Self, Self::Error> {
        Self::new(parts.client_order_id, parts.venue_order_id)
    }
}

/// What a replacement ties together: the order it replaces, the order that
/// replaces it, and where the change stands. The original is recorded
/// before any request, so a restart knows which order to ask the venue
/// about before anything is sent again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    original: OrderRef,
    replacement: Option<OrderRef>,
    state: SpotReplaceState,
}

impl Replacement {
    /// A replacement of `original`, not yet answered.
    pub fn of(original: OrderRef) -> Self {
        Self {
            original,
            replacement: None,
            state: SpotReplaceState::INITIAL,
        }
    }

    pub fn original(&self) -> &OrderRef {
        &self.original
    }

    /// The order that replaces the original, once it has an identity.
    pub fn replacement(&self) -> Option<&OrderRef> {
        self.replacement.as_ref()
    }

    pub const fn state(&self) -> SpotReplaceState {
        self.state
    }

    /// The same replacement, its new order named. An amend in place names
    /// the original again.
    pub fn replaced_by(&self, order: OrderRef) -> Self {
        Self {
            replacement: Some(order),
            ..self.clone()
        }
    }

    /// The same replacement in `to`, where the state machine allows it.
    pub fn moved_to(&self, to: SpotReplaceState) -> Result<Self, ModelError> {
        if !self.state.can_move_to(to) {
            return Err(ModelError::InvalidTransition {
                machine: "spot replacement",
                from: self.state.as_str(),
                to: to.as_str(),
            });
        }
        Ok(Self {
            state: to,
            ..self.clone()
        })
    }

    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        if self.original != previous.original {
            return Err(not_the_same(RECORD, "replaced order"));
        }
        // A client ID never changes. The venue's ID is learned later, and
        // once learned it neither changes nor goes.
        let renamed = match (&previous.replacement, &self.replacement) {
            (Some(old), Some(new)) => {
                old.client_order_id != new.client_order_id
                    || (old.venue_order_id.is_some() && old.venue_order_id != new.venue_order_id)
            }
            (Some(_), None) => true,
            (None, _) => false,
        };
        if renamed {
            return Err(not_the_same(RECORD, "replacing order"));
        }
        if previous.state != self.state && !previous.state.can_move_to(self.state) {
            return Err(ModelError::InvalidTransition {
                machine: "spot replacement",
                from: previous.state.as_str(),
                to: self.state.as_str(),
            });
        }
        Ok(())
    }
}
