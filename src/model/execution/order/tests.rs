//! Orders as the ledger keeps them: what a venue's report may replace,
//! and what a record refuses to hold.

use super::*;
use crate::model::futures::{
    FuturesOrderStatus, OrderOrigin, OwnerTag, PriceProtection, ProfileId, ProfileIdentity,
    RecordSource, StopRef, StopScope, TriggerBasis,
};
use crate::model::{
    AccountRef, Asset, Category, Environment, ExchangeId, PositionSide, Price, Quantity, Side,
    Symbol, Venue, VenueAccountId, VenueOrderId,
};
use rust_decimal::Decimal;

const CLIENT: &str = "qf1-k3x9p2-1-1-e1";

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

fn instrument(symbol: &str) -> InstrumentId {
    InstrumentId::new(
        ExchangeId::BybitLinear,
        Symbol::new(symbol).expect("a symbol"),
        Asset::new("USDT").expect("an asset"),
    )
}

fn recorded() -> OrderRecord {
    let operation = OperationId::new(1).expect("an operation id");
    OrderRecord::recorded(scope(), operation, CLIENT, instrument("SOLUSDT"), 1_000)
        .expect("an order record")
}

fn quantity(text: &str) -> Quantity {
    let amount = Decimal::from_str_exact(text).expect("a decimal");
    Quantity::for_category(Category::Linear, amount).expect("a quantity")
}

/// The order as its venue reports it: `filled` of the 2 it was for.
fn report(venue_order_id: &str, status: FuturesOrderStatus, filled: &str) -> OrderEvidence {
    OrderEvidence::Futures(Box::new(FuturesOrder {
        instrument: instrument("SOLUSDT"),
        venue_order_id: VenueOrderId::new(venue_order_id).expect("an order id"),
        client_id: Some(CLIENT.to_string()),
        side: Side::Buy,
        reduce_only: false,
        origin: OrderOrigin::Client,
        status,
        requested: quantity("2"),
        filled: quantity(filled),
        average_price: None,
        source: RecordSource::Query,
        updated_at_ms: 2_000,
    }))
}

fn stop(symbol: &str, status: StopStatus) -> OrderEvidence {
    OrderEvidence::Stop(Box::new(StopOrder {
        stop: StopRef::Position {
            instrument: instrument(symbol),
        },
        instrument: instrument(symbol),
        position_side: PositionSide::Long,
        trigger_price: Price::new(Decimal::from(200)).expect("a price"),
        basis: Some(TriggerBasis::Mark),
        price_protection: PriceProtection::NotApplicable,
        scope: StopScope::WholePosition,
        status,
        triggered_order: None,
        updated_at_ms: 2_000,
    }))
}

#[test]
fn a_final_report_is_not_replaced_by_one_that_says_the_order_still_works() {
    let working = report("8389765519", FuturesOrderStatus::PartiallyFilled, "1");
    let done = report("8389765519", FuturesOrderStatus::Filled, "2");
    let partly = recorded().reported(working.clone()).expect("a report");
    let filled = partly.reported(done.clone()).expect("the final report");
    assert!(filled.evidence().is_some_and(OrderEvidence::is_final));

    // An older report arriving late takes back what the final one said.
    let late = filled.reported(working).expect_err("the order is done");
    assert!(late.to_string().contains("takes back"), "{late}");
    // The same final report again, and a working one after a working one,
    // take nothing back.
    assert_eq!(filled.reported(done).expect("the same again"), filled);
    let more = report("8389765519", FuturesOrderStatus::PartiallyFilled, "1.5");
    assert!(partly.reported(more).is_ok());

    // A stop that has ended is final too.
    let active = recorded().reported(stop("SOLUSDT", StopStatus::Active));
    let active = active.expect("a stop's report");
    let ended = active
        .reported(stop("SOLUSDT", StopStatus::Finished))
        .expect("the stop ended");
    assert!(
        ended
            .reported(stop("SOLUSDT", StopStatus::Triggered))
            .is_err()
    );
}

#[test]
fn a_record_holds_only_a_report_of_its_own_order() {
    // The venue's ID a record names is the one its report names.
    let reported = recorded()
        .reported(report("8389765519", FuturesOrderStatus::Filled, "2"))
        .expect("a report");
    assert_eq!(reported.venue_order_id(), Some("8389765519"));
    let stored = reported.to_stored().expect("stored");
    assert_eq!(OrderRecord::from_stored(&stored).expect("read"), reported);
    let named = r#""venue_order_id":"8389765519","outcome_unknown""#;
    assert_eq!(stored.matches(named).count(), 1, "{stored}");
    let another = stored.replace(named, r#""venue_order_id":"8389765520","outcome_unknown""#);
    let refused = OrderRecord::from_stored(&another).expect_err("another order's report");
    assert!(
        refused
            .to_string()
            .contains("is not the one the record is of"),
        "{refused}"
    );
    let unnamed = stored.replace(named, r#""venue_order_id":null,"outcome_unknown""#);
    assert!(OrderRecord::from_stored(&unnamed).is_err());

    // An order acknowledged under one ID is not reported under another.
    let acknowledged = recorded().acknowledged("8389765519").expect("acknowledged");
    let other = report("8389765520", FuturesOrderStatus::Filled, "2");
    assert!(acknowledged.reported(other).is_err());

    // A stop's report is of the record's instrument.
    assert!(
        recorded()
            .reported(stop("SOLUSDT", StopStatus::Active))
            .is_ok()
    );
    let elsewhere = recorded().reported(stop("BTCUSDT", StopStatus::Active));
    let refused = elsewhere.expect_err("another instrument's stop");
    assert!(
        refused
            .to_string()
            .contains("is not the one the record is of"),
        "{refused}"
    );
}
