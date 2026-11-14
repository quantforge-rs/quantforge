//! Every shared case on the pure model: what each signal resolves to for
//! the profile its URL selected, which exit closes which entry's position,
//! which delivery is a repeat and which a conflict, and what an entry is
//! against the side already held.

use std::collections::{BTreeMap, BTreeSet};

use quantforge::model::futures::{
    Delivery, EntryIntent, ExitIntent, ExternalIntent, ExternalSignal, FuturesInstrument,
    IntentKind, IntentState, ProfileId, ResolvedSignal, SignalAction, SignalAgeLimit, SignalHash,
    SignalId, SignalRefusal,
};
use quantforge::model::{PositionSide, Refusal, TimestampMs};
use quantforge::server::read_signal;
use serde_json::{Value, json};

use crate::cases::{Fired, Fixture, Line, Resolution, fixture};

/// A resolved signal, as the fixture writes one.
pub(crate) fn as_recorded(resolved: &ResolvedSignal) -> Value {
    let (kind, key, side, signal, fired) = match &resolved.intent {
        IntentKind::Entry(entry) => (
            "entry",
            &entry.key,
            entry.side,
            &entry.signal,
            entry.signal_time_ms,
        ),
        IntentKind::Exit(exit) => (
            "exit",
            &exit.key,
            exit.side,
            &exit.signal,
            exit.signal_time_ms,
        ),
    };
    let mut recorded = json!({
        "kind": kind,
        "side": side.as_str(),
        "instrument": key.instrument(),
        "account": key.account(),
        "spelling": resolved.spelling.as_str(),
        "signal_id": signal.as_str(),
        "signal_time_ms": fired,
    });
    if let IntentKind::Entry(entry) = &resolved.intent {
        recorded["expected_price"] = json!(entry.expected_price.to_string());
        recorded["stop"] = json!(entry.stop.map(|stop| stop.to_string()));
        recorded["policy"] = serde_json::to_value(&resolved.policy).expect("a snapshot");
    } else {
        assert_eq!(resolved.policy, None, "an exit takes no policy");
    }
    recorded
}

fn limit_of(case: &Resolution) -> SignalAgeLimit {
    SignalAgeLimit::new(case.age_limit_s).expect("an age limit")
}

/// The case's signal resolved on its line, as the model alone decides.
pub(crate) fn resolved_on_the_model(
    fixture: &Fixture,
    case: &Resolution,
) -> Result<ResolvedSignal, SignalRefusal> {
    let line = fixture.line(&case.line);
    fixture.signal(&case.payload).resolve(
        &line.policy(),
        &fixture.listed_with_others(line),
        case.now_ms,
        limit_of(case),
    )
}

/// Whether a refusal's message names the part of the signal it is over.
fn speaks_of(over: &str, refusal: &SignalRefusal, signal: &ExternalSignal, line: &Line) -> bool {
    let message = &refusal.message;
    let identity = &line.identity;
    match over {
        "profile" => {
            message.contains(signal.profile.as_str()) && message.contains(identity.id().as_str())
        }
        "exchange" => {
            let asserted = signal.exchange.as_deref().expect("an exchange");
            message.contains(asserted) && message.contains(identity.account_ref().venue().as_str())
        }
        "category" => {
            let asserted = signal.category.expect("a family");
            message.contains(asserted.as_str()) && message.contains(identity.category().as_str())
        }
        "fire_time" => message.contains(" s ago") || message.contains(" s ahead"),
        "ticker" => message.contains(&signal.ticker),
        // The policy speaks of the venue's symbol, not of the ticker.
        "policy" => {
            let listed = line.listed();
            let named = line
                .policy()
                .instrument_named(&signal.ticker, &listed)
                .expect("an instrument");
            message.contains(named.instrument.id().symbol.as_str())
        }
        "stop" => message.contains("no stop"),
        other => panic!("the fixture has refusals over {other}, which no test reads"),
    }
}

#[test]
fn every_signal_resolves_as_the_cases_say() {
    let fixture = fixture();
    let mut wrong = Vec::new();
    let (mut resolved, mut refused) = (0, 0);
    for case in &fixture.resolutions {
        let line = fixture.line(&case.line);
        let signal = fixture.signal(&case.payload);
        let outcome = &case.outcome;
        match resolved_on_the_model(&fixture, case) {
            Ok(got) => {
                resolved += 1;
                let recorded = as_recorded(&got);
                if Some(&recorded) != outcome.resolved.as_ref() {
                    wrong.push(format!(
                        "{}:\n  resolved {recorded}\n  recorded {:?}",
                        case.id, outcome
                    ));
                    continue;
                }
                // What resolves is admitted as it stands: an entry under
                // the policy it was given, an exit under none.
                let mut intent = ExternalIntent::new(got.intent.clone());
                intent
                    .admit(got.policy.clone())
                    .unwrap_or_else(|error| panic!("{}: {error}", case.id));
                assert_eq!(intent.state(), IntentState::Admitted, "{}", case.id);
                assert_eq!(got.intent.key().account(), line.identity.account_ref());
            }
            Err(refusal) => {
                refused += 1;
                let over = outcome.over.as_deref().unwrap_or("nothing");
                if outcome.refused.as_deref() != Some(refusal.refusal.as_str()) {
                    wrong.push(format!(
                        "{}: refused as {refusal}\n  recorded {:?}",
                        case.id, outcome
                    ));
                } else if !speaks_of(over, &refusal, &signal, line) {
                    wrong.push(format!(
                        "{}: a refusal over the {over} says: {}",
                        case.id, refusal.message
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(
        resolved >= 30 && refused >= 25,
        "{resolved} resolved, {refused} refused"
    );
}

#[test]
fn a_profiles_policy_is_one_revision_on_for_every_change() {
    let fixture = fixture();
    for line in &fixture.lines {
        let policy = line.policy();
        assert_eq!(
            policy.revision().get(),
            1 + line.changes.len() as u64,
            "{}",
            line.name
        );
        assert_eq!(policy.identity(), &line.identity, "{}", line.name);
    }
}

#[test]
fn what_needs_no_venue_is_refused_whatever_the_venue_lists() {
    // Whose a signal is, and how old, are decided before its ticker is
    // looked up: the same refusal with nothing listed at all.
    let fixture = fixture();
    let nothing: Vec<FuturesInstrument> = Vec::new();
    let mut decided_first = 0;
    for case in &fixture.resolutions {
        let Some(over) = case.outcome.over.as_deref() else {
            continue;
        };
        let line = fixture.line(&case.line);
        let signal = fixture.signal(&case.payload);
        let unlisted = signal.resolve(&line.policy(), &nothing, case.now_ms, limit_of(case));
        let refusal = unlisted.expect_err("nothing is listed");
        let recorded = case.outcome.refused.as_deref();
        if ["profile", "exchange", "category", "fire_time"].contains(&over) {
            decided_first += 1;
            assert_eq!(Some(refusal.refusal.as_str()), recorded, "{}", case.id);
            // And each rule alone says the same.
            let alone = signal
                .addressed_to(&line.identity)
                .and_then(|()| signal.timely(case.now_ms, limit_of(case)));
            assert_eq!(alone, Err(refusal), "{}", case.id);
        } else {
            assert_eq!(refusal.refusal, Refusal::UnknownTicker, "{}", case.id);
        }
    }
    assert!(decided_first >= 12, "{decided_first}");
}

/// Every part of a signal that breaks its rule, each rule asked on its own
/// of the model: the profile, then each assertion with the rest put right,
/// the clock, the ticker, and, once the ticker names an instrument, the
/// policy and the stop.
fn broken_parts(fixture: &Fixture, case: &Resolution) -> Vec<&'static str> {
    let line = fixture.line(&case.line);
    let policy = line.policy();
    let signal = fixture.signal(&case.payload);
    let mut broken = Vec::new();
    if &signal.profile != line.profile() {
        broken.push("profile");
    }
    let mut asserting = signal.clone();
    asserting.profile = line.profile().clone();
    let mut exchange_only = asserting.clone();
    exchange_only.category = None;
    if exchange_only.addressed_to(&line.identity).is_err() {
        broken.push("exchange");
    }
    let mut category_only = asserting;
    category_only.exchange = None;
    if category_only.addressed_to(&line.identity).is_err() {
        broken.push("category");
    }
    if signal.timely(case.now_ms, limit_of(case)).is_err() {
        broken.push("fire_time");
    }
    let listed = fixture.listed_with_others(line);
    match policy.instrument_named(&signal.ticker, &listed) {
        Err(_) => broken.push("ticker"),
        Ok(named) => {
            if let SignalAction::Entry { stop, .. } = signal.action {
                if policy.entry_snapshot(named.instrument).is_err() {
                    broken.push("policy");
                }
                let fallback = policy
                    .effective(named.instrument.id())
                    .expect("the instrument's settings")
                    .stop_percent
                    .value;
                if stop.is_none() && fallback.is_none() {
                    broken.push("stop");
                }
            }
        }
    }
    broken
}

#[test]
fn a_signal_that_breaks_two_rules_is_refused_over_the_first() {
    let fixture = fixture();
    let order = &fixture.resolution_order;
    assert_eq!(
        order,
        &[
            "profile",
            "exchange",
            "category",
            "fire_time",
            "ticker",
            "policy",
            "stop"
        ]
    );
    let place = |part: &str| {
        order
            .iter()
            .position(|known| known == part)
            .expect("a part")
    };
    let mut shown = BTreeSet::new();
    for case in &fixture.resolutions {
        // Each rule alone breaks exactly where the Python model says.
        assert_eq!(broken_parts(&fixture, case), case.breaks, "{}", case.id);
        assert!(
            case.breaks
                .windows(2)
                .all(|pair| place(&pair[0]) < place(&pair[1])),
            "{}",
            case.id
        );
        assert_eq!(
            case.breaks.first().map(String::as_str),
            case.outcome.over.as_deref(),
            "{}",
            case.id
        );
        if let [first, rest @ ..] = case.breaks.as_slice() {
            shown.extend(rest.iter().map(|then| (place(first), place(then))));
        }
    }
    // Each rule is shown ahead of the next by a signal that breaks both.
    // Only a ticker that names nothing shows nothing: it leaves no
    // instrument for the policy or the stop to be asked about.
    for first in 0..order.len() - 1 {
        assert_eq!(
            shown.contains(&(first, first + 1)),
            order[first] != "ticker",
            "{} before {}",
            order[first],
            order[first + 1]
        );
    }
}

#[test]
fn each_refusal_says_what_to_change() {
    let fixture = fixture();
    for (id, said) in [
        (
            "wrong-profile",
            "ambiguous_selection: the alert names profile by-linear, and its URL selects profile \
             bn-linear; send an alert to the URL of the profile it names",
        ),
        (
            "wrong-exchange",
            "ambiguous_selection: the alert says its chart is of BYBIT, and profile bn-linear \
             trades on binance",
        ),
        (
            "wrong-category",
            "ambiguous_selection: the alert says it is for inverse contracts, and profile \
             by-linear trades linear contracts",
        ),
        (
            "entry-past-the-age-limit",
            "stale: the alert fired 60.001 s ago, and profile by-linear takes entries up to 60 s \
             old",
        ),
        (
            "past-a-longer-age-limit",
            "stale: the alert fired 300.001 s ago, and profile by-linear takes entries up to 300 \
             s old",
        ),
        (
            "entry-from-the-future",
            "future_timestamp: the alert is timed 5.001 s ahead of the clock, and at most 5 s is \
             allowed; check the server's clock",
        ),
        (
            "venue-symbol-without-settings",
            "unknown_ticker: BTCUSDT is the venue's own symbol of a contract profile bn-linear \
             has no settings for; send TradingView's ticker of the perpetual, which ends in .P, \
             or give the asset settings of its own",
        ),
        (
            "unknown-ticker",
            "unknown_ticker: bybit linear lists no contract that DOGEUSDT.P names; a ticker is \
             TradingView's name of a perpetual, such as SOLUSDT.P, or the venue's own symbol of \
             an asset profile by-linear has settings for",
        ),
        (
            "pair-named-twice",
            "ambiguous_selection: bybit linear lists 2 perpetuals of SOLUSDT: SOLUSDT, SOLUSDT2; \
             SOLUSDT.P cannot tell them apart",
        ),
        (
            "disabled-asset",
            "not_armed: 1000SHIBUSDT is disabled in profile bn-linear; enable the asset to take \
             entries on it",
        ),
        (
            "defaults-off",
            "unknown_ticker: profile bn-inverse has no settings for SOLUSD_PERP, and its defaults \
             are not enabled",
        ),
        (
            "usdc-perpetual",
            "outside_universe: ETHPERP: linear contracts settled in USDC are refused; only USDT \
             is",
        ),
        (
            "no-capital",
            "no_capital_in_settlement_asset: profile by-inverse has no capital in ETH, which \
             ETHUSD settles in",
        ),
        (
            "no-fallback-stop",
            "no_stop: the alert brings no stop, and profile bn-inverse gives BTCUSD_PERP no \
             fallback stop percentage",
        ),
    ] {
        let case = fixture
            .resolutions
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("the fixture has no resolution {id}"));
        let refusal = resolved_on_the_model(&fixture, case).expect_err("refused");
        assert_eq!(refusal.to_string(), said, "{id}");
    }
}

/// What a payload asks on its line when it fired at a time of its own:
/// the same body with that fire time, read and resolved like any delivery.
fn fired(fixture: &Fixture, fired: &Fired) -> IntentKind {
    let sent = fixture.payload(&fired.payload).signal.as_ref();
    let mut fields = sent.expect("a signal").fields.clone();
    fields.insert(
        "fire_time".to_string(),
        Value::from(fired.fire_time.as_str()),
    );
    let body = serde_json::to_vec(&fields).expect("a body");
    let signal =
        read_signal(&body).unwrap_or_else(|refused| panic!("{}: {refused:?}", fired.payload));
    assert_eq!(
        signal.fire_time_ms, fired.fire_time_ms,
        "{}",
        fired.fire_time
    );
    let line = fixture.line(&fired.line);
    let now_ms: TimestampMs = fired.fire_time_ms + 2_000;
    signal
        .resolve(
            &line.policy(),
            &line.listed(),
            now_ms,
            SignalAgeLimit::DEFAULT,
        )
        .unwrap_or_else(|refusal| panic!("{} on {}: {refusal}", fired.payload, fired.line))
        .intent
}

#[test]
fn an_exit_closes_only_a_position_its_own_profile_entered_before_it_fired() {
    let fixture = fixture();
    let mut closing = 0;
    for case in &fixture.exits {
        let IntentKind::Exit(exit) = fired(&fixture, &case.exit) else {
            panic!("{}: its exit is no exit", case.id);
        };
        // The position that is open, by the entry that opened it.
        let open: Vec<EntryIntent> = case
            .entry
            .iter()
            .map(|entry| match fired(&fixture, entry) {
                IntentKind::Entry(entry) => entry,
                IntentKind::Exit(_) => panic!("{}: its entry is no entry", case.id),
            })
            .collect();
        let closes = open.iter().any(|entry| exit.closes(entry));
        assert_eq!(closes, case.closes, "{}: {}", case.id, case.note);
        closing += usize::from(closes);
    }
    assert_eq!(closing, 3, "three of the cases close a position");
    assert!(fixture.exits.iter().any(|case| case.entry.is_none()));
}

#[test]
fn an_exit_is_held_to_each_part_of_the_position_it_closes() {
    // One entry, and an exit that differs from it in one thing at a time.
    let fixture = fixture();
    let case = &fixture.exits[0];
    let entry = case.entry.as_ref().expect("an entry");
    let IntentKind::Entry(entry) = fired(&fixture, entry) else {
        panic!("no entry");
    };
    let IntentKind::Exit(exit) = fired(&fixture, &case.exit) else {
        panic!("no exit");
    };
    assert!(exit.closes(&entry));
    let other = |change: &dyn Fn(&mut ExitIntent)| {
        let mut changed = exit.clone();
        change(&mut changed);
        changed.closes(&entry)
    };
    assert!(!other(&|exit| exit.side = exit.side.opposite()));
    assert!(!other(&|exit| exit.signal_time_ms = entry.signal_time_ms));
    assert!(!other(
        &|exit| exit.signal_time_ms = entry.signal_time_ms - 1
    ));
    assert!(other(&|exit| exit.signal_time_ms = entry.signal_time_ms + 1));
    assert!(!other(&|exit| {
        exit.profile = ProfileId::new("another").expect("a profile id");
    }));
    // Its own ID is not what ties an exit to a position.
    assert!(other(&|exit| {
        exit.signal = SignalId::new("another:exit").expect("a signal id");
    }));
    let IntentKind::Exit(elsewhere) = fired(&fixture, &fixture.exits[5].exit) else {
        panic!("no exit");
    };
    assert_ne!(elsewhere.key, entry.key);
    assert!(!elsewhere.closes(&entry));
}

#[test]
fn a_delivery_is_new_a_repeat_or_a_conflict_within_its_own_profile() {
    let fixture = fixture();
    for case in &fixture.deliveries {
        // What a store holds: for each profile and signal ID, the hash of
        // the first payload recorded.
        let mut recorded: BTreeMap<(ProfileId, SignalId), SignalHash> = BTreeMap::new();
        for (number, step) in case.steps.iter().enumerate() {
            let signal = fixture.signal(&step.payload);
            let route = fixture.line_of(&step.profile).profile().clone();
            assert_eq!(signal.profile, route, "{}: step {number}", case.id);
            let key = (route, signal.id.clone());
            let delivery = Delivery::of(recorded.get(&key), &signal.payload_hash);
            let named = match delivery {
                Delivery::New => "new",
                Delivery::Duplicate => "duplicate",
                Delivery::Conflict => "conflict",
            };
            assert_eq!(named, step.outcome, "{}: step {number}", case.id);
            assert_eq!(
                delivery.refusal(),
                (delivery == Delivery::Conflict).then_some(Refusal::Conflict)
            );
            // Only a new signal is recorded: a conflict changes nothing.
            if delivery == Delivery::New {
                recorded.insert(key, signal.payload_hash);
            }
        }
    }
}

#[test]
fn an_entry_on_a_position_already_held_is_refused_by_the_side_it_is_on() {
    let fixture = fixture();
    let side = |name: &str| match name {
        "long" => PositionSide::Long,
        "short" => PositionSide::Short,
        other => panic!("no side {other}"),
    };
    let entry_of = |payload: &str| {
        let line = fixture.line("bybit linear");
        let resolved = fixture
            .signal(payload)
            .resolve(
                &line.policy(),
                &line.listed(),
                1_792_455_302_000,
                SignalAgeLimit::DEFAULT,
            )
            .expect("resolved");
        match resolved.intent {
            IntentKind::Entry(entry) => entry,
            IntentKind::Exit(_) => panic!("no entry"),
        }
    };
    for case in &fixture.held {
        let entry = match case.entry.as_str() {
            "long" => entry_of("by-linear-long-entry"),
            _ => entry_of("by-linear-short-entry"),
        };
        assert_eq!(entry.side, side(&case.entry));
        let held = case.held.as_deref().map(side);
        let refused = entry
            .against(held)
            .err()
            .map(|refusal| refusal.refusal.as_str());
        assert_eq!(refused, case.refused.as_deref(), "{}", case.note);
    }
    let long = entry_of("by-linear-long-entry");
    assert_eq!(
        long.against(Some(PositionSide::Long))
            .expect_err("pyramiding")
            .to_string(),
        "pyramiding: profile by-linear already holds a long position on SOLUSDT; a second entry \
         on its side would add to it"
    );
    assert_eq!(
        long.against(Some(PositionSide::Short))
            .expect_err("reversal")
            .to_string(),
        "reversal: profile by-linear holds a short position on SOLUSDT; a long entry would \
         reverse it, so send the exit first"
    );
}
