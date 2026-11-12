//! What sizing cannot trust: a fault, kept apart from the refusals. A
//! refusal is an answer about the entry, in the parity matrix's words; a
//! fault says the answer could not be worked out from what was given,
//! and names the input to read again.

use crate::model::TimestampMs;
use std::fmt;

/// One of the things sizing is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SizingInput {
    /// The instrument's rules: tick, steps, limits and face value.
    Rules,
    /// The account snapshot.
    Account,
    /// The account's leverage brackets for the instrument.
    Brackets,
    /// The account's fee rates for the instrument.
    Fees,
    /// The margin pool the entry would draw on.
    Pool,
    /// The margin already reserved against that pool.
    Reservation,
    /// The venue's index price of the settlement asset, in USD.
    UsdIndex,
    /// The settlement asset's wallet.
    Wallet,
    /// The mark price and the book, read just before submission.
    Prices,
    /// The entry's fills.
    Fills,
}

impl SizingInput {
    /// Every input, in the order sizing looks at them.
    pub const ALL: [Self; 10] = [
        Self::Rules,
        Self::Account,
        Self::Brackets,
        Self::Fees,
        Self::Pool,
        Self::Reservation,
        Self::UsdIndex,
        Self::Wallet,
        Self::Prices,
        Self::Fills,
    ];

    /// The input's name as records spell it, such as `usd_index`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Account => "account",
            Self::Brackets => "brackets",
            Self::Fees => "fees",
            Self::Pool => "pool",
            Self::Reservation => "reservation",
            Self::UsdIndex => "usd_index",
            Self::Wallet => "wallet",
            Self::Prices => "prices",
            Self::Fills => "fills",
        }
    }

    /// The input as a message names it.
    const fn phrase(self) -> &'static str {
        match self {
            Self::Rules => "instrument rules",
            Self::Account => "account snapshot",
            Self::Brackets => "leverage brackets",
            Self::Fees => "fee rates",
            Self::Pool => "margin pool",
            Self::Reservation => "margin reservation",
            Self::UsdIndex => "USD index price",
            Self::Wallet => "settlement wallet",
            Self::Prices => "prices",
            Self::Fills => "fills",
        }
    }
}

impl fmt::Display for SizingInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What is wrong with an input, or with a figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SizingFaultKind {
    /// The input is for something else: another instrument, account or
    /// asset than the entry's.
    Mismatched,
    /// The input is older than its limit, or timed further ahead of the
    /// clock than that.
    Stale,
    /// The input, or a part sizing needs of it, is not there.
    Missing,
    /// The input gives two answers where sizing needs one.
    Ambiguous,
    /// The input says something that cannot be so.
    Invalid,
    /// A figure came out larger than an exact decimal holds.
    OutOfRange,
}

impl SizingFaultKind {
    /// Every kind.
    pub const ALL: [Self; 6] = [
        Self::Mismatched,
        Self::Stale,
        Self::Missing,
        Self::Ambiguous,
        Self::Invalid,
        Self::OutOfRange,
    ];

    /// The kind's name as records spell it, such as `stale`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mismatched => "mismatched",
            Self::Stale => "stale",
            Self::Missing => "missing",
            Self::Ambiguous => "ambiguous",
            Self::Invalid => "invalid",
            Self::OutOfRange => "out_of_range",
        }
    }
}

impl fmt::Display for SizingFaultKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why sizing gave no answer: an input it cannot trust, or a figure no
/// exact decimal holds. It is not a refusal, and nothing may be sent on
/// the strength of one: what a fault halts, and how it is recorded, is
/// its caller's to decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizingFault {
    kind: SizingFaultKind,
    input: Option<SizingInput>,
    detail: String,
}

impl SizingFault {
    pub const fn kind(&self) -> SizingFaultKind {
        self.kind
    }

    /// The input at fault. `None` for a figure out of range, which no one
    /// input explains.
    pub const fn input(&self) -> Option<SizingInput> {
        self.input
    }

    /// What is wrong, in a sentence's worth.
    pub fn detail(&self) -> &str {
        &self.detail
    }

    fn about(kind: SizingFaultKind, input: SizingInput, detail: impl Into<String>) -> Self {
        Self {
            kind,
            input: Some(input),
            detail: detail.into(),
        }
    }

    pub(super) fn mismatched(input: SizingInput, detail: impl Into<String>) -> Self {
        Self::about(SizingFaultKind::Mismatched, input, detail)
    }

    pub(super) fn missing(input: SizingInput, detail: impl Into<String>) -> Self {
        Self::about(SizingFaultKind::Missing, input, detail)
    }

    pub(super) fn ambiguous(input: SizingInput, detail: impl Into<String>) -> Self {
        Self::about(SizingFaultKind::Ambiguous, input, detail)
    }

    pub(super) fn invalid(input: SizingInput, detail: impl Into<String>) -> Self {
        Self::about(SizingFaultKind::Invalid, input, detail)
    }

    /// `figure` came out beyond what an exact decimal holds.
    pub(super) fn out_of_range(figure: &'static str) -> Self {
        Self {
            kind: SizingFaultKind::OutOfRange,
            input: None,
            detail: format!("the {figure} is beyond what an exact decimal holds"),
        }
    }

    /// `Err` unless `observed_at_ms` is within `limit_ms` of `now_ms`, on
    /// either side: an input timed further ahead of the clock than its
    /// limit says the clocks disagree, and is trusted no more than an old
    /// one. `part` names which part of the input the time is of, where it
    /// has more than one.
    pub(super) fn unless_fresh(
        input: SizingInput,
        part: &str,
        observed_at_ms: TimestampMs,
        now_ms: TimestampMs,
        limit_ms: i64,
    ) -> Result<(), Self> {
        let apart = now_ms.abs_diff(observed_at_ms);
        if u64::try_from(limit_ms).is_ok_and(|limit| apart <= limit) {
            return Ok(());
        }
        let detail = if observed_at_ms > now_ms {
            format!(
                "{part}timed {apart} ms ahead of the clock, and nothing further than {limit_ms} \
                 ms from it is taken; check the clock"
            )
        } else {
            format!("{part}read {apart} ms ago, and nothing older than {limit_ms} ms is taken")
        };
        Err(Self::about(SizingFaultKind::Stale, input, detail))
    }
}

impl fmt::Display for SizingFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.input {
            Some(input) => write!(
                f,
                "sizing cannot trust the {}: {}",
                input.phrase(),
                self.detail
            ),
            None => write!(f, "sizing cannot state a figure: {}", self.detail),
        }
    }
}

impl std::error::Error for SizingFault {}
