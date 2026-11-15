//! Operations: what one may name, and how a replacement moves.

use super::*;
use crate::model::VenueOrderId;
use crate::model::execution::{OrderRef, SpotReplaceState};
use crate::model::futures::{OwnerTag, ProfileIdentity};
use crate::model::{
    AccountRef, Asset, Category, Environment, ExchangeId, Symbol, Venue, VenueAccountId,
};

fn account() -> VenueAccountId {
    VenueAccountId::issued("71002233").expect("an account id")
}

fn profile_scope() -> ExecutionScope {
    let identity = ProfileIdentity::new(
        ProfileId::new("main").expect("a profile id"),
        AccountRef::new(Venue::Bybit, Environment::Demo, account()).expect("an account"),
        Category::Linear,
        OwnerTag::new("k3x9p2").expect("an owner tag"),
    )
    .expect("an identity");
    ExecutionScope::of_profile(&identity)
}

fn spot_scope() -> ExecutionScope {
    ExecutionScope::of_account(ExchangeId::BybitSpot, Environment::Demo, account())
}

fn instrument(exchange: ExchangeId, symbol: &str) -> InstrumentId {
    InstrumentId::new(
        exchange,
        Symbol::new(symbol).expect("a symbol"),
        Asset::new("USDT").expect("an asset"),
    )
}

fn order(client: &str) -> OrderRef {
    OrderRef::new(Some(client.to_string()), None).expect("an order reference")
}

fn replacement() -> NewOperation {
    NewOperation {
        scope: spot_scope(),
        kind: OperationKind::SpotReplace,
        key: Some(OperationKey::new("cmd/4f9a").expect("a key")),
        instrument: instrument(ExchangeId::BybitSpot, "BTCUSDT"),
        signal: None,
        cycle: None,
        replacement: Some(Replacement::of(order("qf-old-1"))),
    }
}

fn id(value: u64) -> OperationId {
    OperationId::new(value).expect("an operation id")
}

#[test]
fn a_spot_replacement_needs_no_signal_and_no_position() {
    let record = replacement().numbered(id(7)).expect("a record");
    assert_eq!(record.signal(), None);
    assert_eq!(record.cycle(), None);
    assert_eq!(record.scope().profile(), None);
    let stored = record.to_stored().expect("stored");
    assert_eq!(OperationRecord::from_stored(&stored).expect("read"), record);
    assert!(stored.starts_with(r#"{"format":1,"id":7,"scope":{"venue":"Bybit","category":"Spot""#));

    // It never names a position cycle, and nothing but a replacement names
    // an order it replaces.
    let with_cycle = NewOperation {
        cycle: Some(PositionCycleId::new(3).expect("a cycle")),
        ..replacement()
    };
    assert!(with_cycle.numbered(id(7)).is_err());
    let unreplacing = NewOperation {
        replacement: None,
        ..replacement()
    };
    assert!(unreplacing.numbered(id(7)).is_err());
    let entry = NewOperation {
        kind: OperationKind::Entry,
        ..replacement()
    };
    assert!(entry.numbered(id(7)).is_err(), "an entry on spot");
}

#[test]
fn a_signals_operation_names_its_signal_its_profile_and_later_its_cycle() {
    let signal = SignalId::new("trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z").expect("an id");
    let profile = ProfileId::new("main").expect("a profile id");
    let entry = NewOperation {
        scope: profile_scope(),
        kind: OperationKind::Entry,
        key: Some(OperationKey::of_signal(&profile, &signal, 1)),
        instrument: instrument(ExchangeId::BybitLinear, "SOLUSDT"),
        signal: Some(signal.clone()),
        cycle: None,
        replacement: None,
    };
    let record = entry.clone().numbered(id(9)).expect("a record");
    assert_eq!(
        record.key().map(OperationKey::as_str),
        Some("sig/main/1/trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z")
    );
    let cycle = PositionCycleId::new(9).expect("a cycle");
    let bound = record.in_cycle(cycle).expect("bound");
    assert_eq!(bound.cycle(), Some(cycle));
    assert!(bound.in_cycle(cycle).is_ok(), "the same cycle again");
    let other = PositionCycleId::new(10).expect("a cycle");
    assert!(bound.in_cycle(other).is_err(), "a cycle is named once");

    // Another product line's instrument, and a signal without a profile.
    let elsewhere = NewOperation {
        instrument: instrument(ExchangeId::BybitInverse, "SOLUSD"),
        ..entry.clone()
    };
    assert!(elsewhere.numbered(id(9)).is_err());
    let unowned = NewOperation {
        scope: ExecutionScope::of_account(ExchangeId::BybitLinear, Environment::Demo, account()),
        ..entry
    };
    assert!(unowned.numbered(id(9)).is_err());
}

#[test]
fn a_replacement_moves_only_along_its_state_machine_and_keeps_what_it_replaces() {
    let record = replacement().numbered(id(7)).expect("a record");
    let pending = record.replacement().expect("a replacement").clone();
    assert_eq!(pending.state(), SpotReplaceState::Pending);

    let named = pending.replaced_by(order("qf-new-1"));
    let unknown = named
        .moved_to(SpotReplaceState::OutcomeUnknown)
        .expect("unknown");
    let settled = unknown
        .moved_to(SpotReplaceState::PartiallyApplied)
        .expect("settled");
    assert!(settled.moved_to(SpotReplaceState::Accepted).is_err());
    assert!(unknown.moved_to(SpotReplaceState::Pending).is_err());

    let moved = record.with_replacement(unknown.clone()).expect("moved");
    assert!(moved.with_replacement(settled).is_ok());
    // Back to pending, another original, or another new order: refused.
    assert!(moved.with_replacement(named.clone()).is_err());
    let other_original = Replacement::of(order("qf-old-2"));
    assert!(record.with_replacement(other_original).is_err());
    let renamed = unknown.replaced_by(order("qf-new-2"));
    assert!(moved.with_replacement(renamed).is_err());
    // The venue's ID for the new order is learned later, and is kept.
    let venue_id = VenueOrderId::new("8389765519").expect("an id");
    let known = unknown.replaced_by(
        OrderRef::new(Some("qf-new-1".to_string()), Some(venue_id)).expect("a reference"),
    );
    assert!(moved.with_replacement(known).is_ok());
}

#[test]
fn every_spot_replace_state_but_the_terminal_ones_has_a_way_out() {
    for state in SpotReplaceState::ALL {
        let leaves = SpotReplaceState::TRANSITIONS
            .iter()
            .any(|(from, _)| *from == state);
        assert_eq!(leaves, !state.is_terminal(), "{state}");
    }
    assert!(OrderRef::new(None, None).is_err(), "it names nothing");
    assert!(OrderRef::new(Some("has space".to_string()), None).is_err());
    let kinds: Vec<&str> = OperationKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        ["entry", "close", "stop", "leverage", "spot_replace"]
    );
}
