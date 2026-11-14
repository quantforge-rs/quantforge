//! What a resolved signal carries, and a signal against what its profile
//! already has on record: the stop an entry needs, a delivery sent again,
//! the position an exit closes, and the side an entry meets.

use super::*;

#[test]
fn a_resolved_entry_carries_its_profiles_account_and_policy_and_an_exit_no_policy() {
    let policy = policy();
    let venue = venue();
    let now_ms = FIRED + 2_000;
    let resolved = entry("SOLUSDT.P")
        .resolve(&policy, &venue, now_ms, SignalAgeLimit::DEFAULT)
        .expect("resolved");
    let IntentKind::Entry(intent) = &resolved.intent else {
        panic!("no entry");
    };
    assert_eq!(&intent.profile, policy.identity().id());
    assert_eq!(intent.key.account(), policy.identity().account_ref());
    assert_eq!(intent.key.instrument(), venue[0].id());
    assert_eq!(intent.signal_time_ms, FIRED);
    assert_eq!(intent.signal.as_str(), "trend:SOLUSDT.P");
    assert_eq!(
        (intent.expected_price, intent.stop),
        (price("234.23"), None)
    );
    let snapshot = resolved.policy.as_ref().expect("a policy");
    assert_eq!(snapshot.revision(), policy.revision());
    assert_eq!(snapshot.instrument(), venue[0].id());
    assert_eq!(resolved.spelling, TickerSpelling::TradingView);

    let resolved = exit("SOLUSDT")
        .resolve(&policy, &venue, now_ms, SignalAgeLimit::DEFAULT)
        .expect("resolved");
    assert!(matches!(resolved.intent, IntentKind::Exit(_)));
    assert_eq!(resolved.policy, None);
    assert_eq!(resolved.spelling, TickerSpelling::VenueSymbol);
}

#[test]
fn an_entry_without_a_stop_needs_the_policys_fallback() {
    let policy = policy();
    let venue = venue();
    let resolve = |policy: &ProfilePolicy, signal: &ExternalSignal| {
        signal
            .resolve(policy, &venue, FIRED, SignalAgeLimit::DEFAULT)
            .map(|resolved| {
                resolved
                    .policy
                    .map(|snapshot| snapshot.stop_percent().value)
            })
            .map_err(|refused| refused.refusal)
    };
    let fallback = StopPercent::new(Decimal::TWO).expect("a stop");
    assert_eq!(
        resolve(&policy, &entry("SOLUSDT.P")),
        Ok(Some(Some(fallback)))
    );

    // With the fallback gone, an entry needs a stop of its own; an exit
    // needs neither.
    let bare = policy
        .apply(&PolicyChange::Defaults(DefaultsEdit {
            stop_percent: Edit::Clear,
            ..DefaultsEdit::default()
        }))
        .expect("a change")
        .expect("a revision");
    assert_eq!(resolve(&bare, &entry("SOLUSDT.P")), Err(Refusal::NoStop));
    let with_stop = signal(
        "SOLUSDT.P",
        SignalAction::Entry {
            expected_price: price("234.23"),
            stop: Some(price("200")),
        },
    );
    assert_eq!(resolve(&bare, &with_stop), Ok(Some(None)));
    assert_eq!(resolve(&bare, &exit("SOLUSDT.P")), Ok(None));
}

#[test]
fn a_delivery_is_told_by_the_hash_recorded_for_its_profile_and_id() {
    let first = SignalHash::from_digest([1; 32]);
    let other = SignalHash::from_digest([2; 32]);
    assert_eq!(Delivery::of(None, &first), Delivery::New);
    assert_eq!(Delivery::of(Some(&first), &first), Delivery::Duplicate);
    assert_eq!(Delivery::of(Some(&first), &other), Delivery::Conflict);
    assert_eq!(Delivery::New.refusal(), None);
    assert_eq!(Delivery::Duplicate.refusal(), None);
    assert_eq!(Delivery::Conflict.refusal(), Some(Refusal::Conflict));
    // A hash is written as the payload format writes it.
    let text = first.to_string();
    assert_eq!(text, format!("sha256:{}", "01".repeat(32)));
    assert_eq!(text.parse::<SignalHash>().expect("a hash"), first);
    assert_eq!(
        "sha256:AB"
            .parse::<SignalHash>()
            .expect_err("refused")
            .to_string(),
        "invalid signal hash \"sha256:AB\": expected sha256: and 64 lowercase hex digits"
    );
}

/// An entry and an exit of profile `main` on SOLUSDT, as they resolve.
fn intents() -> (EntryIntent, ExitIntent) {
    let policy = policy();
    let venue = venue();
    let resolve = |signal: ExternalSignal| {
        signal
            .resolve(&policy, &venue, FIRED, SignalAgeLimit::DEFAULT)
            .expect("resolved")
            .intent
    };
    match (resolve(entry("SOLUSDT.P")), resolve(exit("SOLUSDT.P"))) {
        (IntentKind::Entry(entry), IntentKind::Exit(exit)) => (entry, exit),
        other => panic!("an entry and an exit: {other:?}"),
    }
}

#[test]
fn an_exit_closes_a_position_only_when_it_fired_after_its_entry() {
    let (entry, mut exit) = intents();
    // Both fired in the same second: the exit was not meant for it.
    assert!(!exit.closes(&entry));
    exit.signal_time_ms = entry.signal_time_ms + 1;
    assert!(exit.closes(&entry));
    exit.signal_time_ms = entry.signal_time_ms - 1;
    assert!(!exit.closes(&entry));
    exit.signal_time_ms = i64::MAX;
    assert!(exit.closes(&entry));

    let later = |change: &dyn Fn(&mut ExitIntent)| {
        let mut changed = exit.clone();
        change(&mut changed);
        changed.closes(&entry)
    };
    assert!(!later(&|exit| exit.side = PositionSide::Short));
    assert!(!later(
        &|exit| exit.profile = ProfileId::new("other").expect("a profile id")
    ));
    assert!(!later(&|exit| {
        let other = venue()[1].id().clone();
        exit.key = PositionKey::new(exit.key.account().clone(), other).expect("a key");
    }));
    assert!(!later(&|exit| {
        let account = VenueAccountId::issued("67890").expect("an account id");
        let other = AccountRef::new(Venue::Bybit, Environment::Demo, account).expect("an account");
        exit.key = PositionKey::new(other, exit.key.instrument().clone()).expect("a key");
    }));
}

#[test]
fn an_entry_meets_the_side_already_held() {
    let (entry, _) = intents();
    assert_eq!(entry.against(None), Ok(()));
    assert_eq!(
        refused(entry.against(Some(PositionSide::Long))),
        Refusal::Pyramiding
    );
    assert_eq!(
        refused(entry.against(Some(PositionSide::Short))),
        Refusal::Reversal
    );
    let mut short = entry;
    short.side = PositionSide::Short;
    assert_eq!(
        refused(short.against(Some(PositionSide::Short))),
        Refusal::Pyramiding
    );
    assert_eq!(
        refused(short.against(Some(PositionSide::Long))),
        Refusal::Reversal
    );
}

#[test]
fn a_refusal_reads_as_its_name_and_what_to_change_and_takes_a_policys_as_its_own() {
    let refusal = SignalRefusal::new(Refusal::Stale, "the alert is old");
    assert_eq!(refusal.to_string(), "stale: the alert is old");
    let policy = policy();
    let disabled = policy
        .apply(&PolicyChange::Defaults(DefaultsEdit {
            enabled: Some(false),
            ..DefaultsEdit::default()
        }))
        .expect("a change")
        .expect("a revision");
    let from_policy = disabled
        .entry_snapshot(&venue()[1])
        .expect_err("refused by the policy");
    let taken = SignalRefusal::from(from_policy.clone());
    assert_eq!(
        (taken.refusal, &taken.message),
        (from_policy.refusal, &from_policy.message)
    );
    // And that is the refusal the signal is resolved to.
    let resolved = entry("BTCUSDT.P").resolve(&disabled, &venue(), FIRED, SignalAgeLimit::DEFAULT);
    assert_eq!(resolved, Err(taken));
}
