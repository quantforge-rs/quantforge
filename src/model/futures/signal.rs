//! An external signal before it is an intent (ADR 0002 §5, §9): what one
//! alert asked for, and the rules that decide what it means for the profile
//! its webhook URL selected.
//!
//! - **Whose it is.** The URL chooses the profile. The profile a signal
//!   names, and the exchange and the family it may assert, must agree with
//!   that profile; they never choose one.
//! - **How old it may be.** An entry is held to the profile's
//!   [`SignalAgeLimit`] and to [`SIGNAL_FUTURE_ALLOWANCE_MS`]. An exit is
//!   held to no clock.
//! - **What it names.** A ticker resolves through the profile's assets and
//!   the venue's own list ([`ProfilePolicy::instrument_named`]), never by
//!   guesswork.
//! - **What it becomes.** [`ExternalSignal::resolve`] gives the entry or
//!   the exit the store records, an entry with the policy it is admitted
//!   under, or the refusal it is recorded with.
//! - **Which delivery it is.** A signal is known by its profile and its
//!   ID; [`Delivery`] tells one sent again from a conflicting one by the
//!   hash of its payload.
//! - **What an exit closes, and what an entry meets.** An exit closes only
//!   a position its own profile entered before it fired
//!   ([`ExitIntent::closes`]); an entry on a position already held is
//!   refused ([`EntryIntent::against`]).
//!
//! Everything here is a pure function of what it is handed. The wire form
//! of a signal is the protocol's; the store that records one, and the
//! executor that acts on it, have stories of their own.
//!
//! [`ProfilePolicy::instrument_named`]: super::ProfilePolicy::instrument_named
//! [`ExitIntent::closes`]: super::ExitIntent::closes
//! [`EntryIntent::against`]: super::EntryIntent::against

mod cycle;
mod hash;
mod resolve;
mod ticker;

use super::{PolicyRefusal, ProfileId, SignalId};
use crate::model::{Category, ModelError, PositionSide, Price, Refusal, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;

pub use hash::SignalHash;
pub use resolve::ResolvedSignal;
pub use ticker::{NamedInstrument, PERPETUAL_SUFFIX, TickerSpelling};

/// How far ahead of the clock an entry's fire time may be: five seconds,
/// for clocks that disagree. It is fixed; no profile changes it.
pub const SIGNAL_FUTURE_ALLOWANCE_MS: i64 = 5_000;

/// How old an entry may be when it is admitted, and again just before it
/// is sent: 60 seconds unless the profile sets another limit, from 5 to
/// 300 seconds. TradingView resends a failed delivery at most three times,
/// five seconds apart, so the default takes every legitimate delivery and
/// refuses a late replay. A longer limit only admits alerts that were
/// delivered late; the price checks before submission still run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignalAgeLimit(u32);

impl SignalAgeLimit {
    /// The shortest limit a profile may set, in seconds.
    pub const MIN_SECS: u32 = 5;

    /// The longest, in seconds.
    pub const MAX_SECS: u32 = 300;

    /// The limit of a profile that sets none.
    pub const DEFAULT: Self = Self(60);

    /// Refuses a limit outside 5 to 300 seconds.
    pub fn new(secs: u32) -> Result<Self, ModelError> {
        if !(Self::MIN_SECS..=Self::MAX_SECS).contains(&secs) {
            return Err(ModelError::InvalidIdentifier {
                what: "signal age limit",
                value: format!("{secs} s"),
                rule: "from 5 to 300 seconds",
            });
        }
        Ok(Self(secs))
    }

    pub const fn secs(self) -> u32 {
        self.0
    }

    pub const fn ms(self) -> i64 {
        self.0 as i64 * 1_000
    }
}

impl Default for SignalAgeLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What a signal asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalAction {
    /// Open a position, sized at `expected_price` against the alert's own
    /// stop or, without one, the policy's fallback.
    Entry {
        expected_price: Price,
        stop: Option<Price>,
    },
    /// Close a position.
    Exit,
}

/// One alert, as it was delivered and found to be a signal. Nothing in it
/// is trusted further than that: its profile, its exchange and its family
/// are claims, and its ticker is text, until [`ExternalSignal::resolve`]
/// holds them to the profile its URL selected.
///
/// Its serde form is how a store keeps it, never the wire's: a delivery is
/// read by the payload format, and a signal recorded in an inbox is read
/// back from this.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSignal {
    /// The signal's ID within its profile, as the payload format builds
    /// it: the same for a delivery sent again.
    pub id: SignalId,
    /// The hash of the signal's canonical payload, which tells a delivery
    /// sent again from another signal under the same ID.
    pub payload_hash: SignalHash,
    /// The profile the alert names.
    pub profile: ProfileId,
    /// The instrument as the alert names it: TradingView's ticker.
    pub ticker: String,
    /// The exchange the alert says its chart is of, as TradingView names
    /// it, such as `BYBIT`.
    pub exchange: Option<String>,
    /// The product family the alert says it is meant for.
    pub category: Option<Category>,
    /// The side an entry opens, or an exit closes.
    pub side: PositionSide,
    pub action: SignalAction,
    /// The time of the bar the alert fired on.
    pub bar_time_ms: TimestampMs,
    /// When the alert fired: TradingView's `{{timenow}}`.
    pub fire_time_ms: TimestampMs,
}

/// Why a signal is refused: one of the parity matrix's refusals, with what
/// the operator would change. A refusal is an outcome to record, not an
/// error: the same signal is refused the same way however often it is
/// delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalRefusal {
    pub refusal: Refusal,
    pub message: String,
}

impl SignalRefusal {
    pub(super) fn new(refusal: Refusal, message: impl Into<String>) -> Self {
        Self {
            refusal,
            message: message.into(),
        }
    }
}

impl fmt::Display for SignalRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.refusal, self.message)
    }
}

impl std::error::Error for SignalRefusal {}

/// A policy's refusal of an entry is the signal's.
impl From<PolicyRefusal> for SignalRefusal {
    fn from(refused: PolicyRefusal) -> Self {
        Self {
            refusal: refused.refusal,
            message: refused.message,
        }
    }
}

/// What a delivery is to the signals its profile already has on record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Delivery {
    /// The profile has no signal with this ID: a new signal.
    New,
    /// The profile has this ID with this payload: the same delivery again.
    /// It gets the outcome of the first, and nothing is done twice.
    Duplicate,
    /// The profile has this ID with another payload: refused as
    /// [`Refusal::Conflict`], and the first stays as it was.
    Conflict,
}

impl Delivery {
    /// What a delivery whose payload hashes to `arrived` is, given the hash
    /// `recorded` for the same profile and the same signal ID, if any is.
    /// The profile is part of the key: the same ID under two profiles is
    /// two signals.
    pub fn of(recorded: Option<&SignalHash>, arrived: &SignalHash) -> Self {
        match recorded {
            None => Self::New,
            Some(recorded) if recorded == arrived => Self::Duplicate,
            Some(_) => Self::Conflict,
        }
    }

    /// `new`, `duplicate` or `conflict`, as a store records each delivery.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Duplicate => "duplicate",
            Self::Conflict => "conflict",
        }
    }

    /// The refusal a conflicting delivery is recorded with.
    pub const fn refusal(self) -> Option<Refusal> {
        match self {
            Self::Conflict => Some(Refusal::Conflict),
            Self::New | Self::Duplicate => None,
        }
    }
}

#[cfg(test)]
mod tests;
