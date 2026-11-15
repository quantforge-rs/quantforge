//! The inbox's records: judged once, held to their payload.

use super::*;
use crate::model::futures::{OwnerTag, ProfileIdentity, SignalAction};
use crate::model::{
    AccountRef, Category, Environment, PositionSide, Price, Refusal, Venue, VenueAccountId,
};
use rust_decimal::Decimal;

const PAYLOAD: &str = r#"{"version":1,"kind":"entry"}"#;

fn scope() -> ExecutionScope {
    let account = VenueAccountId::issued("71002233").expect("an account id");
    let identity = ProfileIdentity::new(
        ProfileId::new("main").expect("a profile id"),
        AccountRef::new(Venue::Bybit, Environment::Demo, account).expect("an account"),
        Category::Linear,
        OwnerTag::new("k3x9p2").expect("an owner tag"),
    )
    .expect("an identity");
    ExecutionScope::of_profile(&identity)
}

fn signal(payload: &str) -> ExternalSignal {
    ExternalSignal {
        id: SignalId::new("trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z").expect("an id"),
        payload_hash: SignalHash::of(payload.as_bytes()),
        profile: ProfileId::new("main").expect("a profile id"),
        ticker: "SOLUSDT.P".to_string(),
        exchange: Some("BYBIT".to_string()),
        category: Some(Category::Linear),
        side: PositionSide::Long,
        action: SignalAction::Entry {
            expected_price: Price::new(Decimal::new(23423, 2)).expect("a price"),
            stop: None,
        },
        bar_time_ms: 1_792_454_400_000,
        fire_time_ms: 1_792_455_300_000,
    }
}

fn received() -> SignalRecord {
    SignalRecord::received(scope(), signal(PAYLOAD), PAYLOAD, 1_792_455_301_000).expect("a record")
}

#[test]
fn a_signal_is_judged_once_and_its_record_never_changes_after() {
    let record = received();
    assert_eq!(record.state(), SignalState::Received);
    let admitted = record.admitted(5).expect("admitted");
    assert_eq!(admitted.judged_at_ms(), Some(5));
    assert!(admitted.follows(&record).is_ok());
    // Judged, it is judged for good.
    assert!(admitted.admitted(6).is_err());
    assert!(admitted.noop("nothing", 6).is_err());
    let refusal = SignalRefusal {
        refusal: Refusal::Stale,
        message: "the alert fired 75 s ago".to_string(),
    };
    let rejected = record.rejected(refusal, 7).expect("rejected");
    assert!(rejected.follows(&admitted).is_err(), "a second judgment");
    assert!(
        record.follows(&record).is_err(),
        "received again is no move"
    );
}

#[test]
fn a_record_keeps_the_payload_its_hash_names_and_refuses_any_other() {
    let record = received();
    let stored = record.to_stored().expect("stored");
    assert_eq!(SignalRecord::from_stored(&stored).expect("read"), record);
    assert!(stored.starts_with(r#"{"format":1,"scope":{"venue":"Bybit""#));
    let other = stored.replace(r#"\"kind\":\"entry\""#, r#"\"kind\":\"exit\""#);
    assert_ne!(other, stored);
    let refused = SignalRecord::from_stored(&other).expect_err("another payload");
    assert!(refused.to_string().contains("not the one its hash names"));
    let newer = stored.replacen(r#""format":1"#, r#""format":2"#, 1);
    assert_eq!(
        SignalRecord::from_stored(&newer)
            .expect_err("format 2")
            .to_string(),
        "signal record format 2 is not supported; this build reads format 1"
    );
}

#[test]
fn a_stored_record_is_rejected_with_a_refusal_and_judged_with_a_time() {
    let stored = received()
        .admitted(5)
        .expect("admitted")
        .to_stored()
        .expect("stored");
    let unexplained = stored.replace(r#""state":"Admitted""#, r#""state":"Rejected""#);
    assert!(SignalRecord::from_stored(&unexplained).is_err());
    let untimed = stored.replace(r#""judged_at_ms":5"#, r#""judged_at_ms":null"#);
    assert!(SignalRecord::from_stored(&untimed).is_err());
}

#[test]
fn a_delivery_is_the_same_signal_again_or_a_conflict_by_its_hash() {
    let record = received();
    assert_eq!(
        record.delivery_of(&SignalHash::of(PAYLOAD.as_bytes())),
        Delivery::Duplicate
    );
    let other = r#"{"version":1,"kind":"exit"}"#;
    assert_eq!(
        record.delivery_of(&SignalHash::of(other.as_bytes())),
        Delivery::Conflict
    );
    let arrived = SignalRecord::received(scope(), signal(other), other, 9).expect("a record");
    let noted = SignalDelivery::of(&arrived, Delivery::Conflict);
    assert_eq!(noted.payload.as_deref(), Some(other));
    assert_eq!(
        SignalDelivery::of(&arrived, Delivery::Duplicate).payload,
        None
    );
    // A record with no profile is no inbox's.
    let account = VenueAccountId::issued("71002233").expect("an account id");
    let unowned = ExecutionScope::of_account(
        crate::model::ExchangeId::BybitLinear,
        Environment::Demo,
        account,
    );
    assert!(SignalRecord::received(unowned, signal(PAYLOAD), PAYLOAD, 1).is_err());
}
