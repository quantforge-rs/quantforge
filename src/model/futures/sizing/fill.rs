//! The risk an entry actually took, once it has filled. Sizing works from
//! the alert's expected price; the venue fills where the market is. The
//! two are kept apart and reported side by side.

use super::fault::{SizingFault, SizingInput};
use super::figures::SizedEntry;
use super::{amount, signed_amount, unit_loss};
use crate::model::exact::Exact;
use crate::model::{Money, PositionSide, Price, Quantity};
use rust_decimal::Decimal;

/// What an entry's fills risk to its stop, beside what the same quantity
/// would have risked at the alert's expected price.
///
/// None of it is a loss limit. A stop is a market order once it triggers:
/// a gap fills it past its price, and fees and funding come on top, so a
/// stop-out can cost more than `actual`, and more than the budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillRisk {
    /// How much filled: at most the quantity sized.
    pub filled: Quantity,
    /// What that quantity would lose from E to the stop.
    pub expected: Money,
    /// What it loses from where it filled, each fill at its own price. A
    /// fill at or past the stop has nothing left to lose to it and counts
    /// for nothing here.
    pub actual: Money,
    /// `actual − expected`: above zero when the fills were worse than E.
    pub deviation: Money,
    /// R, the budget the entry was sized to.
    pub budget: Money,
    /// Whether `actual` is above the budget.
    pub exceeds_budget: bool,
    /// Whether any fill was at or past the stop: the position was past
    /// its stop the moment it opened.
    pub past_stop: bool,
}

impl SizedEntry {
    /// The risk the entry's fills took. `fills` are the venue's own, each
    /// a price and a quantity in the unit the instrument trades in; they
    /// may fill less than was sized, never more.
    pub fn fill_risk(&self, fills: &[(Price, Quantity)]) -> Result<FillRisk, SizingFault> {
        let category = self.instrument.category();
        let sized = self.quantity();
        if fills.is_empty() {
            return Err(SizingFault::missing(
                SizingInput::Fills,
                "no fill was given",
            ));
        }
        let mut total = Exact::whole(0);
        let mut places = 0;
        for (_, quantity) in fills {
            if quantity.in_unit_of(category).is_err() {
                return Err(SizingFault::invalid(
                    SizingInput::Fills,
                    format!(
                        "a fill of {quantity} is in {}, and the entry is sized in {}",
                        quantity.unit(),
                        sized.unit()
                    ),
                ));
            }
            total = total.plus(&Exact::magnitude(quantity.value()));
            places = places.max(quantity.value().scale());
        }
        // The sum in the finest scale a fill came in: exact, or a fault.
        let filled = Decimal::try_new(1, places)
            .ok()
            .and_then(|grid| total.floor_on(grid))
            .and_then(|sum| Quantity::for_category(category, sum).ok())
            .ok_or_else(|| SizingFault::out_of_range("filled quantity"))?;
        if filled.is_zero() {
            return Err(SizingFault::missing(
                SizingInput::Fills,
                "the fills add up to nothing",
            ));
        }
        if filled.value() > sized.value() {
            return Err(SizingFault::invalid(
                SizingInput::Fills,
                format!("they add up to {filled}, more than the {sized} the entry was sized for"),
            ));
        }

        let stop = self.stop.price;
        let long = self.side == PositionSide::Long;
        let s = Exact::magnitude(stop.value());
        let face = self.face_value.map(|face| Exact::magnitude(face.value()));
        let mut actual = Exact::whole(0);
        let mut past_stop = false;
        for (price, quantity) in fills {
            let beyond = if long { *price <= stop } else { *price >= stop };
            if beyond {
                past_stop = true;
                continue;
            }
            let each = unit_loss(&Exact::magnitude(price.value()), &s, face.as_ref())?;
            actual = actual.plus(&Exact::magnitude(quantity.value()).times(&each));
        }
        let at_expected = unit_loss(
            &Exact::magnitude(self.expected_price.value()),
            &s,
            face.as_ref(),
        )?;
        let expected = Exact::magnitude(filled.value()).times(&at_expected);
        let settle = self.budget.asset();
        Ok(FillRisk {
            filled,
            expected: amount(&expected, settle, "expected risk")?,
            actual: amount(&actual, settle, "actual risk")?,
            deviation: signed_amount(
                &actual.distance(&expected),
                actual < expected,
                settle,
                "risk deviation",
            )?,
            budget: self.budget.clone(),
            exceeds_budget: actual > self.exact_budget,
            past_stop,
        })
    }
}
