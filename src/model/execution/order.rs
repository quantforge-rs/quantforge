//! Orders and fills as the ledger keeps them: the identity an order was
//! sent with, recorded before the request, and what the venue has said of
//! it since.

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::futures::{FuturesOrder, StopOrder, StopStatus};
use crate::model::identity::plain_text;
use crate::model::{InstrumentId, ModelError, OperationId, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of [`OrderRecord`]'s stored form.
pub const ORDER_RECORD_FORMAT: u32 = 1;

const ORDER: &str = "order record";

/// What a venue has reported of an order, read back from a query or its
/// private stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OrderEvidence {
    Futures(Box<FuturesOrder>),
    Stop(Box<StopOrder>),
}

impl OrderEvidence {
    /// Whether the report is the last the order has: it can fill no
    /// more, or its stop has ended. A later report that says otherwise is
    /// an older one arriving late.
    pub fn is_final(&self) -> bool {
        match self {
            Self::Futures(order) => order.status.is_final(),
            Self::Stop(stop) => !matches!(stop.status, StopStatus::Active | StopStatus::Triggered),
        }
    }
}

/// A venue's definitive refusal of an order, as the ledger keeps it: the
/// exchange error kind and what it said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderRefusal {
    pub kind: String,
    pub message: String,
}

/// How much is known of an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderPhase {
    /// Recorded, and perhaps sent: nothing has answered for it yet. After
    /// a restart it is looked up before anything is sent again.
    Recorded,
    /// The request ended with no answer that says what became of it.
    OutcomeUnknown,
    /// The venue accepted it and gave it an ID. That is not a fill.
    Acknowledged,
    /// The venue refused it, and executed nothing.
    Refused,
    /// The venue has reported the order itself.
    Reported,
}

impl OrderPhase {
    pub const ALL: [Self; 5] = [
        Self::Recorded,
        Self::OutcomeUnknown,
        Self::Acknowledged,
        Self::Refused,
        Self::Reported,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Acknowledged => "acknowledged",
            Self::Refused => "refused",
            Self::Reported => "reported",
        }
    }
}

impl fmt::Display for OrderPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One order of an operation: the client ID it carries, recorded before
/// its request is sent, and what has been learned of it. A client ID is
/// recorded once in a store, so no order is ever sent under an ID another
/// carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "OrderRecordParts")]
pub struct OrderRecord {
    format: u32,
    scope: ExecutionScope,
    operation: OperationId,
    client_order_id: String,
    instrument: InstrumentId,
    recorded_at_ms: TimestampMs,
    venue_order_id: Option<String>,
    outcome_unknown: bool,
    refusal: Option<OrderRefusal>,
    evidence: Option<OrderEvidence>,
}

impl OrderRecord {
    /// An order about to be sent as `client_order_id`. Refuses an ID that
    /// is empty, longer than 64 characters or holds spaces.
    pub fn recorded(
        scope: ExecutionScope,
        operation: OperationId,
        client_order_id: impl Into<String>,
        instrument: InstrumentId,
        recorded_at_ms: TimestampMs,
    ) -> Result<Self, ModelError> {
        Self::try_from(OrderRecordParts {
            format: ORDER_RECORD_FORMAT,
            scope,
            operation,
            client_order_id: client_order_id.into(),
            instrument,
            recorded_at_ms,
            venue_order_id: None,
            outcome_unknown: false,
            refusal: None,
            evidence: None,
        })
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    pub fn client_order_id(&self) -> &str {
        &self.client_order_id
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    /// When the order was recorded, just before its request.
    pub const fn recorded_at_ms(&self) -> TimestampMs {
        self.recorded_at_ms
    }

    /// The venue's own ID for the order, an algo order's included.
    pub fn venue_order_id(&self) -> Option<&str> {
        self.venue_order_id.as_deref()
    }

    pub fn refusal(&self) -> Option<&OrderRefusal> {
        self.refusal.as_ref()
    }

    pub fn evidence(&self) -> Option<&OrderEvidence> {
        self.evidence.as_ref()
    }

    /// How much is known of the order: what the venue reported comes
    /// first, then its refusal, then its acknowledgement.
    pub const fn phase(&self) -> OrderPhase {
        if self.evidence.is_some() {
            OrderPhase::Reported
        } else if self.refusal.is_some() {
            OrderPhase::Refused
        } else if self.venue_order_id.is_some() {
            OrderPhase::Acknowledged
        } else if self.outcome_unknown {
            OrderPhase::OutcomeUnknown
        } else {
            OrderPhase::Recorded
        }
    }

    /// The order once the venue accepted it as `venue_order_id`.
    pub fn acknowledged(&self, venue_order_id: impl Into<String>) -> Result<Self, ModelError> {
        self.changed(|parts| parts.venue_order_id = Some(venue_order_id.into()))
    }

    /// The order once its request ended with no answer.
    pub fn unknown(&self) -> Result<Self, ModelError> {
        self.changed(|parts| parts.outcome_unknown = true)
    }

    /// The order once the venue refused it.
    pub fn refused(&self, refusal: OrderRefusal) -> Result<Self, ModelError> {
        self.changed(|parts| parts.refusal = Some(refusal))
    }

    /// The order with the venue's newest report of it.
    pub fn reported(&self, evidence: OrderEvidence) -> Result<Self, ModelError> {
        let venue_order_id = match &evidence {
            OrderEvidence::Futures(order) => Some(order.venue_order_id.as_str().to_string()),
            OrderEvidence::Stop(_) => self.venue_order_id.clone(),
        };
        self.changed(|parts| {
            parts.venue_order_id = venue_order_id;
            parts.evidence = Some(evidence);
        })
    }

    fn changed(&self, change: impl FnOnce(&mut OrderRecordParts)) -> Result<Self, ModelError> {
        let mut parts = OrderRecordParts {
            format: self.format,
            scope: self.scope.clone(),
            operation: self.operation,
            client_order_id: self.client_order_id.clone(),
            instrument: self.instrument.clone(),
            recorded_at_ms: self.recorded_at_ms,
            venue_order_id: self.venue_order_id.clone(),
            outcome_unknown: self.outcome_unknown,
            refusal: self.refusal.clone(),
            evidence: self.evidence.clone(),
        };
        change(&mut parts);
        let next = Self::try_from(parts)?;
        next.follows(self)?;
        Ok(next)
    }

    /// Whether `self` may replace `previous`: the same order, with nothing
    /// it had learned taken back, a final report among it.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let same = self.scope == previous.scope
            && self.operation == previous.operation
            && self.client_order_id == previous.client_order_id
            && self.instrument == previous.instrument
            && self.recorded_at_ms == previous.recorded_at_ms;
        if !same {
            return Err(not_the_same(ORDER, "identity"));
        }
        let reopened = match (&previous.evidence, &self.evidence) {
            (Some(held), Some(next)) => held.is_final() && !next.is_final(),
            (Some(_), None) => true,
            (None, _) => false,
        };
        let forgotten = (previous.venue_order_id.is_some()
            && self.venue_order_id != previous.venue_order_id)
            || reopened
            || (previous.outcome_unknown && !self.outcome_unknown)
            || (previous.refusal.is_some() && self.refusal != previous.refusal);
        if forgotten {
            return Err(disagrees(
                ORDER,
                "it takes back what the record it follows had learned of the order",
            ));
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(ORDER, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(ORDER, ORDER_RECORD_FORMAT, text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderRecordParts {
    format: u32,
    scope: ExecutionScope,
    operation: OperationId,
    client_order_id: String,
    instrument: InstrumentId,
    recorded_at_ms: TimestampMs,
    venue_order_id: Option<String>,
    outcome_unknown: bool,
    refusal: Option<OrderRefusal>,
    evidence: Option<OrderEvidence>,
}

impl TryFrom<OrderRecordParts> for OrderRecord {
    type Error = ModelError;

    fn try_from(parts: OrderRecordParts) -> Result<Self, Self::Error> {
        if parts.format != ORDER_RECORD_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: ORDER,
                found: parts.format,
                expected: ORDER_RECORD_FORMAT,
            });
        }
        parts.scope.holding(ORDER, &parts.instrument)?;
        let ids = [Some(&parts.client_order_id), parts.venue_order_id.as_ref()];
        if let Some(unreadable) = ids.into_iter().flatten().find(|id| !plain_text(id, 128)) {
            return Err(ModelError::InvalidIdentifier {
                what: "order id",
                value: unreadable.clone(),
                rule: "1 to 128 characters without spaces",
            });
        }
        // What the venue reported is of this order: its instrument, and for
        // an order its client ID and the venue's own.
        let reported = match &parts.evidence {
            Some(OrderEvidence::Futures(order)) => {
                order.instrument == parts.instrument
                    && order.client_id.as_deref() == Some(&parts.client_order_id)
                    && parts.venue_order_id.as_deref() == Some(order.venue_order_id.as_str())
            }
            Some(OrderEvidence::Stop(stop)) => stop.instrument == parts.instrument,
            None => true,
        };
        if !reported {
            return Err(disagrees(
                ORDER,
                "the order the venue reported is not the one the record is of",
            ));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            operation: parts.operation,
            client_order_id: parts.client_order_id,
            instrument: parts.instrument,
            recorded_at_ms: parts.recorded_at_ms,
            venue_order_id: parts.venue_order_id,
            outcome_unknown: parts.outcome_unknown,
            refusal: parts.refusal,
            evidence: parts.evidence,
        })
    }
}

#[cfg(test)]
mod tests;
