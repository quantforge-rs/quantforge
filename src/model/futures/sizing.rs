//! Risk sizing (ADR 0002 §6, §9): how much an admitted entry buys or
//! sells, worked out from the stop it would be closed at. An entry risks a
//! share of its profile's capital; sizing turns that budget, the alert's
//! expected price and the stop into a quantity the venue takes, and says
//! what margin it needs and whether the account can fund it.
//!
//! [`SizingRequest::size`] is a pure function of what it is handed: the
//! policy the entry was admitted under, the instrument's rules, the
//! account's brackets and fees, an account snapshot and what is already
//! reserved against it, and the clock. It answers one of three things:
//!
//! - **sized** ([`SizedEntry`]): the quantity, with every figure behind it;
//! - **refused** ([`SizingRefusal`]): one of the parity matrix's refusals,
//!   with the figures reached before it. An entry is never shrunk to fit;
//! - **a fault** ([`SizingFault`]): an input sizing cannot trust, because
//!   it is for something else, too old, missing, ambiguous or invalid.
//!   Faults are looked for first, in one fixed order, and are not refusals.
//!
//! The rules, with E the alert's expected price:
//!
//! 1. the budget is `R = B × r`: the capital in the settlement asset times
//!    the risk percentage. Capital is never converted between assets;
//! 2. the stop is the alert's own, which must lie on the protective side,
//!    or else the policy's fallback: `E × (1 − p)` for a long, `E × (1 +
//!    p)` for a short. It is rounded to the tick away from the entry;
//! 3. a linear quantity is `R / |E − S′|`; an inverse contract count is
//!    `R / (F × |1/E − 1/S′|)`, with F the venue's face value. Both are
//!    floored to the market-order step, and the risk is recomputed;
//! 4. the quantity must reach the market-order minimum, stay within the
//!    maximum, and be worth the minimum notional;
//! 5. the margin is the notional in the settlement asset over the
//!    leverage, plus taker fees for opening and closing. Leverage changes
//!    the margin, never the quantity, and the position must fit the
//!    largest bracket that allows the leverage;
//! 6. the margin must fit what the entry's pool has free once
//!    reservations are taken off. A pool in USD is measured at the venue's
//!    own index price of the settlement asset;
//! 7. the settlement asset's wallet must hold at least R, since a
//!    stop-out is paid in that asset whatever funds the margin.
//!
//! Then, just before the entry is sent, [`SizedEntry::at_submission`]
//! holds it to fresh prices, and once it has filled
//! [`SizedEntry::fill_risk`] reports the risk actually taken beside the
//! one sized. The budget is not a loss limit: a gap, fees and funding can
//! each take a stop-out past it.
//!
//! Every decision is made on exact values. What is reported is exact to
//! [`AMOUNT_PLACES`] decimal places and rounded half to even beyond them;
//! a stop keeps its tick's scale and a quantity its step's.
//! `docs/risk-sizing.md` is the contract, and the cases an independent
//! model wrote (`tests/fixtures/sizing_cases.json`) hold this to it.

mod fault;
mod figures;
mod fill;
mod funding;
mod request;
mod steps;
mod submission;

use crate::model::exact::Exact;
use crate::model::{Asset, Money};

pub use fault::{SizingFault, SizingFaultKind, SizingInput};
pub use figures::{
    MarginNeed, Notional, PoolFit, SizedEntry, SizedPosition, SizedStop, Sizing, SizingFigures,
    SizingRefusal, SizingWarning,
};
pub use fill::FillRisk;
pub use funding::margin_pool;
pub use request::{
    ACCOUNT_FRESH_MS, Freshness, Observed, PRICES_FRESH_MS, RULES_FRESH_MS, SizingRequest, UsdIndex,
};
pub use submission::{DRIFT_LIMIT, FillEstimate, SubmissionCheck, SubmissionRefusal};

/// How many decimal places a reported amount keeps. An amount with no
/// more than these is reported exactly; one with more is rounded half to
/// even, as a division's result has to be somewhere.
pub const AMOUNT_PLACES: u32 = 12;

/// `value` of `asset` as a figure is reported.
fn amount(value: &Exact, asset: &Asset, figure: &'static str) -> Result<Money, SizingFault> {
    signed_amount(value, false, asset, figure)
}

/// The same, below zero where `negative` says so.
fn signed_amount(
    value: &Exact,
    negative: bool,
    asset: &Asset,
    figure: &'static str,
) -> Result<Money, SizingFault> {
    let size = value
        .rounded(AMOUNT_PLACES)
        .ok_or_else(|| SizingFault::out_of_range(figure))?;
    Ok(Money::new(
        if negative { -size } else { size },
        asset.clone(),
    ))
}

/// What one unit loses between `price` and `stop`, in the settlement
/// asset: their distance for a linear contract, and `F × |1/price −
/// 1/stop|` for an inverse one of face value `face`.
fn unit_loss(price: &Exact, stop: &Exact, face: Option<&Exact>) -> Result<Exact, SizingFault> {
    let apart = price.distance(stop);
    match face {
        None => Ok(apart),
        // |1/a − 1/b| is |a − b| / (a × b): one division, and exact.
        Some(face) => face
            .times(&apart)
            .over(&price.times(stop))
            .ok_or_else(|| SizingFault::out_of_range("unit loss")),
    }
}
