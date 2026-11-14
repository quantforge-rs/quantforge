//! A signal: what one alert asks for, once its body has passed every rule.

use sha2::{Digest, Sha256};

use super::super::{ContentHash, ExactDecimal};
use super::problems::SignalProblems;
use super::times::SignalTime;
use super::words::{SignalCategory, SignalKind, SignalSide};
use super::{SIGNAL_VERSION, read};

/// What only one kind of signal carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SignalOrder {
    Entry {
        price: ExactDecimal,
        stop: Option<ExactDecimal>,
    },
    Exit,
}

/// One alert's message, read and found to be a version 1 signal: an entry
/// or an exit on one ticker, for one profile. The only way to make one is
/// [`Signal::read`], so a `Signal` has passed every rule of the format.
///
/// What a signal names is not yet what it trades. The profile is the one
/// its URL selected or the signal is refused, and the ticker, the exchange
/// and the family are held to that profile's venue and policy by whoever
/// receives it.
///
/// ```
/// use quantforge::protocol::signal::{Signal, SignalKind, SignalSide};
///
/// let signal = Signal::read(
///     br#"{
///   "version": 1,
///   "kind": "entry",
///   "profile": "by-linear",
///   "source": "trend",
///   "ticker": "SOLUSDT.P",
///   "side": "long",
///   "bar_time": "2026-10-20T00:00:00Z",
///   "fire_time": "2026-10-20T00:15:00Z",
///   "price": "234.23",
///   "stop": "200.00"
/// }"#,
/// )?;
/// assert_eq!((signal.kind(), signal.side()), (SignalKind::Entry, SignalSide::Long));
/// assert_eq!(signal.profile(), "by-linear");
/// assert_eq!(signal.ticker(), "SOLUSDT.P");
/// // A price keeps the digits it was sent with.
/// assert_eq!(signal.stop().map(|stop| stop.to_string()).as_deref(), Some("200.00"));
/// assert_eq!(signal.signal_id(), "trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z");
/// # Ok::<(), quantforge::protocol::signal::SignalProblems>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub(super) profile: String,
    pub(super) source: String,
    pub(super) ticker: String,
    pub(super) exchange: Option<String>,
    pub(super) category: Option<SignalCategory>,
    pub(super) side: SignalSide,
    pub(super) bar_time: SignalTime,
    pub(super) fire_time: SignalTime,
    pub(super) order: SignalOrder,
}

impl Signal {
    /// Reads an alert's message, exactly as it was delivered. A body that
    /// is not a signal is refused with everything that is wrong with it,
    /// never with the first problem alone, and never corrected: no case is
    /// folded, no space trimmed, no missing value filled in.
    ///
    /// ```
    /// use quantforge::protocol::signal::{Signal, SignalProblemCode};
    ///
    /// // A price without its quotes, and a member the format does not have.
    /// let body = br#"{"version":1,"kind":"entry","profile":"main","source":"trend",
    ///     "ticker":"SOLUSDT.P","side":"long","bar_time":"2026-10-20T00:00:00Z",
    ///     "fire_time":"2026-10-20T00:15:00Z","price":234.23,"leverage":"50"}"#;
    /// let refused = Signal::read(body).expect_err("not a signal");
    /// let found: Vec<_> = refused
    ///     .problems()
    ///     .iter()
    ///     .map(|problem| (problem.code(), problem.field_name()))
    ///     .collect();
    /// assert_eq!(
    ///     found,
    ///     [
    ///         (SignalProblemCode::WrongType, Some("price")),
    ///         (SignalProblemCode::UnknownField, Some("leverage")),
    ///     ]
    /// );
    /// assert_eq!(
    ///     refused.to_string(),
    ///     "the body is not a version 1 signal: \"price\" is a number, and must be a string; \
    ///      \"leverage\" is not a field of a version 1 signal"
    /// );
    /// ```
    pub fn read(body: &[u8]) -> Result<Self, SignalProblems> {
        read::signal(body)
    }

    /// Whether the signal enters or exits.
    pub fn kind(&self) -> SignalKind {
        match self.order {
            SignalOrder::Entry { .. } => SignalKind::Entry,
            SignalOrder::Exit => SignalKind::Exit,
        }
    }

    /// The profile the alert says it is for.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The alert's constant name, which starts the signal's ID.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The ticker, as TradingView rendered it.
    pub fn ticker(&self) -> &str {
        &self.ticker
    }

    /// The exchange the alert says its chart is of, as TradingView names
    /// it, if it says.
    pub fn exchange(&self) -> Option<&str> {
        self.exchange.as_deref()
    }

    /// The product family the alert says it is meant for, if it says.
    pub fn category(&self) -> Option<SignalCategory> {
        self.category
    }

    /// The side an entry opens, or an exit closes.
    pub fn side(&self) -> SignalSide {
        self.side
    }

    /// The time of the bar the alert fired on: TradingView's `{{time}}`.
    pub fn bar_time(&self) -> &SignalTime {
        &self.bar_time
    }

    /// When the alert fired: TradingView's `{{timenow}}`.
    pub fn fire_time(&self) -> &SignalTime {
        &self.fire_time
    }

    /// An entry's expected price, in the instrument's quote asset; `None`
    /// for an exit.
    pub fn price(&self) -> Option<ExactDecimal> {
        match self.order {
            SignalOrder::Entry { price, .. } => Some(price),
            SignalOrder::Exit => None,
        }
    }

    /// An entry's own stop price, if the alert gave one.
    pub fn stop(&self) -> Option<ExactDecimal> {
        match self.order {
            SignalOrder::Entry { stop, .. } => stop,
            SignalOrder::Exit => None,
        }
    }

    /// The signal's ID within its profile: its source, its kind, its side,
    /// its ticker and its bar time, joined by colons. TradingView renders
    /// each the same way every time it sends the alert, so a delivery sent
    /// again has the ID it had before. The fire time is left out for that
    /// reason: nothing says a resend renders it as the first send did.
    ///
    /// An ID is at most 106 characters, and holds no space and no control
    /// character.
    pub fn signal_id(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.source,
            self.kind(),
            self.side,
            self.ticker,
            self.bar_time
        )
    }

    /// The canonical payload: one JSON object on one line, holding every
    /// field the signal has but its fire time, in the format's order, with
    /// no space between them. Whatever the spacing, the order of members
    /// and the escapes of the body that was sent, the same signal has the
    /// same canonical payload.
    ///
    /// ```
    /// use quantforge::protocol::signal::Signal;
    ///
    /// let sent = br#"{"side":"long","ticker":"SOLUSDT.P","source":"trend","profile":"by-linear",
    ///     "fire_time":"2026-10-20T00:15:00Z","bar_time":"2026-10-20T00:00:00Z",
    ///     "kind":"exit","version":1}"#;
    /// assert_eq!(
    ///     Signal::read(sent)?.canonical(),
    ///     r#"{"version":1,"kind":"exit","profile":"by-linear","source":"trend","ticker":"SOLUSDT.P","side":"long","bar_time":"2026-10-20T00:00:00Z"}"#
    /// );
    /// # Ok::<(), quantforge::protocol::signal::SignalProblems>(())
    /// ```
    pub fn canonical(&self) -> String {
        let mut text = format!("{{\"version\":{SIGNAL_VERSION}");
        let mut member = |name: &str, value: &str| {
            text.push_str(",\"");
            text.push_str(name);
            text.push_str("\":");
            // No value of a signal needs an escape; this writes one that
            // did as JSON would.
            text.push_str(&serde_json::Value::from(value).to_string());
        };
        member("kind", self.kind().as_str());
        member("profile", &self.profile);
        member("source", &self.source);
        member("ticker", &self.ticker);
        if let Some(exchange) = &self.exchange {
            member("exchange", exchange);
        }
        if let Some(category) = self.category {
            member("category", category.as_str());
        }
        member("side", self.side.as_str());
        member("bar_time", self.bar_time.as_str());
        if let SignalOrder::Entry { price, stop } = self.order {
            member("price", &price.to_string());
            if let Some(stop) = stop {
                member("stop", &stop.to_string());
            }
        }
        text.push('}');
        text
    }

    /// The hash of the canonical payload's bytes. Two deliveries with one
    /// ID are the same signal when their hashes are equal, and a conflict
    /// when they are not: the same alert on the same bar with another
    /// price, another stop or another profile.
    ///
    /// ```
    /// use quantforge::protocol::signal::Signal;
    ///
    /// let first = br#"{"version":1,"kind":"exit","profile":"by-linear","source":"trend",
    ///     "ticker":"SOLUSDT.P","side":"long","bar_time":"2026-10-20T00:00:00Z",
    ///     "fire_time":"2026-10-20T00:15:00Z"}"#;
    /// // Sent again ten seconds later, should a resend render the time again.
    /// let again = br#"{"version":1,"kind":"exit","profile":"by-linear","source":"trend",
    ///     "ticker":"SOLUSDT.P","side":"long","bar_time":"2026-10-20T00:00:00Z",
    ///     "fire_time":"2026-10-20T00:15:10Z"}"#;
    /// let (first, again) = (Signal::read(first)?, Signal::read(again)?);
    /// assert_ne!(first, again);
    /// assert_eq!(first.signal_id(), again.signal_id());
    /// assert_eq!(first.payload_hash(), again.payload_hash());
    /// # Ok::<(), quantforge::protocol::signal::SignalProblems>(())
    /// ```
    pub fn payload_hash(&self) -> ContentHash {
        ContentHash::sha256(Sha256::digest(self.canonical().as_bytes()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = r#"{"version":1,"kind":"entry","profile":"by-linear","source":"trend","ticker":"SOLUSDT.P","exchange":"BYBIT","category":"linear","side":"short","bar_time":"2026-10-20T00:00:00Z","fire_time":"2026-10-20T00:15:00Z","price":"234.230","stop":"240.00"}"#;

    fn read(body: &str) -> Signal {
        Signal::read(body.as_bytes()).expect("a signal")
    }

    #[test]
    fn a_signal_holds_what_its_body_said() {
        let signal = read(ENTRY);
        assert_eq!(signal.kind(), SignalKind::Entry);
        assert_eq!(signal.profile(), "by-linear");
        assert_eq!(signal.source(), "trend");
        assert_eq!(signal.ticker(), "SOLUSDT.P");
        assert_eq!(signal.exchange(), Some("BYBIT"));
        assert_eq!(signal.category(), Some(SignalCategory::Linear));
        assert_eq!(signal.side(), SignalSide::Short);
        assert_eq!(signal.bar_time().ms(), 1_792_454_400_000);
        assert_eq!(signal.fire_time().ms(), 1_792_455_300_000);
        let text = |price: Option<ExactDecimal>| price.map(|price| price.to_string());
        assert_eq!(text(signal.price()).as_deref(), Some("234.230"));
        assert_eq!(text(signal.stop()).as_deref(), Some("240.00"));
    }

    #[test]
    fn the_canonical_payload_is_the_body_without_its_fire_time() {
        let signal = read(ENTRY);
        let canonical = signal.canonical();
        assert_eq!(
            canonical,
            ENTRY.replace(r#""fire_time":"2026-10-20T00:15:00Z","#, "")
        );
        // It is a signal's own text but for the one field it leaves out.
        assert!(Signal::read(canonical.as_bytes()).is_err());
        assert_eq!(
            signal.signal_id(),
            "trend:entry:short:SOLUSDT.P:2026-10-20T00:00:00Z"
        );
    }

    #[test]
    fn the_hash_follows_every_field_but_the_fire_time() {
        let hash = |body: &str| read(body).payload_hash();
        let base = hash(ENTRY);
        assert_eq!(base, hash(&ENTRY.replace("00:15:00Z", "00:15:59Z")));
        for (from, to) in [
            ("\"by-linear\"", "\"by-inverse\""),
            ("\"trend\"", "\"breakout\""),
            ("SOLUSDT.P", "SOLUSDT"),
            ("\"exchange\":\"BYBIT\",", ""),
            ("\"category\":\"linear\",", ""),
            ("\"short\"", "\"long\""),
            ("2026-10-20T00:00:00Z", "2026-10-19T23:45:00Z"),
            ("234.230", "234.23"),
            ("\"240.00\"", "\"240.0\""),
            (",\"stop\":\"240.00\"", ""),
        ] {
            let changed = ENTRY.replace(from, to);
            assert_ne!(changed, ENTRY, "{from}");
            assert_ne!(hash(&changed), base, "{from} -> {to}");
        }
        let exit = ENTRY
            .replace("\"entry\"", "\"exit\"")
            .replace(",\"price\":\"234.230\",\"stop\":\"240.00\"", "");
        assert_ne!(hash(&exit), base);
        assert_eq!(read(&exit).price(), None);
        assert_eq!(read(&exit).stop(), None);
    }

    #[test]
    fn the_hash_is_the_sha_256_of_the_canonical_bytes() {
        let signal = read(ENTRY);
        let digest: [u8; 32] = Sha256::digest(signal.canonical().as_bytes()).into();
        assert_eq!(signal.payload_hash().digest(), &digest);
        assert!(signal.payload_hash().to_string().starts_with("sha256:"));
    }
}
