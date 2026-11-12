//! What an admitted entry carries of its profile's policy, on each
//! backend: the exact settings of the revision it was admitted under,
//! which no later edit reaches, and which the store reproduces for good.
//! And what never reaches the policy: anything a signal says. An exit, and
//! everything else that only reduces a position, takes no policy at all.

use quantforge::PolicyStore;
use quantforge::automation::PolicyService;
use quantforge::model::futures::{
    EntryIntent, EntryStop, ExitIntent, ExternalIntent, IntentKind, IntentState, PolicySnapshot,
    PositionKey, ProfileId, ProfilePolicy, SignalId,
};
use quantforge::model::{PositionSide, Refusal};
use serde_json::{Value, json};

use crate::cases::{Line, change, fixture, price};
use crate::support::{Backend, T, Ticking, on_each_backend};

fn profile() -> ProfileId {
    ProfileId::new("main").expect("a profile id")
}

/// An entry signal on `line`'s main instrument, as a payload gives it:
/// a time, a side, a price and, perhaps, a stop.
fn entry(line: &Line, signal: &str, expected: &str, stop: Option<&str>) -> ExternalIntent {
    let instrument = line.listed(&line.symbols.main).id().clone();
    let key = PositionKey::new(line.identity.account_ref().clone(), instrument).expect("a key");
    ExternalIntent::new(IntentKind::Entry(EntryIntent {
        profile: profile(),
        signal: SignalId::new(signal).expect("a signal id"),
        signal_time_ms: T,
        key,
        side: PositionSide::Long,
        expected_price: price(expected),
        stop: stop.map(price),
    }))
}

fn exit(line: &Line) -> ExternalIntent {
    let instrument = line.listed(&line.symbols.main).id().clone();
    let key = PositionKey::new(line.identity.account_ref().clone(), instrument).expect("a key");
    ExternalIntent::new(IntentKind::Exit(ExitIntent {
        profile: profile(),
        signal: SignalId::new("exit").expect("a signal id"),
        signal_time_ms: T + 60_000,
        key,
        side: PositionSide::Long,
    }))
}

/// The profile of `line`, with defaults, its main asset and capital.
fn configured(store: &impl PolicyStore, service: &PolicyService<'_>, line: &Line) {
    store
        .create_profile(&ProfilePolicy::new(line.identity.clone()), T)
        .expect("a profile");
    let main = &line.symbols.main;
    let settle = line.listed(main).id().settle.as_str().to_string();
    for asked in [
        json!({"defaults": {"risk_percent": {"set": "1"}, "stop_percent": {"set": "3"},
                            "leverage": {"set": "5"}}}),
        json!({"add_asset": {"symbol": main, "settle": settle, "enabled": true,
                             "risk_percent": "0.75"}}),
        json!({"set_capital": {"amount": "15000.00", "asset": settle}}),
    ] {
        service
            .change(&profile(), None, &change(line, &asked))
            .expect("a change");
    }
}

/// An admitted entry keeps the policy of the revision it was admitted
/// under: later edits, a disabled asset and a restart leave it as it was,
/// and the store still answers that revision, word for word.
async fn an_admitted_entry_keeps_the_exact_policy_it_was_admitted_under<B: Backend>(backend: &B) {
    let fixture = fixture();
    for line in &fixture.lines {
        if line.name != "bybit linear" && line.name != "binance inverse" {
            continue;
        }
        let Some(fresh) = backend.another() else {
            continue;
        };
        let store = fresh.open();
        let clock = Ticking::default();
        let service = PolicyService::new(&store, &clock);
        configured(&store, &service, line);
        let listed = line.listed(&line.symbols.main);

        let taken = service.entry(&profile(), listed).expect("a snapshot");
        let mut intent = entry(line, "entry 1", "234.23", Some("200.00"));
        intent.admit(Some(taken.clone())).expect("admitted");
        assert_eq!(intent.state(), IntentState::Admitted);
        assert_eq!(intent.policy(), Some(&taken));
        assert_eq!(taken.revision().get(), 4);
        assert_eq!(taken.capital_revision().get(), 4);
        assert_eq!(taken.account(), line.identity.account_ref());
        assert_eq!(taken.risk_percent().value.to_string(), "0.75");
        assert_eq!(taken.capital().amount().to_string(), "15000.00");
        let recorded = serde_json::to_string(&intent).expect("json");

        // The operator edits everything the entry was sized from, then
        // disables the asset and takes its capital away.
        let main = &line.symbols.main;
        for asked in [
            json!({"edit_asset": {"symbol": main, "risk_percent": {"set": "9"},
                                  "leverage": {"set": "2"}, "stop": "off"}}),
            json!({"set_capital": {"amount": "1", "asset": listed.id().settle.as_str()}}),
            json!({"edit_asset": {"symbol": main, "enabled": false}}),
            json!({"clear_capital": listed.id().settle.as_str()}),
        ] {
            service
                .change(&profile(), None, &change(line, &asked))
                .expect("a change");
        }
        assert_eq!(
            service.policy(&profile()).expect("policy").revision().get(),
            8
        );
        // Nothing of it reached the admitted entry: it is what was recorded.
        assert_eq!(serde_json::to_string(&intent).expect("json"), recorded);
        // New entries are held to the newest revision, and refused.
        let refused = service.entry(&profile(), listed).expect_err("disabled now");
        assert!(refused.to_string().starts_with("not_armed: "), "{refused}");

        // A process that starts on the store reads the entry back with its
        // policy, and the store reproduces that policy from the revision
        // the entry names.
        let reopened = fresh.open();
        let later = PolicyService::new(&reopened, &clock);
        let back: ExternalIntent = serde_json::from_str(&recorded).expect("the recorded intent");
        assert_eq!(back, intent);
        let held = back.policy().expect("the policy it was admitted under");
        let then = later
            .policy_at(&profile(), held.revision())
            .expect("the revision it names");
        assert_eq!(&then.entry_snapshot(listed).expect("as it was"), held);
        let capital = then.capital_in(&listed.id().settle).expect("its capital");
        assert_eq!(capital.revision(), held.capital_revision());
    }
}

/// A signal's only say is its own stop: nothing it carries reaches the
/// risk, the leverage, the capital or the asset's switch.
async fn nothing_a_signal_says_reaches_the_policy_but_its_own_stop<B: Backend>(backend: &B) {
    let fixture = fixture();
    let line = fixture.line("bybit linear");
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    configured(&store, &service, line);
    let listed = line.listed(&line.symbols.main);

    // Whatever the payload says of its time, its price and its stop, the
    // entry is admitted under the same policy.
    let signals = [
        entry(line, "a", "234.23", Some("200.00")),
        entry(line, "b", "0.00000001", None),
        entry(line, "c", "999999999", Some("1")),
    ];
    let mut snapshots: Vec<PolicySnapshot> = Vec::new();
    for mut intent in signals {
        let taken = service.entry(&profile(), listed).expect("a snapshot");
        intent.admit(Some(taken.clone())).expect("admitted");
        snapshots.push(taken);
    }
    assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));

    // The stop is the one thing a signal chooses: its own comes first, the
    // fallback only without one.
    let policy = &snapshots[0];
    assert_eq!(
        policy.stop_percent().value.map(|p| p.to_string()),
        Some("3".to_string())
    );
    assert_eq!(
        policy.stop_for(Some(price("200.00"))),
        Ok(EntryStop::Signal(price("200.00")))
    );
    assert!(
        matches!(policy.stop_for(None), Ok(EntryStop::Fallback(percent)) if percent.to_string() == "3")
    );

    // An entry's stored form has no field a payload could fill for the
    // account, the credentials, the risk or the switch.
    let Value::Object(fields) = serde_json::to_value(entry(line, "d", "1", None)).expect("json")
    else {
        panic!("an object");
    };
    let entry_fields: Vec<&String> = fields["kind"]["Entry"]
        .as_object()
        .expect("an entry")
        .keys()
        .collect();
    assert_eq!(
        entry_fields,
        [
            "expected_price",
            "key",
            "profile",
            "side",
            "signal",
            "signal_time_ms",
            "stop"
        ]
    );
}

/// Only an entry is held to the policy. Disabling an asset, or the whole
/// policy going away, never holds back an exit.
async fn an_exit_takes_no_policy_and_no_switch_holds_it_back<B: Backend>(backend: &B) {
    let fixture = fixture();
    let line = fixture.line("binance inverse");
    let store = backend.open();
    let clock = Ticking::default();
    let service = PolicyService::new(&store, &clock);
    configured(&store, &service, line);
    let listed = line.listed(&line.symbols.main);
    let main = &line.symbols.main;
    let taken = service.entry(&profile(), listed).expect("a snapshot");

    // The asset is disabled and its capital removed while a position of it
    // is open: entries stop, and the exit is admitted as ever.
    for asked in [
        json!({"edit_asset": {"symbol": main, "enabled": false}}),
        json!({"clear_capital": listed.id().settle.as_str()}),
        json!({"defaults": {"enabled": false}}),
    ] {
        let _ = service
            .change(&profile(), None, &change(line, &asked))
            .expect("a change");
    }
    let refused = service.entry(&profile(), listed).expect_err("no entry");
    let quantforge::automation::PolicyError::Refused(refused) = refused else {
        panic!("a refusal of the policy");
    };
    assert_eq!(refused.refusal, Refusal::NotArmed);
    let mut closing = exit(line);
    closing
        .admit(None)
        .expect("an exit is admitted without a policy");
    assert_eq!(closing.state(), IntentState::Admitted);
    assert_eq!(closing.policy(), None);

    // An entry on the profile's own instrument, keyed to another account
    // of the same venue, is never admitted under this profile's policy.
    let mut another = entry(line, "h", "1", None);
    let mut record = serde_json::to_value(&another).expect("json");
    let account = &mut record["kind"]["Entry"]["key"]["account"]["venue_account_id"];
    *account = json!({"Venue": "99999999"});
    another = serde_json::from_value(record).expect("an entry on another account");
    let error = another
        .admit(Some(taken.clone()))
        .expect_err("another account's entry");
    assert!(error.to_string().contains("its account"), "{error}");

    // An exit is never given a policy, and an entry never admitted
    // without its own.
    assert!(exit(line).admit(Some(taken.clone())).is_err());
    assert!(entry(line, "e", "1", None).admit(None).is_err());
    assert!(
        entry(line, "e", "1", None)
            .move_to(IntentState::Admitted)
            .is_err()
    );
    let other = fixture.line("bybit inverse");
    // The other store's backend lives until its snapshot is taken: a
    // store's files go with it.
    let foreign = backend.another().map(|fresh| {
        let other_store = fresh.open();
        let service = PolicyService::new(&other_store, &clock);
        configured(&other_store, &service, other);
        service
            .entry(&profile(), other.listed(&other.symbols.main))
            .expect("another instrument's snapshot")
    });
    if let Some(foreign) = foreign {
        let error = entry(line, "f", "1", None)
            .admit(Some(foreign))
            .expect_err("another instrument's policy");
        assert!(
            error.to_string().contains("its own profile's policy"),
            "{error}"
        );
    }

    // What a store hands back is held to the same rules.
    let mut admitted = entry(line, "g", "1", None);
    admitted.admit(Some(taken)).expect("admitted");
    let mut stripped = serde_json::to_value(&admitted).expect("json");
    stripped["policy"] = Value::Null;
    assert!(serde_json::from_value::<ExternalIntent>(stripped).is_err());
    let mut given = serde_json::to_value(&closing).expect("json");
    given["policy"] = serde_json::to_value(admitted.policy()).expect("json");
    assert!(serde_json::from_value::<ExternalIntent>(given).is_err());
}

/// What an entry would be sized from, the revision it is taken at aside.
fn settings(snapshot: &PolicySnapshot) -> impl PartialEq + std::fmt::Debug + '_ {
    (
        snapshot.instrument(),
        snapshot.risk_percent(),
        snapshot.stop_percent(),
        snapshot.leverage(),
        snapshot.capital(),
        snapshot.capital_revision(),
    )
}

/// An asset's switch is its own. Disabled, its entries stop and no other
/// asset's; enabled again, it trades on the settings it had.
async fn an_assets_switch_stops_its_own_entries_and_no_other_assets<B: Backend>(backend: &B) {
    let fixture = fixture();
    for line in &fixture.lines {
        let Some(fresh) = backend.another() else {
            continue;
        };
        let store = fresh.open();
        let clock = Ticking::default();
        let service = PolicyService::new(&store, &clock);
        configured(&store, &service, line);
        let (main, second) = (&line.symbols.main, &line.symbols.second);
        let (listed, other) = (line.listed(main), line.listed(second));
        let settle = other.id().settle.as_str();
        for asked in [
            json!({"add_asset": {"symbol": second, "settle": settle, "enabled": true,
                                 "leverage": "2"}}),
            // On a line whose assets settle in one asset this changes nothing.
            json!({"set_capital": {"amount": "15000.00", "asset": settle}}),
        ] {
            service
                .change(&profile(), None, &change(line, &asked))
                .expect("a change");
        }
        let main_before = service.entry(&profile(), listed).expect("a snapshot");
        let other_before = service.entry(&profile(), other).expect("a snapshot");
        assert_eq!(
            other_before.leverage().value.to_string(),
            "2",
            "{}",
            line.name
        );

        let switch = |enabled: bool| {
            let asked = json!({"edit_asset": {"symbol": main, "enabled": enabled}});
            let outcome = service
                .change(&profile(), None, &change(line, &asked))
                .expect("a change");
            assert!(outcome.changed, "{}", line.name);
        };
        switch(false);
        let refused = service.entry(&profile(), listed).expect_err("disabled");
        assert_eq!(
            refused.to_string(),
            format!(
                "not_armed: {main} is disabled in profile main; enable the asset to take \
                 entries on it"
            ),
            "{}",
            line.name
        );
        // The other asset trades as before, under the newer revision.
        let other_after = service.entry(&profile(), other).expect("a snapshot");
        assert_eq!(
            settings(&other_after),
            settings(&other_before),
            "{}",
            line.name
        );
        assert_eq!(
            other_after.revision().get(),
            other_before.revision().get() + 1,
            "{}",
            line.name
        );
        // A disabled asset keeps its settings, and is still looked at.
        let held = service.policy(&profile()).expect("the policy");
        let effective = held.effective(listed.id()).expect("what it would trade on");
        assert!(effective.configured && !effective.enabled, "{}", line.name);
        assert_eq!(
            effective.risk_percent.map(|risk| risk.value.to_string()),
            Some("0.75".to_string()),
            "{}",
            line.name
        );

        switch(true);
        let main_after = service.entry(&profile(), listed).expect("a snapshot");
        assert_eq!(
            settings(&main_after),
            settings(&main_before),
            "{}",
            line.name
        );
        assert_eq!(
            main_after.revision().get(),
            main_before.revision().get() + 2,
            "{}",
            line.name
        );
    }
}

on_each_backend!(
    an_admitted_entry_keeps_the_exact_policy_it_was_admitted_under,
    an_assets_switch_stops_its_own_entries_and_no_other_assets,
    nothing_a_signal_says_reaches_the_policy_but_its_own_stop,
    an_exit_takes_no_policy_and_no_switch_holds_it_back,
);
