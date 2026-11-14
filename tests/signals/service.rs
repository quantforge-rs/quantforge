//! Signals resolved through the signal service over a store, on each
//! backend: the profiles of the shared cases set up as an operator would,
//! and every signal resolved by a process that starts on the store, to the
//! answer the pure model gives. Then what the service adds to the model:
//! the URL's profile is read from the store, the venue is asked only when
//! it must be, and the same signal ID under two profiles stays two
//! signals.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use quantforge::automation::{PolicyError, PolicyService, SignalError, SignalService};
use quantforge::model::futures::{
    AssetEdit, FuturesInstrument, IntentKind, PolicyChange, PriceSnapshot, ProfileId,
    SignalAgeLimit,
};
use quantforge::model::{Environment, ExchangeId, InstrumentId, Refusal, Symbol, TimestampMs};
use quantforge::ports::futures::{FuturesCapabilities, FuturesConnector, InstrumentConnector};
use quantforge::ports::{Clock, ExchangeError};

use crate::cases::{Fixture, Line, fixture};
use crate::resolution::resolved_on_the_model;
use crate::support::{Backend, Ticking, Venue, on_each_backend};

/// A clock that stands at one time.
#[derive(Debug)]
struct At(TimestampMs);

impl Clock for At {
    fn now_ms(&self) -> TimestampMs {
        self.0
    }
}

/// The time the cases' alerts are delivered at: two seconds after they
/// fired.
const DELIVERED_MS: TimestampMs = 1_792_455_302_000;

/// The connector of a line's venue, listing what the line lists and, with
/// it, every other product's instruments.
fn venue_of(fixture: &Fixture, line: &Line) -> Venue {
    Venue {
        exchange: line.policy().exchange(),
        environment: line.identity.account_ref().environment(),
        instruments: fixture.listed_with_others(line),
    }
}

/// A venue that counts how often it is asked, and may be down.
#[derive(Debug)]
struct Counted {
    exchange: ExchangeId,
    instruments: Vec<FuturesInstrument>,
    down: bool,
    asked: AtomicUsize,
}

impl FuturesConnector for Counted {
    fn exchange_id(&self) -> ExchangeId {
        self.exchange
    }

    fn environment(&self) -> Environment {
        Environment::Demo
    }

    fn capabilities(&self) -> FuturesCapabilities {
        FuturesCapabilities::default()
    }
}

#[async_trait]
impl InstrumentConnector for Counted {
    async fn instruments(&self) -> Result<Vec<FuturesInstrument>, ExchangeError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.down {
            return Err(ExchangeError::InvalidRequest {
                message: "the venue is down".to_string(),
            });
        }
        Ok(self.instruments.clone())
    }

    async fn prices(&self, instrument: &InstrumentId) -> Result<PriceSnapshot, ExchangeError> {
        Err(ExchangeError::InvalidRequest {
            message: format!("no test asks the price of {instrument}"),
        })
    }
}

/// The operator's session: each line's profile, and its policy change by
/// change.
fn set_up<B: Backend>(backend: &B, fixture: &Fixture) {
    let clock = Ticking::default();
    let store = backend.open();
    let service = PolicyService::new(&store, &clock);
    for line in &fixture.lines {
        let identity = &line.identity;
        service
            .create_profile(
                identity.id().clone(),
                identity.account_ref().clone(),
                identity.category(),
            )
            .unwrap_or_else(|error| panic!("{}: {error}", line.name));
        for change in line.changes() {
            let made = service
                .change(identity.id(), None, &change)
                .unwrap_or_else(|error| panic!("{}: {error}", line.name));
            assert!(made.changed, "{}", line.name);
        }
    }
}

/// Every case, through the service over a store opened anew: the same
/// entry or exit, the same policy and the same refusal as the model gives,
/// on SQLite and on PostgreSQL.
async fn every_signal_resolves_the_same_from_a_policy_read_from_the_store<B: Backend>(backend: &B) {
    let fixture = fixture();
    set_up(backend, &fixture);

    let store = backend.open();
    let mut wrong = Vec::new();
    for case in &fixture.resolutions {
        let line = fixture.line(&case.line);
        let clock = At(case.now_ms);
        let service = SignalService::new(&store, &clock);
        let limit = SignalAgeLimit::new(case.age_limit_s).expect("an age limit");
        let got = service
            .resolve(
                line.profile(),
                &fixture.signal(&case.payload),
                &venue_of(&fixture, line),
                limit,
            )
            .await;
        let got = match got {
            Ok(resolved) => Ok(resolved),
            Err(SignalError::Refused(refusal)) => Err(refusal),
            Err(SignalError::Policy(error)) => panic!("{}: {error}", case.id),
        };
        let model = resolved_on_the_model(&fixture, case);
        if got != model {
            wrong.push(format!(
                "{}:\n  the service {got:?}\n  the model   {model:?}",
                case.id
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// What needs no venue is decided without one: a signal for another
/// profile, one whose assertions disagree and a stale entry are refused
/// while the venue is down, and never ask it. A signal that needs the
/// venue's list is not refused then: nothing is decided, and it can be
/// resolved again.
async fn a_venue_that_is_down_delays_a_signal_and_never_decides_it<B: Backend>(backend: &B) {
    let fixture = fixture();
    set_up(backend, &fixture);
    let store = backend.open();
    let (mut without, mut waiting) = (0, 0);
    for case in &fixture.resolutions {
        let line = fixture.line(&case.line);
        let clock = At(case.now_ms);
        let service = SignalService::new(&store, &clock);
        let venue = Counted {
            exchange: line.policy().exchange(),
            instruments: Vec::new(),
            down: true,
            asked: AtomicUsize::new(0),
        };
        let limit = SignalAgeLimit::new(case.age_limit_s).expect("an age limit");
        let got = service
            .resolve(
                line.profile(),
                &fixture.signal(&case.payload),
                &venue,
                limit,
            )
            .await;
        let asked = venue.asked.load(Ordering::SeqCst);
        let over = case.outcome.over.as_deref();
        if matches!(
            over,
            Some("profile" | "exchange" | "category" | "fire_time")
        ) {
            without += 1;
            let Err(SignalError::Refused(refusal)) = got else {
                panic!("{}: {got:?}", case.id);
            };
            assert_eq!(
                Some(refusal.refusal.as_str()),
                case.outcome.refused.as_deref()
            );
            assert_eq!(asked, 0, "{}: the venue was asked", case.id);
        } else {
            waiting += 1;
            assert!(
                matches!(got, Err(SignalError::Policy(PolicyError::Exchange(_)))),
                "{}: {got:?}",
                case.id
            );
            assert_eq!(asked, 1, "{}", case.id);
        }
    }
    assert!(
        without >= 12 && waiting >= 40,
        "{without} without, {waiting} waiting"
    );
}

/// The URL's profile is the store's: a URL for a profile the store does
/// not hold resolves nothing, and neither does a connector of another
/// product line or environment than the profile's. Neither is a refusal
/// of the signal.
async fn the_profile_and_its_venue_are_the_stores_never_the_signals<B: Backend>(backend: &B) {
    let fixture = fixture();
    set_up(backend, &fixture);
    let store = backend.open();
    let clock = At(DELIVERED_MS);
    let service = SignalService::new(&store, &clock);
    let line = fixture.line("bybit linear");
    let signal = fixture.signal("by-linear-long-entry");
    let limit = SignalAgeLimit::DEFAULT;

    let unknown = ProfileId::new("nobody").expect("a profile id");
    let got = service
        .resolve(&unknown, &signal, &venue_of(&fixture, line), limit)
        .await;
    assert!(
        matches!(&got, Err(SignalError::Policy(PolicyError::UnknownProfile(profile))) if profile == &unknown),
        "{got:?}"
    );

    // The connector of the other family, and one of another environment.
    let inverse = fixture.line("bybit inverse");
    let other_line = venue_of(&fixture, inverse);
    let mut other_environment = venue_of(&fixture, line);
    other_environment.environment = Environment::Mainnet;
    for venue in [other_line, other_environment] {
        let got = service
            .resolve(line.profile(), &signal, &venue, limit)
            .await;
        assert!(
            matches!(
                got,
                Err(SignalError::Policy(PolicyError::WrongConnector { .. }))
            ),
            "{got:?}"
        );
    }

    // With its own profile and its own venue, the signal resolves.
    let resolved = service
        .resolve(line.profile(), &signal, &venue_of(&fixture, line), limit)
        .await
        .expect("resolved");
    assert_eq!(resolved.intent.key().account(), line.identity.account_ref());
}

/// The same signal ID under two profiles is two signals. Each resolves
/// only at its own profile's URL, on that profile's own account and
/// venue; at the other's URL it is refused, and nothing crosses over.
async fn the_same_signal_id_under_two_profiles_never_crosses<B: Backend>(backend: &B) {
    let fixture = fixture();
    set_up(backend, &fixture);
    let store = backend.open();
    let clock = At(DELIVERED_MS);
    let service = SignalService::new(&store, &clock);
    let limit = SignalAgeLimit::DEFAULT;
    let lines = [fixture.line("bybit linear"), fixture.line("binance linear")];
    let signals = [
        fixture.signal("by-linear-long-entry"),
        fixture.signal("by-linear-long-entry-as-bn"),
    ];
    // One ID, two profiles, and so two payload hashes.
    assert_eq!(signals[0].id, signals[1].id);
    assert_ne!(signals[0].profile, signals[1].profile);
    assert_ne!(signals[0].payload_hash, signals[1].payload_hash);

    for (own, line) in lines.iter().enumerate() {
        let venue = venue_of(&fixture, line);
        let resolved = service
            .resolve(line.profile(), &signals[own], &venue, limit)
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", line.name));
        let IntentKind::Entry(entry) = &resolved.intent else {
            panic!("{}: no entry", line.name);
        };
        // Its own profile, account, venue and policy, whatever the other's.
        assert_eq!(&entry.profile, line.profile());
        assert_eq!(entry.signal, signals[own].id);
        assert_eq!(entry.key.account(), line.identity.account_ref());
        assert_eq!(entry.key.instrument().exchange, line.policy().exchange());
        let policy = resolved.policy.as_ref().expect("a policy");
        assert_eq!(policy.profile(), line.profile());
        assert_eq!(policy.account(), line.identity.account_ref());

        // The other profile's signal, delivered here, is refused for that.
        let other = &signals[1 - own];
        let crossed = service.resolve(line.profile(), other, &venue, limit).await;
        let Err(SignalError::Refused(refusal)) = crossed else {
            panic!("{}: {crossed:?}", line.name);
        };
        assert_eq!(refusal.refusal, Refusal::AmbiguousSelection);
        assert!(
            refusal.message.contains(other.profile.as_str()),
            "{refusal}"
        );
    }
}

/// A signal is resolved under the profile's newest policy, read when the
/// signal is resolved: an asset disabled since the last entry refuses the
/// next one, in a process that starts later too, and still lets its exit
/// through.
async fn the_newest_policy_answers_and_an_exit_needs_none<B: Backend>(backend: &B) {
    let fixture = fixture();
    set_up(backend, &fixture);
    let line = fixture.line("bybit linear");
    let venue = venue_of(&fixture, line);
    let limit = SignalAgeLimit::DEFAULT;
    let entry = fixture.signal("by-linear-long-entry");
    let exit = fixture.signal("by-linear-long-exit");
    let clock = At(DELIVERED_MS);

    let before = {
        let store = backend.open();
        let service = SignalService::new(&store, &clock);
        let resolved = service
            .resolve(line.profile(), &entry, &venue, limit)
            .await
            .expect("resolved");
        resolved.policy.expect("a policy").revision()
    };
    assert_eq!(before, line.policy().revision());

    {
        // The operator disables the asset.
        let store = backend.open();
        let ticking = Ticking::default();
        let policies = PolicyService::new(&store, &ticking);
        let disable = PolicyChange::EditAsset {
            symbol: Symbol::new("SOLUSDT").expect("a symbol"),
            edit: AssetEdit {
                enabled: Some(false),
                ..AssetEdit::default()
            },
        };
        let made = policies
            .change(line.profile(), Some(before), &disable)
            .expect("changed");
        assert!(made.changed);
    }

    let store = backend.open();
    let service = SignalService::new(&store, &clock);
    let refused = service.resolve(line.profile(), &entry, &venue, limit).await;
    let Err(SignalError::Refused(refusal)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(refusal.refusal, Refusal::NotArmed);
    let closing = service
        .resolve(line.profile(), &exit, &venue, limit)
        .await
        .expect("an exit resolves");
    assert!(matches!(closing.intent, IntentKind::Exit(_)));
    assert_eq!(closing.policy, None);
}

on_each_backend!(
    every_signal_resolves_the_same_from_a_policy_read_from_the_store,
    a_venue_that_is_down_delays_a_signal_and_never_decides_it,
    the_profile_and_its_venue_are_the_stores_never_the_signals,
    the_same_signal_id_under_two_profiles_never_crosses,
    the_newest_policy_answers_and_an_exit_needs_none,
);
