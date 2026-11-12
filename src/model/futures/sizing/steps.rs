//! The rules of ADR 0002 §6, one after another: the budget, the stop on
//! its tick, the quantity on its step, what it is worth, the margin it
//! needs, the bracket, the pool and the wallet. Every comparison is made
//! on exact values; only what is reported is rounded.

use super::fault::SizingFault;
use super::figures::{
    MarginNeed, Notional, PoolFit, SizedEntry, SizedPosition, SizedStop, Sizing, SizingFigures,
    SizingRefusal, SizingWarning,
};
use super::funding::{self, Funding};
use super::request::SizingRequest;
use super::{amount, signed_amount, unit_loss};
use crate::model::exact::Exact;
use crate::model::futures::EntryStop;
use crate::model::{PositionSide, Price, Quantity, Refusal};
use rust_decimal::Decimal;

/// Why the steps end before an entry is sized.
enum Halt {
    Refused(Refusal, String),
    Fault(SizingFault),
}

impl From<SizingFault> for Halt {
    fn from(fault: SizingFault) -> Self {
        Self::Fault(fault)
    }
}

fn refuse<T>(refusal: Refusal, message: String) -> Result<T, Halt> {
    Err(Halt::Refused(refusal, message))
}

pub(super) fn size(request: &SizingRequest<'_>) -> Result<Sizing, SizingFault> {
    let funding = funding::trusted(request)?;
    let policy = request.policy;
    let settle = &policy.instrument().settle;
    // R = B × r, with r in percent.
    let budget = Exact::magnitude(policy.capital().amount())
        .times(&Exact::magnitude(policy.risk_percent().value.value()))
        .over(&Exact::whole(100))
        .ok_or_else(|| SizingFault::out_of_range("risk budget"))?;
    let mut figures = SizingFigures {
        budget: amount(&budget, settle, "risk budget")?,
        stop: None,
        position: None,
        notional: None,
        margin: None,
        bracket_cap: None,
        pool: None,
    };
    match sized(request, &funding, budget, &mut figures) {
        Ok(entry) => Ok(Sizing::Sized(Box::new(entry))),
        Err(Halt::Refused(refusal, message)) => Ok(Sizing::Refused(Box::new(SizingRefusal {
            refusal,
            message,
            figures,
        }))),
        Err(Halt::Fault(fault)) => Err(fault),
    }
}

/// What a pool has free: its available margin less what is reserved, and
/// whether that is below zero.
fn free_margin(available: Decimal, reserved: Decimal) -> (Exact, bool) {
    let (held, promised) = (Exact::magnitude(available), Exact::magnitude(reserved));
    if available.is_sign_negative() && !available.is_zero() {
        return (held.plus(&promised), true);
    }
    let overdrawn = held < promised;
    (held.distance(&promised), overdrawn)
}

fn sized(
    request: &SizingRequest<'_>,
    funding: &Funding<'_>,
    budget: Exact,
    figures: &mut SizingFigures,
) -> Result<SizedEntry, Halt> {
    let policy = request.policy;
    let spec = request.instrument.spec();
    let (settle, symbol) = (&spec.id.settle, &spec.id.symbol);
    let long = request.side == PositionSide::Long;
    let entry = request.expected_price;
    let e = Exact::magnitude(entry.value());

    // The stop: the alert's own first, else the policy's fallback
    // percentage of E, else none. The fallback never replaces a stop the
    // alert brought, even one on the wrong side.
    let asked = policy
        .stop_for(request.signal_stop)
        .or_else(|refusal| refuse(refusal.refusal, refusal.message))?;
    let wanted = match asked {
        EntryStop::Signal(stop) => {
            let protective = if long { stop < entry } else { stop > entry };
            if !protective {
                let side = if long { "below" } else { "above" };
                return refuse(
                    Refusal::StopWrongSide,
                    format!(
                        "the alert's stop {stop} is not {side} its expected entry {entry}, as a \
                         {}'s must be",
                        request.side
                    ),
                );
            }
            Exact::magnitude(stop.value())
        }
        EntryStop::Fallback(percent) => {
            let hundred = Exact::whole(100);
            let share = Exact::magnitude(percent.value());
            // E × (1 − p) for a long, E × (1 + p) for a short.
            let kept = if long {
                hundred.distance(&share)
            } else {
                hundred.plus(&share)
            };
            e.times(&kept)
                .over(&hundred)
                .ok_or_else(|| SizingFault::out_of_range("stop"))?
        }
    };
    // To the tick, away from the entry: never tighter than asked.
    let tick = spec.tick.value();
    let on_tick = if long {
        wanted.floor_on(tick)
    } else {
        wanted.ceil_on(tick)
    }
    .ok_or_else(|| SizingFault::out_of_range("stop"))?;
    let Ok(price) = Price::new(on_tick) else {
        return refuse(
            Refusal::NoStop,
            format!(
                "the stop rounds down to no price on {symbol}'s tick of {tick}, so the entry has \
                 no stop the venue could hold"
            ),
        );
    };
    let stop = SizedStop { price, asked };
    figures.stop = Some(stop);
    let s = Exact::magnitude(price.value());
    let face = spec
        .inverse
        .map(|inverse| Exact::magnitude(inverse.face_value.value()));

    // The quantity: the budget over one unit's loss at the stop, floored
    // to the market-order step. Its risk is recomputed from what is left.
    let unit = unit_loss(&e, &s, face.as_ref())?;
    let floored = budget
        .over(&unit)
        .and_then(|count| count.floor_on(spec.market_step.value()))
        .ok_or_else(|| SizingFault::out_of_range("quantity"))?;
    let quantity = Quantity::for_category(spec.id.category(), floored)
        .map_err(|_| SizingFault::out_of_range("quantity"))?;
    let q = Exact::magnitude(floored);
    let position = SizedPosition {
        quantity,
        unit_loss: amount(&unit, settle, "unit loss")?,
        risk: amount(&q.times(&unit), settle, "risk")?,
    };
    figures.position = Some(position.clone());
    let (least, most) = (spec.market_min_qty, spec.market_max_qty);
    if quantity.is_zero() || floored < least.value() {
        return refuse(
            Refusal::BelowMinQty,
            format!(
                "a budget of {} sizes {quantity} of {symbol}, below its market-order minimum of \
                 {least}; a quantity is never rounded up",
                figures.budget
            ),
        );
    }
    if floored > most.value() {
        return refuse(
            Refusal::AboveMarketMax,
            format!(
                "the entry sizes {quantity} of {symbol}, above its market-order maximum of \
                 {most}; lower the capital or the risk percentage"
            ),
        );
    }

    // What it is worth at E: in the quote asset, and in the settlement
    // asset, which for an inverse contract is the coin.
    let (worth, settled) = match &face {
        None => (q.times(&e), q.times(&e)),
        Some(face) => {
            let worth = q.times(face);
            let settled = worth
                .over(&e)
                .ok_or_else(|| SizingFault::out_of_range("notional"))?;
            (worth, settled)
        }
    };
    let notional = Notional {
        quote: amount(&worth, &spec.quote, "notional")?,
        settle: amount(&settled, settle, "notional")?,
    };
    figures.notional = Some(notional.clone());
    if let Some(least) = &spec.min_notional {
        if worth < Exact::magnitude(least.amount()) {
            return refuse(
                Refusal::BelowMinNotional,
                format!(
                    "{quantity} of {symbol} is worth {}, below its minimum notional of {least}",
                    notional.quote
                ),
            );
        }
    }

    // The margin: leverage changes it, never the quantity.
    let leverage = policy.leverage().value;
    let initial = settled
        .over(&Exact::magnitude(leverage.value()))
        .ok_or_else(|| SizingFault::out_of_range("initial margin"))?;
    let fees = settled
        .times(&Exact::magnitude(request.fees.value.taker))
        .times(&Exact::whole(2));
    let required = initial.plus(&fees);
    let margin = MarginNeed {
        initial: amount(&initial, settle, "initial margin")?,
        fees: amount(&fees, settle, "fee allowance")?,
        required: amount(&required, settle, "required margin")?,
    };
    figures.margin = Some(margin.clone());

    // The bracket: the largest cap among the tiers that allow the
    // leverage. A position it cannot hold is refused, never shrunk.
    let allowed = request
        .brackets
        .value
        .iter()
        .filter(|tier| tier.max_leverage >= leverage)
        .map(|tier| tier.cap.amount())
        .max();
    let Some(cap) = allowed else {
        return refuse(
            Refusal::AboveLeverageBracket,
            format!("no leverage bracket of {symbol} allows {leverage}x; lower the leverage"),
        );
    };
    let cap = Exact::magnitude(cap);
    let bracket_cap = amount(&cap, settle, "bracket cap")?;
    figures.bracket_cap = Some(bracket_cap.clone());
    if settled > cap {
        return refuse(
            Refusal::AboveLeverageBracket,
            format!(
                "a position of {} is above the {bracket_cap} that {symbol}'s brackets allow at \
                 {leverage}x; lower the leverage, the capital or the risk percentage",
                notional.settle
            ),
        );
    }

    // The pool: the margin, in the pool's own unit, against what the
    // pool has free. A USD pool is measured at the venue's index price.
    let pool = funding.pool;
    let unit_of_pool = pool.available.asset();
    let needed = match funding.index {
        Some(index) => required.times(&Exact::magnitude(index.price.value())),
        None => required,
    };
    let (free, overdrawn) = free_margin(pool.available.amount(), request.reserved.amount());
    let fit = PoolFit {
        scope: pool.scope.clone(),
        required: amount(&needed, unit_of_pool, "margin in the pool")?,
        free: signed_amount(&free, overdrawn, unit_of_pool, "free margin")?,
        index: funding.index.cloned(),
    };
    figures.pool = Some(fit.clone());
    if overdrawn || needed > free {
        return refuse(
            Refusal::InsufficientMargin,
            format!(
                "the entry needs {} of margin, and the pool has {} free: {} available less {} \
                 reserved",
                fit.required, fit.free, pool.available, request.reserved
            ),
        );
    }

    // The wallet: a stop-out is paid in the settlement asset, whatever
    // collateral funds the margin.
    let balance = funding
        .wallet
        .map_or(Decimal::ZERO, |wallet| wallet.balance.amount());
    let owing = balance.is_sign_negative() && !balance.is_zero();
    if owing || Exact::magnitude(balance) < budget {
        let cost = &figures.budget;
        let message = match funding.wallet {
            Some(wallet) => format!(
                "the {settle} wallet holds {}, less than the {cost} a stop-out would cost",
                wallet.balance
            ),
            None => {
                format!("the account holds no {settle} to pay the {cost} a stop-out would cost")
            }
        };
        return refuse(Refusal::SettlementWalletBelowRisk, message);
    }
    let mut warnings = Vec::new();
    if balance < policy.capital().amount() {
        warnings.push(SizingWarning::WalletBelowCapital);
    }

    Ok(SizedEntry {
        instrument: spec.id.clone(),
        side: request.side,
        expected_price: entry,
        face_value: spec.inverse.map(|inverse| inverse.face_value),
        leverage,
        exact_budget: budget,
        budget: figures.budget.clone(),
        stop,
        position,
        notional,
        margin,
        bracket_cap,
        pool: fit,
        warnings,
    })
}
