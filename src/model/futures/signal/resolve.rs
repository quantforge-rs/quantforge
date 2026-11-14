//! Resolving a signal for the profile its URL selected: whose it is, how
//! old it is, what it names, and, for an entry, the policy it is admitted
//! under. The first rule that refuses it is the one it is recorded with.

use super::super::{
    EntryIntent, ExitIntent, FuturesInstrument, IntentKind, PolicySnapshot, PositionKey,
    ProfileIdentity, ProfilePolicy,
};
use super::{
    ExternalSignal, SIGNAL_FUTURE_ALLOWANCE_MS, SignalAction, SignalAgeLimit, SignalRefusal,
    TickerSpelling,
};
use crate::model::{Refusal, TimestampMs};

/// What a signal asks of its profile, once everything it names is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSignal {
    /// The entry or the exit, as a store records it.
    pub intent: IntentKind,
    /// The policy an entry is admitted under, taken from the profile's
    /// policy as it stood and never from the signal. An exit has none: no
    /// policy holds back a close.
    pub policy: Option<PolicySnapshot>,
    /// How the signal's ticker named the instrument.
    pub spelling: TickerSpelling,
}

/// A count of milliseconds as seconds, for a message: `75`, or `60.001`.
fn seconds(ms: i64) -> String {
    match (ms / 1_000, ms % 1_000) {
        (whole, 0) => whole.to_string(),
        (whole, rest) => format!("{whole}.{rest:03}"),
    }
}

impl ExternalSignal {
    /// Whether the signal is for the profile its URL selected: it names
    /// that profile, and the exchange and the family it asserts, if it
    /// asserts any, are that profile's. A signal that disagrees is refused
    /// as `ambiguous_selection`, whatever else it says: it never chooses
    /// another profile, venue or family.
    pub fn addressed_to(&self, identity: &ProfileIdentity) -> Result<(), SignalRefusal> {
        let refuse =
            |message: String| Err(SignalRefusal::new(Refusal::AmbiguousSelection, message));
        let profile = identity.id();
        if &self.profile != profile {
            return refuse(format!(
                "the alert names profile {}, and its URL selects profile {profile}; send an \
                 alert to the URL of the profile it names",
                self.profile
            ));
        }
        let venue = identity.account_ref().venue();
        if let Some(exchange) = &self.exchange {
            // TradingView names an exchange by its code in capitals.
            if *exchange != venue.as_str().to_ascii_uppercase() {
                return refuse(format!(
                    "the alert says its chart is of {exchange}, and profile {profile} trades on \
                     {venue}"
                ));
            }
        }
        if let Some(category) = self.category {
            if category != identity.category() {
                return refuse(format!(
                    "the alert says it is for {category} contracts, and profile {profile} trades \
                     {} contracts",
                    identity.category()
                ));
            }
        }
        Ok(())
    }

    /// Whether the signal may still be acted on at `now_ms`. An entry
    /// fired further ahead of the clock than
    /// [`SIGNAL_FUTURE_ALLOWANCE_MS`] is refused as `future_timestamp`,
    /// and one fired longer ago than `limit` as `stale`; an entry exactly
    /// at either bound is taken. An exit is never refused for its time: it
    /// never expires, and no clock holds back a close.
    pub fn timely(&self, now_ms: TimestampMs, limit: SignalAgeLimit) -> Result<(), SignalRefusal> {
        if matches!(self.action, SignalAction::Exit) {
            return Ok(());
        }
        let ahead = self.fire_time_ms.saturating_sub(now_ms);
        if ahead > SIGNAL_FUTURE_ALLOWANCE_MS {
            return Err(SignalRefusal::new(
                Refusal::FutureTimestamp,
                format!(
                    "the alert is timed {} s ahead of the clock, and at most {} s is allowed; \
                     check the server's clock",
                    seconds(ahead),
                    seconds(SIGNAL_FUTURE_ALLOWANCE_MS)
                ),
            ));
        }
        let age = now_ms.saturating_sub(self.fire_time_ms);
        if age > limit.ms() {
            return Err(SignalRefusal::new(
                Refusal::Stale,
                format!(
                    "the alert fired {} s ago, and profile {} takes entries up to {} s old",
                    seconds(age),
                    self.profile,
                    limit.secs()
                ),
            ));
        }
        Ok(())
    }

    /// What the signal asks of the profile whose `policy` this is, the one
    /// its URL selected, or the refusal it is recorded with. `listed` is
    /// what the profile's venue lists now, and `limit` the profile's age
    /// limit.
    ///
    /// The rules, in the order they are looked for:
    /// 1. the signal is for this profile ([`ExternalSignal::addressed_to`]);
    /// 2. an entry is neither from the future nor stale
    ///    ([`ExternalSignal::timely`]);
    /// 3. its ticker names one instrument
    ///    ([`ProfilePolicy::instrument_named`]);
    /// 4. for an entry, the policy takes the instrument
    ///    ([`ProfilePolicy::entry_snapshot`]);
    /// 5. and the entry has a stop, its own or the policy's fallback
    ///    ([`PolicySnapshot::stop_for`]).
    ///
    /// An exit stops at 3: it takes no policy, so a disabled asset, or one
    /// the profile never had settings for, can still be closed.
    ///
    /// Nothing here knows what the store holds. Whether the delivery is a
    /// repeat ([`Delivery`]), whether an entry meets a position already
    /// held ([`EntryIntent::against`]) and which position an exit closes
    /// ([`ExitIntent::closes`]) are asked of the store's records.
    ///
    /// [`Delivery`]: super::Delivery
    pub fn resolve(
        &self,
        policy: &ProfilePolicy,
        listed: &[FuturesInstrument],
        now_ms: TimestampMs,
        limit: SignalAgeLimit,
    ) -> Result<ResolvedSignal, SignalRefusal> {
        let identity = policy.identity();
        self.addressed_to(identity)?;
        self.timely(now_ms, limit)?;
        let named = policy.instrument_named(&self.ticker, listed)?;
        // The list is the profile's own line's, so the key always holds.
        let key = PositionKey::new(
            identity.account_ref().clone(),
            named.instrument.id().clone(),
        )
        .map_err(|error| SignalRefusal::new(Refusal::AmbiguousSelection, error.to_string()))?;
        let profile = identity.id().clone();
        let (intent, snapshot) = match self.action {
            SignalAction::Entry {
                expected_price,
                stop,
            } => {
                let snapshot = policy.entry_snapshot(named.instrument)?;
                snapshot.stop_for(stop)?;
                let entry = EntryIntent {
                    profile,
                    signal: self.id.clone(),
                    signal_time_ms: self.fire_time_ms,
                    key,
                    side: self.side,
                    expected_price,
                    stop,
                };
                (IntentKind::Entry(entry), Some(snapshot))
            }
            SignalAction::Exit => {
                let exit = ExitIntent {
                    profile,
                    signal: self.id.clone(),
                    signal_time_ms: self.fire_time_ms,
                    key,
                    side: self.side,
                };
                (IntentKind::Exit(exit), None)
            }
        };
        Ok(ResolvedSignal {
            intent,
            policy: snapshot,
            spelling: named.spelling,
        })
    }
}
