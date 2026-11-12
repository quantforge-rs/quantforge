//! A sized entry held to the prices read just before it is sent (ADR
//! 0002 §9): refused when the mark price or the touch has reached the
//! stop, or when the risk at the expected fill has drifted past the limit.

use super::fault::{SizingFault, SizingInput};
use super::figures::SizedEntry;
use super::request::Freshness;
use super::{amount, unit_loss};
use crate::model::exact::Exact;
use crate::model::futures::PriceSnapshot;
use crate::model::{Money, PositionSide, Price, Refusal, TimestampMs};
use rust_decimal::Decimal;
use std::fmt;

/// How far the risk at the expected fill may exceed the budget before an
/// entry is refused: 1.25 × R. The parity matrix freezes the same number.
pub const DRIFT_LIMIT: Decimal = Decimal::from_parts(125, 0, 0, false, 2);

/// What the prices just before submission say of a sized entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmissionCheck {
    /// Neither the mark price nor the touch has reached the stop, and the
    /// risk at the expected fill is within the drift limit: the entry may
    /// be sent.
    Clear(FillEstimate),
    /// The entry is not sent.
    Refused(SubmissionRefusal),
}

/// Where a market entry would fill, and what it would risk from there.
/// An estimate: the venue's fill is reported by [`SizedEntry::fill_risk`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillEstimate {
    /// The ask for a long, the bid for a short.
    pub price: Price,
    /// The quantity's loss from that price to the stop.
    pub risk: Money,
}

/// Why a sized entry is not sent: `stop_crossed` or `risk_drift`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmissionRefusal {
    pub refusal: Refusal,
    pub message: String,
    /// The risk at the expected fill, where the stop had not been reached.
    pub risk_at_fill: Option<Money>,
}

impl fmt::Display for SubmissionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.refusal, self.message)
    }
}

impl std::error::Error for SubmissionRefusal {}

impl SizedEntry {
    /// Holds the entry to `prices`, which must be its own instrument's and
    /// fresh: a mark price and a book each within `freshness.prices_ms` of
    /// `now_ms`. Prices that are another instrument's, incomplete, crossed
    /// or old are a fault, and nothing is sent on a fault.
    pub fn at_submission(
        &self,
        prices: &PriceSnapshot,
        now_ms: TimestampMs,
        freshness: &Freshness,
    ) -> Result<SubmissionCheck, SizingFault> {
        if prices.instrument != self.instrument {
            return Err(SizingFault::mismatched(
                SizingInput::Prices,
                format!(
                    "they are {}'s, and the entry is on {}",
                    prices.instrument, self.instrument
                ),
            ));
        }
        let (Some(bid), Some(ask), Some(quoted_at_ms)) =
            (prices.bid, prices.ask, prices.quoted_at_ms)
        else {
            return Err(SizingFault::missing(
                SizingInput::Prices,
                "the snapshot lacks a bid, an ask or the time its book was read",
            ));
        };
        if bid > ask {
            return Err(SizingFault::invalid(
                SizingInput::Prices,
                format!("a bid of {bid} above an ask of {ask} is not a book"),
            ));
        }
        for (part, at_ms) in [
            ("the mark price, ", prices.marked_at_ms),
            ("the book, ", quoted_at_ms),
        ] {
            let limit = freshness.prices_ms;
            SizingFault::unless_fresh(SizingInput::Prices, part, at_ms, now_ms, limit)?;
        }

        // The stop triggers on the mark price; the touch is where a
        // market entry trades against. Either at or past the stop refuses.
        let stop = self.stop.price;
        let long = self.side == PositionSide::Long;
        let (touch, touch_name, fill, fill_name) = if long {
            (bid, "bid", ask, "ask")
        } else {
            (ask, "ask", bid, "bid")
        };
        for (price, name) in [(prices.mark, "mark price"), (touch, touch_name)] {
            let reached = if long { price <= stop } else { price >= stop };
            if reached {
                return Ok(SubmissionCheck::Refused(SubmissionRefusal {
                    refusal: Refusal::StopCrossed,
                    message: format!("the {name} {price} has reached the stop {stop}"),
                    risk_at_fill: None,
                }));
            }
        }

        let face = self.face_value.map(|face| Exact::magnitude(face.value()));
        let each = unit_loss(
            &Exact::magnitude(fill.value()),
            &Exact::magnitude(stop.value()),
            face.as_ref(),
        )?;
        let at_fill = Exact::magnitude(self.quantity().value()).times(&each);
        let risk = amount(&at_fill, self.budget.asset(), "risk at the fill")?;
        if at_fill > self.exact_budget.times(&Exact::magnitude(DRIFT_LIMIT)) {
            return Ok(SubmissionCheck::Refused(SubmissionRefusal {
                refusal: Refusal::RiskDrift,
                message: format!(
                    "at the {fill_name} {fill} the entry would risk {risk}, more than \
                     {DRIFT_LIMIT} × its budget of {}",
                    self.budget
                ),
                risk_at_fill: Some(risk),
            }));
        }
        Ok(SubmissionCheck::Clear(FillEstimate { price: fill, risk }))
    }
}
