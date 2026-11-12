//! What sizing is given: the entry and the policy it was admitted under,
//! what the venue says of the instrument and of the account, what is
//! already promised from the account's margin, and the clock. All of it is
//! handed in; sizing reads nothing itself.

use super::fault::SizingFault;
use super::figures::Sizing;
use crate::model::futures::{
    AccountSnapshot, FeeRates, FuturesInstrument, LeverageBracket, PolicySnapshot,
};
use crate::model::{Asset, Money, PositionSide, Price, TimestampMs};

/// How old a mark price, a book or an index price may be: five seconds.
pub const PRICES_FRESH_MS: i64 = 5_000;

/// How old an account snapshot may be: ten seconds.
pub const ACCOUNT_FRESH_MS: i64 = 10_000;

/// How old an instrument's rules, the account's leverage brackets and its
/// fee rates may be: an hour.
pub const RULES_FRESH_MS: i64 = 3_600_000;

/// How old each kind of input may be before sizing no longer trusts it.
/// An input counts as fresh while its time is within its limit of the
/// clock, on either side: one timed further ahead than that says the
/// clocks disagree, and is trusted no more than an old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freshness {
    /// The mark price, the book and a USD index price.
    pub prices_ms: i64,
    /// The account snapshot: its wallets and its margin pools.
    pub account_ms: i64,
    /// The instrument's rules, the leverage brackets and the fee rates.
    pub rules_ms: i64,
}

impl Default for Freshness {
    fn default() -> Self {
        Self {
            prices_ms: PRICES_FRESH_MS,
            account_ms: ACCOUNT_FRESH_MS,
            rules_ms: RULES_FRESH_MS,
        }
    }
}

/// A value with the time it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed<T> {
    pub value: T,
    pub at_ms: TimestampMs,
}

/// What one unit of a settlement asset is worth in USD by the venue's own
/// index: how an entry's margin is measured against an account whose
/// margin is one USD pool. It is the venue's figure, never a rate from
/// anywhere else, and it says which index it is and when it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsdIndex {
    pub asset: Asset,
    /// USD for one unit of the asset.
    pub price: Price,
    /// The venue's name for the index, as its adapter read it.
    pub source: String,
    pub observed_at_ms: TimestampMs,
}

/// Everything one sizing reads.
#[derive(Clone, Debug)]
pub struct SizingRequest<'a> {
    /// The policy the entry was admitted under: its capital, risk
    /// percentage, fallback stop and leverage, as they stood then.
    pub policy: &'a PolicySnapshot,
    pub side: PositionSide,
    /// E, the alert's expected entry price. The quantity is sized from it;
    /// the venue's fill is reported apart, by [`SizedEntry::fill_risk`].
    ///
    /// [`SizedEntry::fill_risk`]: super::SizedEntry::fill_risk
    pub expected_price: Price,
    /// The alert's absolute stop, if it gave one. It is used before the
    /// policy's fallback, and never replaced by it.
    pub signal_stop: Option<Price>,
    /// The instrument as its venue lists it now.
    pub instrument: &'a FuturesInstrument,
    /// The account's leverage brackets for the instrument.
    pub brackets: Observed<&'a [LeverageBracket]>,
    /// The account's fee rates for the instrument.
    pub fees: Observed<FeeRates>,
    /// The account as the venue reports it: an immutable snapshot.
    pub account: &'a AccountSnapshot,
    /// Margin already reserved against the entry's pool for the same
    /// account, in the pool's unit. [`margin_pool`] names the pool.
    ///
    /// [`margin_pool`]: super::margin_pool
    pub reserved: &'a Money,
    /// The venue's index price of the settlement asset, needed when the
    /// entry's pool is the account's USD pool.
    pub usd_index: Option<&'a UsdIndex>,
    pub now_ms: TimestampMs,
    pub freshness: Freshness,
}

impl SizingRequest<'_> {
    /// Sizes the entry: a quantity with everything it was worked out
    /// from, or the refusal that stopped it, or a fault where an input
    /// cannot be trusted. Nothing is read or changed; the same request
    /// gives the same answer every time.
    pub fn size(&self) -> Result<Sizing, SizingFault> {
        super::steps::size(self)
    }
}
