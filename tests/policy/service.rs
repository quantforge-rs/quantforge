//! Every shared case through the policy service, on each backend: each
//! change is one stored revision, a policy read back from the store
//! answers exactly as the model's, a store opened anew holds the same
//! policy, and every revision ever made is still there, word for word.
//! Then what the service adds of its own: profiles, the venue's say on a
//! change, and two venues' profiles kept apart.

use quantforge::automation::{AssetSettings, PolicyError, PolicyService};
use quantforge::model::futures::{
    DefaultsEdit, Edit, OwnerTag, PolicyChange, PolicyProblemKind, PolicyRevision, ProfileId,
    ProfileIdentity, ProfilePolicy, StopFallback,
};
use quantforge::model::{AccountRef, Category, Environment, Leverage, Symbol, Venue as VenueName};
use quantforge::ports::Clock;
use quantforge::ports::futures::InstrumentConnector;
use quantforge::{PolicyStore, StoreControl};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::cases::{Case, Fixture, Line, change, effective_json, fixture, risk};
use crate::model::{enter, problems_json};
use crate::support::{Backend, T, Ticking, Venue, on_each_backend, stored};

fn profile() -> ProfileId {
    ProfileId::new("main").expect("a profile id")
}

fn revision(number: u64) -> PolicyRevision {
    PolicyRevision::new(number).expect("a revision")
}

/// Replays `case` through the service over `backend`, and answers every
/// document it made, by revision.
async fn replay<B: Backend>(backend: &B, line: &Line, case: &Case) -> Vec<String> {
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    let venue = Venue::of(line);
    store
        .create_profile(&ProfilePolicy::new(line.identity.clone()), clock.now_ms())
        .unwrap_or_else(|error| panic!("{}: {error}", case.id));
    let mut documents = vec![case.created.clone()];
    for (n, step) in case.steps.iter().enumerate() {
        let at = format!("{}, step {n}", case.id);
        let held = service.policy(&profile()).expect("the policy");
        assert_eq!(
            &stored(&held),
            documents.last().expect("a document"),
            "{at}"
        );
        if let Some(asked) = &step.change {
            let made = service.change(&profile(), Some(held.revision()), &change(line, asked));
            match (made, &step.document) {
                (Ok(outcome), Some(document)) => {
                    assert!(outcome.changed, "{at}");
                    assert_eq!(&stored(&outcome.policy), document, "{at}");
                    documents.push(document.clone());
                }
                (Ok(outcome), None) => {
                    assert_eq!(step.unchanged, Some(true), "{at}");
                    assert!(!outcome.changed, "{at}");
                    assert_eq!(outcome.policy, held, "{at}");
                }
                (Err(PolicyError::Model(error)), None) => {
                    let refused = step.refused.as_ref().and_then(Value::as_str);
                    assert_eq!(Some(error.to_string().as_str()), refused, "{at}");
                }
                (other, _) => panic!("{at}: {other:?}"),
            }
        } else if let Some(symbol) = &step.effective {
            let effective = held.effective(line.listed(symbol).id()).expect("effective");
            assert_eq!(Some(effective_json(&effective)), step.expect, "{at}");
        } else if let Some(symbol) = &step.entry {
            // The model's answer on the policy the store handed back, and
            // the service's own, which reads the newest revision itself.
            enter(&held, line, step, &at);
            let own = service.entry(&profile(), line.listed(symbol));
            assert_eq!(own.is_ok(), step.snapshot.is_some(), "{at}");
        } else {
            let check = service.check(&profile(), &venue).await.expect("a check");
            assert_eq!(check.revision, held.revision(), "{at}");
            assert_eq!(
                Some(&problems_json(&check.problems)),
                step.problems.as_ref(),
                "{at}"
            );
            assert_eq!(check.is_ready(), check.problems.is_empty(), "{at}");
        }
    }
    documents
}

/// Every case of `line` through the service, then a store opened anew
/// held to every document of every revision.
async fn replay_line<B: Backend>(backend: impl Fn() -> Option<B>, fixture: &Fixture, line: &str) {
    let line = fixture.line(line);
    for case in fixture.cases.iter().filter(|case| case.line == line.name) {
        let Some(backend) = backend() else { return };
        let documents = replay(&backend, line, case).await;

        // A process that starts on the store finds the newest revision, and
        // each one before it, exactly as it was made.
        let store = backend.open();
        let newest = store.profile_policy(&profile()).expect("a read");
        assert_eq!(
            newest.as_ref().map(stored).as_ref(),
            documents.last(),
            "{}",
            case.id
        );
        for (number, document) in (1..).zip(&documents) {
            let at = store
                .profile_policy_at(&profile(), revision(number))
                .expect("a read")
                .unwrap_or_else(|| panic!("{}: revision {number} is gone", case.id));
            assert_eq!(&stored(&at), document, "{}, revision {number}", case.id);
        }
        let past = revision(u64::try_from(documents.len()).expect("a count") + 1);
        assert!(
            store
                .profile_policy_at(&profile(), past)
                .expect("a read")
                .is_none(),
            "{}: a revision no change made",
            case.id
        );
        let stamps = store.policy_revisions(&profile()).expect("stamps");
        let numbers: Vec<u64> = stamps.iter().map(|stamp| stamp.revision.get()).collect();
        let expected: Vec<u64> = (1..).take(documents.len()).collect();
        assert_eq!(numbers, expected, "{}", case.id);
        assert!(
            stamps
                .windows(2)
                .all(|pair| pair[0].changed_at_ms < pair[1].changed_at_ms),
            "{}: each revision later than the one before",
            case.id
        );
    }
}

macro_rules! every_case_on {
    ($($test:ident => $line:literal),* $(,)?) => {$(
        mod $test {
            use super::*;

            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn sqlite() {
                replay_line(|| Some(crate::support::Sqlite::new()), &fixture(), $line).await;
            }

            #[cfg(feature = "postgres")]
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn postgres() {
                replay_line(crate::support::postgres::Postgres::new, &fixture(), $line).await;
            }
        }
    )*};
}

every_case_on!(
    every_case_through_the_service_on_binance_linear => "binance linear",
    every_case_through_the_service_on_binance_inverse => "binance inverse",
    every_case_through_the_service_on_bybit_linear => "bybit linear",
    every_case_through_the_service_on_bybit_inverse => "bybit inverse",
);

fn account(venue: VenueName, environment: Environment, id: &str) -> AccountRef {
    let id = quantforge::model::VenueAccountId::issued(id).expect("an account id");
    AccountRef::new(venue, environment, id).expect("an account")
}

/// A profile is created once, disarmed and empty, with a tag of its own,
/// for the store's own environment only.
async fn a_profile_is_created_once_empty_with_a_tag_of_its_own<B: Backend>(backend: &B) {
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    let id = |name: &str| ProfileId::new(name).expect("a profile id");
    let demo = |venue| account(venue, Environment::Demo, "777");

    let first = service
        .create_profile(id("bybit-main"), demo(VenueName::Bybit), Category::Linear)
        .expect("a profile");
    assert_eq!(first.revision(), PolicyRevision::FIRST);
    assert!(first.assets().is_empty() && first.capital().is_empty());
    assert!(
        !first.defaults().enabled(),
        "nothing is enabled by creation"
    );
    let second = service
        .create_profile(id("alpha"), demo(VenueName::Binance), Category::Inverse)
        .expect("another profile");
    assert_ne!(
        first.identity().owner_tag(),
        second.identity().owner_tag(),
        "each profile has a tag of its own"
    );
    assert!(OwnerTag::new(first.identity().owner_tag().as_str()).is_ok());

    // Listed in the byte order of their IDs, as created.
    let listed: Vec<ProfileIdentity> = service.profiles().expect("profiles");
    assert_eq!(
        listed,
        [second.identity().clone(), first.identity().clone()]
    );
    assert_eq!(service.policy(&id("alpha")).expect("the policy"), second);

    let again = service
        .create_profile(id("alpha"), demo(VenueName::Bybit), Category::Linear)
        .expect_err("created once");
    assert_eq!(
        again.to_string(),
        "invalid argument: profile alpha is already recorded; a profile is created once"
    );
    let elsewhere = service
        .create_profile(
            id("live"),
            account(VenueName::Bybit, Environment::Mainnet, "777"),
            Category::Linear,
        )
        .expect_err("another environment");
    assert_eq!(
        elsewhere.to_string(),
        "invalid argument: profile live is for mainnet, and the store is bound to demo; a store \
         holds profiles of its own environment only"
    );
    assert!(matches!(
        service.policy(&id("live")),
        Err(PolicyError::UnknownProfile(_))
    ));
    assert_eq!(
        service
            .create_profile(id("spot"), demo(VenueName::Bybit), Category::Spot)
            .expect_err("no spot profile")
            .to_string(),
        "invalid profile category \"spot\": a profile trades linear or inverse futures"
    );
    assert_eq!(
        service.profiles().expect("profiles").len(),
        2,
        "nothing left behind"
    );
}

/// A change names the revision it was made from, or none.
async fn a_change_names_the_revision_it_was_made_from_or_none<B: Backend>(backend: &B) {
    let fixture = fixture();
    let line = fixture.line("bybit linear");
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    store
        .create_profile(&ProfilePolicy::new(line.identity.clone()), T)
        .expect("a profile");
    let label = |text: &str| change(line, &json!({"label": text}));

    let first = service
        .change(&profile(), Some(revision(1)), &label("one"))
        .expect("from revision 1");
    assert_eq!(first.policy.revision(), revision(2));
    // Made from a revision the profile has left: told so, and nothing stored.
    let stale = service
        .change(&profile(), Some(revision(1)), &label("two"))
        .expect_err("the profile moved on");
    assert_eq!(
        stale.to_string(),
        "profile main is at revision 2, not 1: it was changed in the meantime; read it again \
         and repeat the change"
    );
    assert_eq!(service.policy(&profile()).expect("policy"), first.policy);
    // Naming none, it is made to whatever is newest.
    let third = service
        .change(&profile(), None, &label("two"))
        .expect("on the newest");
    assert_eq!(third.policy.revision(), revision(3));
    // A change that asks for what already is makes no revision, whichever
    // way it is asked.
    for expected in [None, Some(revision(3))] {
        let same = service
            .change(&profile(), expected, &label("two"))
            .expect("nothing to change");
        assert!(!same.changed);
        assert_eq!(same.policy.revision(), revision(3));
    }
    assert_eq!(service.history(&profile()).expect("history").len(), 3);

    let unknown = ProfileId::new("nobody").expect("a profile id");
    for refused in [
        service.change(&unknown, None, &label("x")).map(|_| ()),
        service.history(&unknown).map(|_| ()),
        service.policy_at(&unknown, revision(1)).map(|_| ()),
    ] {
        assert_eq!(
            refused.expect_err("no such profile").to_string(),
            "the store holds no profile nobody; create it first"
        );
    }
    assert_eq!(
        service
            .policy_at(&profile(), revision(9))
            .expect_err("no such revision")
            .to_string(),
        "profile main holds no revision 9 of its policy"
    );
    assert_eq!(
        service
            .policy_at(&profile(), revision(2))
            .expect("revision 2"),
        first.policy
    );
}

fn leverage(text: &str) -> Leverage {
    Leverage::new(Decimal::from_str_exact(text).expect("a decimal")).expect("a leverage")
}

/// The venue's adapter decides what an asset is: a change that would
/// enable what the venue does not take is refused before it is stored.
async fn the_venue_has_its_say_before_a_change_is_stored<B: Backend>(backend: &B) {
    let fixture = fixture();
    let line = fixture.line("bybit linear");
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    let venue = Venue::of(line);
    store
        .create_profile(&ProfilePolicy::new(line.identity.clone()), T)
        .expect("a profile");
    let symbol = |name: &str| Symbol::new(name).expect("a symbol");
    let settings = |enabled: bool, lever: Option<&str>| AssetSettings {
        enabled,
        risk_percent: Some(risk(&json!("1"))),
        stop: StopFallback::Inherit,
        leverage: lever.map(leverage),
    };

    // Added as the venue lists it: the settlement asset is the venue's.
    let added = service
        .add_asset(
            &profile(),
            None,
            &symbol("solusdt"),
            settings(true, Some("5")),
            &venue,
        )
        .await
        .expect("an asset the venue lists");
    let asset = added.policy.asset(&symbol("SOLUSDT")).expect("the asset");
    assert_eq!(asset.instrument(), line.listed("SOLUSDT").id());
    assert_eq!(added.policy.revision(), revision(2));

    // What the venue does not list, keeps outside the universe, or takes no
    // such leverage for: each refused, each naming why, nothing stored.
    let not_listed = service
        .add_asset(
            &profile(),
            None,
            &symbol("GONEUSDT"),
            settings(true, Some("5")),
            &venue,
        )
        .await
        .expect_err("not listed");
    assert_eq!(
        not_listed.to_string(),
        "the venue lists no bybit linear contract GONEUSDT"
    );
    for (name, lever, kind, why) in [
        (
            "BTC-26DEC26",
            "5",
            PolicyProblemKind::OutsideUniverse,
            "BTC-26DEC26: dated futures are outside the v0.3 universe",
        ),
        (
            "ETHPERP",
            "5",
            PolicyProblemKind::OutsideUniverse,
            "ETHPERP: linear contracts settled in USDC are refused; only USDT is",
        ),
        (
            "BTCUSDT",
            "101",
            PolicyProblemKind::LeverageOutOfRange,
            "BTCUSDT: the venue takes a leverage from 1 to 100.00 in steps of 0.01, not 101",
        ),
        (
            "BTCUSDT",
            "3.333",
            PolicyProblemKind::LeverageOffStep,
            "BTCUSDT: the venue takes a leverage from 1 to 100.00 in steps of 0.01, not 3.333",
        ),
    ] {
        let refused = service
            .add_asset(
                &profile(),
                None,
                &symbol(name),
                settings(true, Some(lever)),
                &venue,
            )
            .await
            .expect_err("the venue does not take it");
        let PolicyError::Rejected(problems) = &refused else {
            panic!("{name}: {refused:?}");
        };
        assert_eq!(problems.len(), 1, "{name}");
        assert_eq!(problems[0].kind, kind, "{name}");
        assert_eq!(
            refused.to_string(),
            format!("the venue does not take the change: {why}")
        );
    }
    assert_eq!(service.policy(&profile()).expect("policy"), added.policy);

    // Disabled, it is held to nothing yet; enabling it is where the venue
    // has its say.
    let parked = service
        .add_asset(
            &profile(),
            None,
            &symbol("BTC-26DEC26"),
            settings(false, Some("5")),
            &venue,
        )
        .await
        .expect("a disabled asset");
    let enable = change(
        line,
        &json!({"edit_asset": {"symbol": "BTC-26DEC26", "enabled": true}}),
    );
    let refused = service
        .change_checked(&profile(), None, &enable, &venue)
        .await
        .expect_err("outside the universe");
    assert!(matches!(refused, PolicyError::Rejected(_)), "{refused:?}");
    assert_eq!(service.policy(&profile()).expect("policy"), parked.policy);

    // A default leverage reaches every asset that inherits it.
    let inherit = change(
        line,
        &json!({"edit_asset": {"symbol": "SOLUSDT", "leverage": "clear"}}),
    );
    let defaults = |lever: &str| {
        PolicyChange::Defaults(DefaultsEdit {
            leverage: Edit::Set(leverage(lever)),
            ..DefaultsEdit::default()
        })
    };
    service
        .change_checked(&profile(), None, &defaults("20"), &venue)
        .await
        .expect("a default leverage");
    service
        .change_checked(&profile(), None, &inherit, &venue)
        .await
        .expect("inherited");
    let refused = service
        .change_checked(&profile(), None, &defaults("125"), &venue)
        .await
        .expect_err("past what the venue takes for SOLUSDT");
    assert_eq!(
        refused.to_string(),
        "the venue does not take the change: SOLUSDT: the venue takes a leverage from 1 to \
         100.00 in steps of 0.01, not 125"
    );
    // Capital is not the venue's to refuse: the check reports it.
    let check = service.check(&profile(), &venue).await.expect("a check");
    assert_eq!(check.problems.len(), 1);
    assert_eq!(check.problems[0].kind, PolicyProblemKind::NoCapital);
    assert!(!check.is_ready());

    // The venue delists an asset the profile holds enabled. The check says
    // so; the problem the policy already has blocks neither an unrelated
    // change nor the one that parks the asset, and enabling it again is
    // where the venue has its say.
    let mut delisted = Venue::of(line);
    delisted
        .instruments
        .retain(|listed| listed.id().symbol.as_str() != "SOLUSDT");
    let check = service.check(&profile(), &delisted).await.expect("a check");
    let kinds: Vec<PolicyProblemKind> = check.problems.iter().map(|found| found.kind).collect();
    assert_eq!(kinds, [PolicyProblemKind::NotListed]);
    let label = change(line, &json!({"label": "after the delisting"}));
    let switch = |enabled: bool| {
        change(
            line,
            &json!({"edit_asset": {"symbol": "SOLUSDT", "enabled": enabled}}),
        )
    };
    for unrelated in [label, switch(false)] {
        let outcome = service
            .change_checked(&profile(), None, &unrelated, &delisted)
            .await
            .expect("a change that enables nothing new");
        assert!(outcome.changed, "{unrelated:?}");
    }
    let parked = service.check(&profile(), &delisted).await.expect("a check");
    assert!(parked.is_ready(), "{parked:?}");
    let refused = service
        .change_checked(&profile(), None, &switch(true), &delisted)
        .await
        .expect_err("no longer listed");
    assert_eq!(
        refused.to_string(),
        "the venue does not take the change: SOLUSDT: the venue lists no bybit linear contract \
         SOLUSDT"
    );
    service
        .change_checked(&profile(), None, &switch(true), &venue)
        .await
        .expect("listed again");

    // Another line's connector, or another environment's, is no say at all.
    let other_line = Venue::of(fixture.line("bybit inverse"));
    let mut other_environment = Venue::of(line);
    other_environment.environment = Environment::Testnet;
    for (wrong, named) in [
        (
            &other_line as &dyn InstrumentConnector,
            "a bybit inverse connector for demo",
        ),
        (&other_environment, "a bybit linear connector for testnet"),
    ] {
        let refused = service.check(&profile(), wrong).await.expect_err("wrong");
        assert_eq!(
            refused.to_string(),
            format!(
                "{named} is not the connector of profile main, which trades bybit linear on demo"
            )
        );
    }
}

/// The same symbol on Binance and on Bybit shares nothing: each profile's
/// settings, its switch and its revisions are its own.
async fn the_same_symbol_on_two_venues_shares_no_setting_and_no_switch<B: Backend>(backend: &B) {
    let fixture = fixture();
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    let mut profiles = Vec::new();
    for (name, line) in [
        ("on-binance", "binance linear"),
        ("on-bybit", "bybit linear"),
    ] {
        let line = fixture.line(line);
        let id = ProfileId::new(name).expect("a profile id");
        let identity = ProfileIdentity::new(
            id.clone(),
            line.identity.account_ref().clone(),
            line.identity.category(),
            OwnerTag::generate(),
        )
        .expect("an identity");
        store
            .create_profile(&ProfilePolicy::new(identity), T)
            .expect("a profile");
        for asked in [
            json!({"defaults": {"risk_percent": {"set": "1"}, "leverage": {"set": "5"}}}),
            json!({"add_asset": {"symbol": "SOLUSDT", "settle": "USDT", "enabled": true}}),
            json!({"set_capital": {"amount": "15000", "asset": "USDT"}}),
        ] {
            service
                .change(&id, None, &change(line, &asked))
                .expect("a change");
        }
        profiles.push((id, line));
    }
    let (binance, binance_line) = &profiles[0];
    let (bybit, bybit_line) = &profiles[1];
    let before = stored(&service.policy(bybit).expect("policy"));

    // Binance's SOLUSDT is given other settings, disabled, and loses its
    // capital: nothing of Bybit's moves.
    for asked in [
        json!({"edit_asset": {"symbol": "SOLUSDT", "risk_percent": {"set": "9"},
                              "leverage": {"set": "50"}}}),
        json!({"edit_asset": {"symbol": "SOLUSDT", "enabled": false}}),
        json!({"clear_capital": "USDT"}),
    ] {
        service
            .change(binance, None, &change(binance_line, &asked))
            .expect("a change");
    }
    assert_eq!(stored(&service.policy(bybit).expect("policy")), before);
    assert_eq!(service.history(bybit).expect("history").len(), 4);
    assert_eq!(service.history(binance).expect("history").len(), 7);
    let entry = |id: &ProfileId, line: &Line| service.entry(id, line.listed("SOLUSDT"));
    let refused = entry(binance, binance_line).expect_err("disabled on binance");
    assert_eq!(
        refused.to_string(),
        "not_armed: SOLUSDT is disabled in profile on-binance; enable the asset to take entries \
         on it"
    );
    let taken = entry(bybit, bybit_line).expect("still enabled on bybit");
    assert_eq!(taken.risk_percent().value.to_string(), "1");
    assert_eq!(taken.leverage().value.to_string(), "5");
    // One profile's instrument is never the other's.
    let crossed = service
        .entry(bybit, binance_line.listed("SOLUSDT"))
        .expect_err("binance's contract in bybit's profile");
    assert_eq!(
        crossed.to_string(),
        "ambiguous_selection: SOLUSDT is a binance linear contract, and profile on-bybit trades \
         bybit linear"
    );
}

/// A store that is bound to no environment yet holds no profile.
#[test]
fn a_store_bound_to_no_environment_takes_no_profile() {
    let dir = tempfile::tempdir().expect("a directory");
    let store = quantforge::SqliteStore::new(dir.path().join("market.sqlite"));
    quantforge::CandleStore::init(&store).expect("init");
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    let refused = service
        .create_profile(
            profile(),
            account(VenueName::Bybit, Environment::Demo, "1"),
            Category::Linear,
        )
        .expect_err("unbound");
    assert_eq!(
        refused.to_string(),
        "invalid argument: the store is bound to no environment yet, and profile main needs \
         one; bind the store first"
    );
    store.bind_environment(Environment::Demo).expect("bound");
    assert!(service.profiles().expect("profiles").is_empty());
}

on_each_backend!(
    a_profile_is_created_once_empty_with_a_tag_of_its_own,
    a_change_names_the_revision_it_was_made_from_or_none,
    the_venue_has_its_say_before_a_change_is_stored,
    the_same_symbol_on_two_venues_shares_no_setting_and_no_switch,
);
