//! The policy's units, its stored form, and the rules a document keeps.
//! What each change does, on every product line, is held to an independent
//! model in `tests/policy/`.

use super::*;
use crate::model::futures::{
    ContractKind, FuturesInstrument, InstrumentSpec, LeverageFilter, OwnerTag, ProfileId,
    ProfileIdentity, TradingStatus,
};
use crate::model::{
    AccountRef, Asset, Category, Environment, InstrumentId, Leverage, Money, Price, Quantity,
    Refusal, Symbol, Venue, VenueAccountId,
};
use serde_json::{Value, json};
use std::str::FromStr;

mod changes;
mod entries;

/// A new profile's policy, as stored.
const NEW: &str = r#"{"format":1,"identity":{"id":"main","account_ref":{"venue":"Bybit","environment":"Demo","venue_account_id":{"Venue":"12345"}},"category":"Linear","owner_tag":"k3x9p2"},"revision":1,"label":null,"defaults":{"enabled":false,"risk_percent":null,"stop_percent":null,"leverage":null,"revision":1},"assets":[],"capital":[]}"#;

/// [`configured`], as stored.
const CONFIGURED: &str = r#"{"format":1,"identity":{"id":"main","account_ref":{"venue":"Bybit","environment":"Demo","venue_account_id":{"Venue":"12345"}},"category":"Linear","owner_tag":"k3x9p2"},"revision":4,"label":null,"defaults":{"enabled":true,"risk_percent":"1","stop_percent":"3.50","leverage":"5","revision":2},"assets":[{"instrument":{"exchange":"BybitLinear","symbol":"SOLUSDT","settle":"USDT"},"enabled":true,"risk_percent":"0.5","stop":"Off","leverage":null,"revision":3}],"capital":[{"amount":{"amount":"15000","asset":"USDT"},"revision":4}]}"#;

/// The entry [`configured`] admits on SOLUSDT, as stored.
const SNAPSHOT: &str = r#"{"format":1,"profile":"main","account":{"venue":"Bybit","environment":"Demo","venue_account_id":{"Venue":"12345"}},"revision":4,"instrument":{"exchange":"BybitLinear","symbol":"SOLUSDT","settle":"USDT"},"risk_percent":{"value":"0.5","source":"Asset"},"stop_percent":{"value":null,"source":"Asset"},"leverage":{"value":"5","source":"Defaults"},"capital":{"amount":"15000","asset":"USDT"},"capital_revision":4}"#;

/// The greatest revision.
const LAST: u64 = 9_223_372_036_854_775_807;

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).expect("a decimal")
}

fn risk(text: &str) -> RiskPercent {
    RiskPercent::new(decimal(text)).expect("a risk percentage")
}

fn stop(text: &str) -> StopPercent {
    StopPercent::new(decimal(text)).expect("a stop percentage")
}

fn leverage(text: &str) -> Leverage {
    Leverage::new(decimal(text)).expect("a leverage")
}

fn money(amount: &str, asset: &str) -> Money {
    Money::new(decimal(amount), Asset::new(asset).expect("an asset"))
}

/// Profile `main` on Bybit's linear line, on demo.
fn identity() -> ProfileIdentity {
    let account = VenueAccountId::issued("12345").expect("an account id");
    ProfileIdentity::new(
        ProfileId::new("main").expect("a profile id"),
        AccountRef::new(Venue::Bybit, Environment::Demo, account).expect("an account"),
        Category::Linear,
        OwnerTag::new("k3x9p2").expect("an owner tag"),
    )
    .expect("an identity")
}

fn instrument(exchange: ExchangeId, symbol: &str, settle: &str) -> InstrumentId {
    InstrumentId::new(
        exchange,
        Symbol::new(symbol).expect("a symbol"),
        Asset::new(settle).expect("an asset"),
    )
}

/// `symbol` as Bybit lists it on its linear line: a contract of `kind`
/// settled in `settle`, taking a leverage from 1 to 50 in steps of 0.5.
fn listed(symbol: &str, settle: &str, kind: ContractKind) -> FuturesInstrument {
    let quantity =
        |text: &str| Quantity::for_category(Category::Linear, decimal(text)).expect("a quantity");
    FuturesInstrument::try_from(InstrumentSpec {
        id: instrument(ExchangeId::BybitLinear, symbol, settle),
        base: Asset::new("SOL").expect("an asset"),
        quote: Asset::new("USDT").expect("an asset"),
        contract: kind,
        status: TradingStatus::Trading,
        delist_at_ms: None,
        inverse: None,
        tick: Price::new(decimal("0.01")).expect("a tick"),
        step: quantity("0.1"),
        market_step: quantity("0.1"),
        min_qty: quantity("0.1"),
        market_min_qty: quantity("0.1"),
        market_max_qty: quantity("12000"),
        min_notional: Some(money("5", "USDT")),
        leverage: Some(LeverageFilter {
            min: leverage("1"),
            max: leverage("50"),
            step: decimal("0.5"),
        }),
        observed_at_ms: 0,
    })
    .expect("an instrument")
}

fn sol() -> FuturesInstrument {
    listed("SOLUSDT", "USDT", ContractKind::Perpetual)
}

/// `policy` after `change`, which must change it.
fn after(policy: &ProfilePolicy, change: PolicyChange) -> ProfilePolicy {
    policy
        .apply(&change)
        .expect("a change the policy takes")
        .expect("a change that changes it")
}

/// Why `policy` refuses `change`.
fn refused(policy: &ProfilePolicy, change: PolicyChange) -> String {
    policy.apply(&change).expect_err("refused").to_string()
}

fn defaults(edit: DefaultsEdit) -> PolicyChange {
    PolicyChange::Defaults(edit)
}

fn asset(symbol: &str, enabled: bool, risk_percent: Option<&str>) -> PolicyChange {
    PolicyChange::AddAsset(NewAsset {
        instrument: instrument(ExchangeId::BybitLinear, symbol, "USDT"),
        enabled,
        risk_percent: risk_percent.map(risk),
        stop: StopFallback::Off,
        leverage: None,
    })
}

fn edit(symbol: &str, edit: AssetEdit) -> PolicyChange {
    let symbol = Symbol::new(symbol).expect("a symbol");
    PolicyChange::EditAsset { symbol, edit }
}

/// Defaults of 1 % risk, a 3.50 % stop and 5x, enabled at revision 2;
/// SOLUSDT at half a percent with no fallback stop at 3; and 15,000 USDT
/// at 4.
fn configured() -> ProfilePolicy {
    let policy = after(
        &ProfilePolicy::new(identity()),
        defaults(DefaultsEdit {
            enabled: Some(true),
            risk_percent: Edit::Set(risk("1")),
            stop_percent: Edit::Set(stop("3.50")),
            leverage: Edit::Set(leverage("5")),
        }),
    );
    let policy = after(&policy, asset("SOLUSDT", true, Some("0.5")));
    after(&policy, PolicyChange::SetCapital(money("15000", "USDT")))
}

#[test]
fn a_percentage_keeps_its_bounds_and_its_exact_text() {
    for outside in ["0", "-1", "100.01"] {
        assert!(RiskPercent::new(decimal(outside)).is_err(), "{outside}");
    }
    for inside in ["0.01", "100", "100.00"] {
        assert!(RiskPercent::new(decimal(inside)).is_ok(), "{inside}");
    }
    for outside in ["0", "-0.5", "100", "150"] {
        assert!(StopPercent::new(decimal(outside)).is_err(), "{outside}");
    }
    for inside in ["0.01", "99.99"] {
        assert!(StopPercent::new(decimal(inside)).is_ok(), "{inside}");
    }
    assert_eq!(
        RiskPercent::new(Decimal::ZERO)
            .expect_err("none")
            .to_string(),
        "invalid risk percentage 0: must be above 0 and at most 100"
    );
    assert_eq!(
        StopPercent::new(decimal("100"))
            .expect_err("all")
            .to_string(),
        "invalid stop percentage 100: must be above 0 and below 100"
    );
    // Text in and text out, with the scale it was written with, and never
    // a JSON number.
    let stored = serde_json::to_string(&risk("1.50")).expect("json");
    assert_eq!(stored, r#""1.50""#);
    let back: RiskPercent = serde_json::from_str(&stored).expect("read back");
    assert_eq!(back.to_string(), "1.50");
    for unread in ["1.5", r#""1e1""#, r#"" 1""#, r#""0""#, r#""100.5""#] {
        assert!(
            serde_json::from_str::<RiskPercent>(unread).is_err(),
            "{unread}"
        );
    }
}

#[test]
fn a_label_is_a_short_line_with_no_space_at_either_end() {
    let longest = "x".repeat(64);
    for taken in ["main account", "ž", longest.as_str()] {
        assert!(ProfileLabel::new(taken).is_ok(), "{taken:?}");
    }
    let too_long = "x".repeat(65);
    for refused in ["", " main", "main ", "a\nb", "a\tb", too_long.as_str()] {
        assert!(ProfileLabel::new(refused).is_err(), "{refused:?}");
    }
}

#[test]
fn a_revision_starts_at_one_and_ends_where_a_store_can_still_keep_it() {
    assert!(PolicyRevision::new(0).is_err());
    assert_eq!(PolicyRevision::new(1).ok(), Some(PolicyRevision::FIRST));
    let last = PolicyRevision::new(LAST).expect("the last");
    assert!(PolicyRevision::new(LAST + 1).is_err());
    assert!(last.next().is_err(), "no revision follows the last");
    assert_eq!(
        PolicyRevision::FIRST.next().ok().map(PolicyRevision::get),
        Some(2)
    );
    assert_eq!(PolicyRevision::FIRST.previous(), None);
    assert_eq!(last.previous().map(PolicyRevision::get), Some(LAST - 1));
    assert_eq!(
        serde_json::to_string(&last).expect("json"),
        LAST.to_string()
    );
    for unread in ["0", "-1", "9223372036854775808", r#""1""#, "1.0"] {
        assert!(
            serde_json::from_str::<PolicyRevision>(unread).is_err(),
            "{unread}"
        );
    }
}

#[test]
fn a_policy_is_stored_in_one_spelling_and_read_back_as_it_was() {
    let new = ProfilePolicy::new(identity());
    assert!(!new.defaults().enabled() && new.assets().is_empty() && new.capital().is_empty());
    assert_eq!(new.to_stored().expect("json"), NEW);
    assert_eq!(ProfilePolicy::from_stored(NEW).expect("read back"), new);

    let policy = configured();
    assert_eq!(policy.to_stored().expect("json"), CONFIGURED);
    let back = ProfilePolicy::from_stored(CONFIGURED).expect("read back");
    assert_eq!(back, policy);
    // The scale an operator wrote is kept: 3.50 is not rewritten as 3.5.
    let stop_percent = back.defaults().stop_percent().expect("a stop");
    assert_eq!(stop_percent.to_string(), "3.50");
}

/// One thing changed in a stored document, and how its read then begins.
type Altered = (fn(&mut Value), &'static str);

/// `text` with one thing changed in it.
fn altered(text: &str, alter: fn(&mut Value)) -> String {
    let mut document: Value = serde_json::from_str(text).expect("json");
    alter(&mut document);
    document.to_string()
}

#[test]
fn a_stored_policy_no_build_could_have_written_is_refused() {
    let cases: [Altered; 12] = [
        (
            |d| d["format"] = json!(2),
            "policy format 2 is not supported; this build reads format 1",
        ),
        (
            |d| d["armed"] = json!(true),
            "invalid policy: unknown field `armed`",
        ),
        (
            |d| d["defaults"]["armed"] = json!(true),
            "invalid policy: unknown field `armed`",
        ),
        (
            |d| d["defaults"]["revision"] = json!(5),
            "invalid policy: a part of it is stamped revision 5, later than its own revision 4",
        ),
        (
            |d| d["defaults"]["risk_percent"] = Value::Null,
            "invalid policy: the defaults are enabled without both a risk percentage and a \
             leverage; set them, or disable the defaults",
        ),
        (
            |d| {
                d["defaults"]["enabled"] = json!(false);
                d["defaults"]["leverage"] = Value::Null;
            },
            "invalid policy: asset SOLUSDT is enabled without a leverage, its own or the \
             defaults'; set one, or disable the asset",
        ),
        (
            |d| d["assets"][0]["instrument"]["exchange"] = json!("BybitInverse"),
            "invalid policy: asset SOLUSDT is a bybit inverse contract, and the profile trades \
             bybit linear",
        ),
        (
            |d| d["assets"] = json!([d["assets"][0], d["assets"][0]]),
            "invalid policy: asset SOLUSDT is listed twice or out of order",
        ),
        (
            |d| d["capital"] = json!([d["capital"][0], d["capital"][0]]),
            "invalid policy: capital in USDT is listed twice or out of order",
        ),
        (
            |d| d["capital"][0]["amount"]["amount"] = json!("0"),
            "invalid policy: capital of 0 USDT is not above zero",
        ),
        (
            |d| d["capital"][0]["amount"]["amount"] = json!(15000),
            "invalid policy: invalid type: integer `15000`",
        ),
        (
            |d| d["defaults"]["risk_percent"] = json!("101"),
            "invalid policy: invalid risk percentage 101: must be above 0 and at most 100",
        ),
    ];
    for (alter, why) in cases {
        let error = ProfilePolicy::from_stored(&altered(CONFIGURED, alter)).expect_err(why);
        assert!(error.to_string().starts_with(why), "{error}\nnot: {why}");
    }
}
