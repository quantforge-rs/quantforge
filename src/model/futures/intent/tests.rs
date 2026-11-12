//! An intent's admission and its stored form: an entry is admitted under
//! its own policy and keeps it from then on, an exit under none; and a
//! stored record's format and its attempts' legs.

use super::*;
use crate::model::futures::{ActivationEpoch, OwnerTag};
use crate::model::{
    AccountRef, Asset, Environment, ExchangeId, OperationId, Symbol, Venue, VenueAccountId,
};

/// What profile `main` holds SOLUSDT to on its venue's linear line, as an
/// admitted entry stores it.
const POLICY: &str = r#"{"format":1,"profile":"main","account":{"venue":"Bybit","environment":"Demo","venue_account_id":{"Venue":"1"}},"revision":4,"instrument":{"exchange":"BybitLinear","symbol":"SOLUSDT","settle":"USDT"},"risk_percent":{"value":"0.5","source":"Asset"},"stop_percent":{"value":null,"source":"Asset"},"leverage":{"value":"5","source":"Defaults"},"capital":{"amount":"15000","asset":"USDT"},"capital_revision":4}"#;

fn policy() -> PolicySnapshot {
    serde_json::from_str(POLICY).expect("a snapshot")
}

fn key(symbol: &str) -> PositionKey {
    key_of("1", symbol)
}

/// `symbol`'s position on account `account`.
fn key_of(account: &str, symbol: &str) -> PositionKey {
    let id = VenueAccountId::issued(account).expect("account id");
    let account = AccountRef::new(Venue::Bybit, Environment::Demo, id).expect("account");
    let symbol = Symbol::new(symbol).expect("symbol");
    let usdt = Asset::new("USDT").expect("asset");
    let instrument = InstrumentId::new(ExchangeId::BybitLinear, symbol, usdt);
    PositionKey::new(account, instrument).expect("key")
}

fn entry(profile: &str, symbol: &str) -> ExternalIntent {
    ExternalIntent::new(IntentKind::Entry(EntryIntent {
        profile: ProfileId::new(profile).expect("profile"),
        signal: SignalId::new("entry").expect("signal"),
        signal_time_ms: 0,
        key: key(symbol),
        side: PositionSide::Long,
        expected_price: Price::new(rust_decimal::Decimal::ONE_HUNDRED).expect("price"),
        stop: None,
    }))
}

fn exit() -> ExternalIntent {
    ExternalIntent::new(IntentKind::Exit(ExitIntent {
        profile: ProfileId::new("main").expect("profile"),
        signal: SignalId::new("exit").expect("signal"),
        signal_time_ms: 0,
        key: key("BTCUSDT"),
        side: PositionSide::Short,
    }))
}

#[test]
fn an_entry_is_admitted_under_its_own_policy_once_and_an_exit_under_none() {
    let mut own = entry("main", "SOLUSDT");
    let refused = own.admit(None).expect_err("no entry without its policy");
    assert_eq!(
        refused.to_string(),
        "intent reaches admitted only on a snapshot of its own profile's policy for its \
         account and instrument, for an entry, and none for an exit"
    );
    let bare = own.move_to(IntentState::Admitted).expect_err("a bare move");
    assert_eq!(
        bare.to_string(),
        "intent reaches admitted only on its admission, with an entry's policy snapshot"
    );
    let mut theirs = entry("other", "SOLUSDT");
    assert!(theirs.admit(Some(policy())).is_err(), "another profile's");
    let mut elsewhere = entry("main", "BTCUSDT");
    assert!(
        elsewhere.admit(Some(policy())).is_err(),
        "another instrument's"
    );
    // The profile's own instrument, on an account that is not the profile's.
    let mut another = entry("main", "SOLUSDT");
    let IntentKind::Entry(asked) = &mut another.kind else {
        panic!("an entry");
    };
    asked.key = key_of("2", "SOLUSDT");
    assert!(another.admit(Some(policy())).is_err(), "another account's");
    // Refused, an intent is as it was.
    for intent in [&own, &theirs, &elsewhere, &another] {
        assert_eq!(
            (intent.state(), intent.policy()),
            (IntentState::Received, None)
        );
    }

    own.admit(Some(policy())).expect("admitted");
    assert_eq!(own.state(), IntentState::Admitted);
    assert_eq!(own.policy(), Some(&policy()));
    // Admitted once: the policy it took is never taken again.
    assert!(own.admit(Some(policy())).is_err());

    let mut closing = exit();
    assert!(closing.admit(Some(policy())).is_err(), "an exit takes none");
    assert!(
        closing.move_to(IntentState::Admitted).is_err(),
        "a bare move"
    );
    closing.admit(None).expect("admitted");
    assert_eq!(
        (closing.state(), closing.policy()),
        (IntentState::Admitted, None)
    );
}

#[test]
fn a_stored_entry_holds_its_policy_from_admission_on_and_nothing_else_holds_one() {
    let mut admitted = entry("main", "SOLUSDT");
    admitted.admit(Some(policy())).expect("admitted");
    // The record holds the snapshot whole, in the snapshot's own spelling.
    let stored = serde_json::to_string(&admitted).expect("json");
    assert!(
        stored.contains(&format!(r#""policy":{POLICY},"#)),
        "{stored}"
    );
    let with = serde_json::to_value(&admitted).expect("json");
    let back: ExternalIntent = serde_json::from_value(with.clone()).expect("round trip");
    assert_eq!(back, admitted);
    let mut without = with.clone();
    without["policy"] = serde_json::Value::Null;
    let reads = |record: &serde_json::Value, state: &str| {
        let mut record = record.clone();
        record["state"] = serde_json::json!(state);
        serde_json::from_value::<ExternalIntent>(record).is_ok()
    };
    // Before admission none, from admission on always; an entry refused or
    // dropped may have stood either side of it.
    for (state, held, bare) in [
        ("Received", false, true),
        ("Duplicate", false, true),
        ("Rejected", true, true),
        ("Noop", true, true),
        ("Admitted", true, false),
        ("Expired", true, false),
    ] {
        assert_eq!(reads(&with, state), held, "{state} with a policy");
        assert_eq!(reads(&without, state), bare, "{state} without one");
    }
    // Never another profile's, another account's or another instrument's,
    // and never an exit's.
    let mut theirs = with.clone();
    theirs["policy"]["profile"] = serde_json::json!("other");
    let mut another = with.clone();
    another["policy"]["account"]["venue_account_id"]["Venue"] = serde_json::json!("2");
    let mut elsewhere = with.clone();
    elsewhere["policy"]["instrument"]["symbol"] = serde_json::json!("BTCUSDT");
    let mut closing = serde_json::to_value(exit()).expect("json");
    assert!(reads(&closing, "Admitted"));
    closing["policy"] = with["policy"].clone();
    for state in ["Admitted", "Rejected"] {
        assert!(!reads(&theirs, state), "{state}: another profile's");
        assert!(!reads(&another, state), "{state}: another account's");
        assert!(!reads(&elsewhere, state), "{state}: another instrument's");
        assert!(!reads(&closing, state), "{state}: an exit's");
    }
}

#[test]
fn a_stored_intent_of_another_format_is_refused_never_guessed_at() {
    let mut json = serde_json::to_value(exit()).expect("json");
    assert_eq!(json["format"], INTENT_FORMAT);
    let back: ExternalIntent = serde_json::from_value(json.clone()).expect("round trip");
    assert_eq!(back, exit());
    json["format"] = serde_json::json!(INTENT_FORMAT + 1);
    let error = serde_json::from_value::<ExternalIntent>(json).expect_err("format");
    assert!(error.to_string().contains("intent format 2"), "{error}");
}

#[test]
fn a_stored_attempt_on_the_wrong_leg_is_refused() {
    let mut json = serde_json::to_value(exit()).expect("json");
    let entry_leg = ClientOrderId::new(
        OwnerTag::new("k3x9p2").expect("tag"),
        ActivationEpoch(1),
        OperationId::new(1).expect("operation"),
        OrderLeg::Entry,
        1,
    )
    .expect("client order id");
    json["attempts"] = serde_json::json!([{"client_order_id": entry_leg, "sent_at_ms": 0}]);
    assert!(serde_json::from_value::<ExternalIntent>(json).is_err());
}
