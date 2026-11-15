//! A pool's total: exact, or none.

use super::*;
use crate::model::futures::{OwnerTag, ProfileId, ProfileIdentity};
use crate::model::{
    AccountRef, Asset, Category, Environment, ExchangeId, Symbol, Venue, VenueAccountId,
};

fn usdt() -> Asset {
    Asset::new("USDT").expect("an asset")
}

fn held(operation: u64, margin: &str) -> Reservation {
    let account = VenueAccountId::issued("71002233").expect("an account id");
    let identity = ProfileIdentity::new(
        ProfileId::new("main").expect("a profile id"),
        AccountRef::new(Venue::Bybit, Environment::Demo, account).expect("an account"),
        Category::Linear,
        OwnerTag::new("k3x9p2").expect("an owner tag"),
    )
    .expect("an identity");
    let instrument = InstrumentId::new(
        ExchangeId::BybitLinear,
        Symbol::new("SOLUSDT").expect("a symbol"),
        usdt(),
    );
    let amount = |text: &str| Money::new(Decimal::from_str_exact(text).expect("a decimal"), usdt());
    Reservation::held(
        ExecutionScope::of_profile(&identity),
        OperationId::new(operation).expect("an operation id"),
        instrument,
        PoolScope::Asset(usdt()),
        amount(margin),
        amount("149.9274"),
        1_000,
    )
    .expect("a reservation")
}

fn total(margins: &[&str]) -> Result<String, ModelError> {
    let held = margins
        .iter()
        .zip(1..)
        .map(|(margin, operation)| held(operation, margin))
        .collect();
    let pool = PoolReservations { revision: 0, held };
    pool.total(&PoolScope::Asset(usdt()))
        .map(|total| total.amount().to_string())
}

#[test]
fn a_total_keeps_the_places_of_its_longest_amount() {
    assert_eq!(total(&[]).expect("nothing held"), "0");
    assert_eq!(total(&["205.18548"]).expect("one"), "205.18548");
    assert_eq!(total(&["205.18548", "410.1"]).expect("two"), "615.28548");
    assert_eq!(total(&["100.00", "0.5"]).expect("two"), "100.50");
    // 29 significant digits, which a sum kept to 28 would round.
    assert_eq!(
        total(&["1.0000000000000000000000000001", "0.00150000"]).expect("two"),
        "1.0015000000000000000000000001"
    );
}

#[test]
fn a_total_no_decimal_holds_exactly_is_refused_and_never_rounded() {
    // The largest amount with one place: 2^96 - 1 tenths.
    let most = "7922816251426433759354395033.5";
    // With one place the total does not fit; without its zero it does.
    assert_eq!(
        total(&[most, "0.5"]).expect("exact without its zero"),
        "7922816251426433759354395034"
    );
    // Every trailing zero goes then, not only as many as would make it
    // fit: with two places, the largest amount and 65 hundredths.
    assert_eq!(
        total(&["792281625142643375935439503.35", "0.65"]).expect("exact"),
        "792281625142643375935439504"
    );
    // A `Decimal`'s own addition rounds this one to ...034 and says
    // nothing: the tenth it drops is not a zero.
    let rounded = Decimal::from_str_exact(most)
        .expect("a decimal")
        .checked_add(Decimal::from_str_exact("0.6").expect("a decimal"));
    assert_eq!(
        rounded.map(|sum| sum.to_string()).as_deref(),
        Some("7922816251426433759354395034")
    );
    let refused = total(&[most, "0.6"]).expect_err("no decimal holds it");
    assert!(
        refused
            .to_string()
            .contains("an amount a decimal holds exactly"),
        "{refused}"
    );
    // And one past every decimal is refused too.
    assert!(total(&["79228162514264337593543950335", "1"]).is_err());
}

#[test]
fn a_pool_of_many_reservations_adds_up_like_a_pool_of_few() {
    let many = vec!["0.00150000"; 1_000];
    assert_eq!(total(&many).expect("a thousand"), "1.50000000");
    // Five thousand amounts of eight places, one of two and one of
    // twenty-eight: 29 significant digits, which a decimal still holds.
    let mut mixed = vec!["0.00100000"; 5_000];
    mixed.push("0.25");
    mixed.push("0.0000000000000000000000000001");
    assert_eq!(
        total(&mixed).expect("five thousand and two"),
        "5.2500000000000000000000000001"
    );
}

#[test]
fn a_total_is_of_one_unit() {
    let pool = PoolReservations {
        revision: 0,
        held: vec![held(1, "205.18548")],
    };
    let other = PoolScope::Asset(Asset::new("BTC").expect("an asset"));
    assert!(matches!(
        pool.total(&other),
        Err(ModelError::AssetMismatch { .. })
    ));
}
