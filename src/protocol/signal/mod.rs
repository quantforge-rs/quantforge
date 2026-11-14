//! Signals: the message of a TradingView alert that QuantForge executes,
//! and the receipt its delivery is answered with. The [signal contract]
//! says what a server does with one; these pages say what one is. A
//! [JSON schema] gives its shape to other tools.
//!
//! [signal contract]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/signal-payload.md>
//! [JSON schema]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/signal-schema.json>
//!
//! # The payload
//!
//! TradingView delivers an alert by `POST`ing its message, as the whole
//! body, to the alert's webhook URL. A [`Signal`] is that message: one JSON
//! object, in UTF-8, of at most [`MAX_SIGNAL_BYTES`]. The alert's template
//! writes it, with TradingView's placeholders where a value changes from
//! one alert to the next.
//!
//! | field | value | in a template |
//! |---|---|---|
//! | `version` | the number [`SIGNAL_VERSION`] | `1` |
//! | `kind` | `entry` or `exit` | a constant |
//! | `profile` | the automation profile the alert is for: 1 to 32 of `a-z`, `0-9`, `-` and `_`, starting with a letter or a digit | a constant |
//! | `source` | the alert's own name, under the same rule | a constant |
//! | `ticker` | the instrument, as TradingView names it: 1 to [`MAX_TICKER_CHARS`] characters, uppercase, no spaces | `"{{ticker}}"` |
//! | `exchange` | optional: the chart's exchange, as TradingView names it: 1 to 32 of `A-Z`, `0-9` and `_` | `"{{exchange}}"` |
//! | `category` | optional: `linear` or `inverse` | a constant |
//! | `side` | `long` or `short`: the side an entry opens, or an exit closes | a constant |
//! | `bar_time` | a [`SignalTime`]: the bar the alert fired on | `"{{time}}"` |
//! | `fire_time` | a [`SignalTime`]: when the alert fired | `"{{timenow}}"` |
//! | `price` | an entry's expected price: a decimal string above zero | `"{{close}}"` |
//! | `stop` | optional: an entry's own stop price, likewise | `"{{plot_0}}"` |
//!
//! An exit carries neither `price` nor `stop`. Every other field but the
//! three marked optional is required.
//!
//! A payload names things; it decides nothing. There is no field for a
//! quantity, a risk, a leverage, an account, an environment or a
//! credential, and none that turns trading on: those are the operator's,
//! kept in the profile the webhook URL selects. `profile`, `exchange` and
//! `category` only assert, and a signal whose assertion disagrees with the
//! profile its URL selected is refused.
//!
//! # Strict by design
//!
//! Unlike the rest of the protocol, which ignores what it does not know, a
//! signal is exactly its format, as a bundle's manifest is. A body is
//! refused, with every [`SignalProblem`] it has, when it:
//!
//! - is not one JSON value, nests deeper than [`MAX_SIGNAL_DEPTH`], or
//!   holds a number no double holds;
//! - is not an object, or writes a member twice;
//! - has a member that is not in [`SIGNAL_FIELDS`], or an exit with an
//!   entry's fields;
//! - lacks a required field, or gives `null` for an optional one;
//! - has a value of another JSON type, such as a price without its quotes,
//!   which a float may already have rounded;
//! - has a value that is not exactly its field's form. Nothing is trimmed,
//!   no case is folded, and a placeholder TradingView left unfilled, such
//!   as a stop of `NaN`, refuses the alert instead of being dropped.
//!
//! The format has its own version, in `version`, apart from the protocol's:
//! a payload is written once into an alert and sent for months. A change
//! to the format is a new version.
//!
//! # Identity
//!
//! A delivery is known by the profile it is for and its
//! [`Signal::signal_id`]: the alert's source, kind and side, the ticker and
//! the bar time. Those are what TradingView renders the same way each time
//! it sends an alert, so a delivery sent again is recognized.
//!
//! Its [`Signal::payload_hash`] covers the [`Signal::canonical`] payload:
//! every field but `fire_time`. Under one ID, an equal hash is the same
//! signal again, and another hash is a conflict.
//!
//! # The receipt
//!
//! A delivery that was recorded is answered `2xx` with a
//! [`SignalReceipt`], which names the signal and one of five states. It
//! means that the delivery was recorded and will not be lost. It never
//! means that an order exists.

mod fields;
mod json;
mod payload;
mod problems;
mod read;
mod receipt;
mod times;
mod words;

pub use fields::SIGNAL_FIELDS;
pub use payload::Signal;
pub use problems::{SignalProblem, SignalProblemCode, SignalProblems};
pub use receipt::{ReceiptState, SignalReceipt};
pub use times::SignalTime;
pub use words::{SignalCategory, SignalKind, SignalSide};

/// The version of the payload format, and of the receipt.
pub const SIGNAL_VERSION: u32 = 1;

/// The most bytes a body may be. An alert's message is a few hundred.
pub const MAX_SIGNAL_BYTES: usize = 4096;

/// How deep a body may nest lists and objects, the signal's own object
/// included. A signal nests nothing; the limit bounds what is read of a
/// body that is not one.
pub const MAX_SIGNAL_DEPTH: usize = 8;

/// The most characters a ticker may be.
pub const MAX_TICKER_CHARS: usize = 40;
