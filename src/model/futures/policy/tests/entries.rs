//! An entry held to the policy: why one is refused, what an admitted one
//! carries, and the venue's list held against whatever could take one.

use super::*;

#[test]
fn an_entry_is_refused_for_the_first_of_its_reasons_in_one_fixed_order() {
    let new = ProfilePolicy::new(identity());
    let policy = configured();
    let reason = |policy: &ProfilePolicy, listed: &FuturesInstrument| {
        let refusal = policy.entry_snapshot(listed).expect_err("refused");
        (refusal.refusal, refusal.message)
    };
    let said = |refusal: Refusal, message: &str| (refusal, message.to_string());
    // Nothing configured, defaults off: the ticker is unknown to the
    // profile, whatever else is wrong with it.
    let dated = listed("SOLUSDT", "USDT", ContractKind::Dated { delivery_ms: 5 });
    assert_eq!(
        reason(&new, &dated),
        said(
            Refusal::UnknownTicker,
            "profile main has no settings for SOLUSDT, and its defaults are not enabled"
        )
    );
    // Held under another settlement asset than the venue lists: no guess.
    assert_eq!(
        reason(&policy, &listed("SOLUSDT", "USDC", ContractKind::Perpetual)),
        said(
            Refusal::AmbiguousSelection,
            "profile main holds SOLUSDT as settled in USDT, and the venue lists it settled in USDC"
        )
    );
    // Disabled comes before the universe and the capital.
    let off = AssetEdit {
        enabled: Some(false),
        ..AssetEdit::default()
    };
    let disabled = after(&policy, edit("SOLUSDT", off));
    assert_eq!(
        reason(&disabled, &dated),
        said(
            Refusal::NotArmed,
            "SOLUSDT is disabled in profile main; enable the asset to take entries on it"
        )
    );
    // Then the universe, before the capital.
    let poor = after(
        &policy,
        PolicyChange::ClearCapital(Asset::new("USDT").expect("an asset")),
    );
    assert_eq!(reason(&poor, &dated).0, Refusal::OutsideUniverse);
    assert_eq!(
        reason(&poor, &sol()),
        said(
            Refusal::NoCapitalInSettlementAsset,
            "profile main has no capital in USDT, which SOLUSDT settles in"
        )
    );
    // An asset without settings trades on enabled defaults alone.
    let inherited = policy
        .entry_snapshot(&listed("BTCUSDT", "USDT", ContractKind::Perpetual))
        .expect("the defaults");
    assert_eq!(inherited.risk_percent().source, SettingSource::Defaults);
    assert_eq!(inherited.stop_percent().value, Some(stop("3.50")));
}

#[test]
fn a_snapshot_is_stored_whole_and_read_back_only_as_one_could_be_taken() {
    let snapshot = configured().entry_snapshot(&sol()).expect("an entry");
    assert_eq!(serde_json::to_string(&snapshot).expect("json"), SNAPSHOT);
    let back: PolicySnapshot = serde_json::from_str(SNAPSHOT).expect("read back");
    assert_eq!(back, snapshot);
    // The asset turned the fallback off, so only the alert's stop will do.
    let price = Price::new(decimal("200.00")).expect("a price");
    assert_eq!(back.stop_for(Some(price)), Ok(EntryStop::Signal(price)));
    let none = back.stop_for(None).expect_err("no stop");
    assert_eq!(
        none.to_string(),
        "no_stop: the alert brings no stop, and profile main gives SOLUSDT no fallback stop \
         percentage"
    );

    let cases: [Altered; 6] = [
        (
            |s| s["format"] = json!(2),
            "policy snapshot format 2 is not supported; this build reads format 1",
        ),
        (
            |s| s["credentials"] = json!("key"),
            "unknown field `credentials`",
        ),
        (
            |s| s["capital"]["asset"] = json!("BTC"),
            "invalid policy snapshot: its capital is in BTC, and SOLUSDT settles in USDT",
        ),
        (
            |s| s["capital"]["amount"] = json!("0"),
            "invalid policy snapshot: its capital is not above zero",
        ),
        (
            |s| s["capital_revision"] = json!(5),
            "invalid policy snapshot: its capital is of revision 5, later than its own revision 4",
        ),
        (
            |s| s["instrument"]["exchange"] = json!("BybitSpot"),
            "invalid policy snapshot: a spot instrument has no policy",
        ),
    ];
    for (alter, why) in cases {
        let error = serde_json::from_str::<PolicySnapshot>(&altered(SNAPSHOT, alter))
            .expect_err(why)
            .to_string();
        assert!(error.starts_with(why), "{error}\nnot: {why}");
    }
    // Nor one whose account is on another venue than its instrument.
    let other = Venue::ALL
        .into_iter()
        .find(|venue| *venue != Venue::Bybit)
        .expect("another venue");
    let mut moved: Value = serde_json::from_str(SNAPSHOT).expect("json");
    moved["account"]["venue"] = serde_json::to_value(other).expect("json");
    let error = serde_json::from_value::<PolicySnapshot>(moved).expect_err("another venue's");
    assert_eq!(
        error.to_string(),
        format!("invalid policy snapshot: SOLUSDT trades on bybit, and its account is on {other}")
    );
    // It names the account it was taken for, the profile's own.
    assert_eq!(snapshot.account(), identity().account_ref());
}

#[test]
fn the_venues_list_is_held_against_whatever_could_take_an_entry() {
    let policy = configured();
    let kinds = |policy: &ProfilePolicy, list: &[FuturesInstrument]| -> Vec<String> {
        policy
            .check(list)
            .iter()
            .map(|problem| format!("{} {}", problem.subject, problem.kind.as_str()))
            .collect()
    };
    assert_eq!(kinds(&policy, &[sol()]), Vec::<String>::new());
    assert_eq!(kinds(&policy, &[]), ["SOLUSDT not_listed"]);
    let usdc = listed("SOLUSDT", "USDC", ContractKind::Perpetual);
    assert_eq!(kinds(&policy, &[usdc]), ["SOLUSDT settlement_changed"]);
    let tradfi = listed("SOLUSDT", "USDT", ContractKind::TradFiPerpetual);
    assert_eq!(kinds(&policy, &[tradfi]), ["SOLUSDT outside_universe"]);

    // The venue takes 1 to 50 in steps of 0.5.
    let with = |text: &str| {
        let own = AssetEdit {
            leverage: Edit::Set(leverage(text)),
            ..AssetEdit::default()
        };
        after(&policy, edit("SOLUSDT", own))
    };
    assert_eq!(kinds(&with("50"), &[sol()]), Vec::<String>::new());
    assert_eq!(kinds(&with("12.5"), &[sol()]), Vec::<String>::new());
    assert_eq!(
        kinds(&with("50.5"), &[sol()]),
        ["SOLUSDT leverage_out_of_range"]
    );
    assert_eq!(
        kinds(&with("12.25"), &[sol()]),
        ["SOLUSDT leverage_off_step"]
    );
    let off_step = with("12.25").check(&[sol()]);
    assert_eq!(
        off_step[0].to_string(),
        "SOLUSDT: the venue takes a leverage from 1 to 50 in steps of 0.5, not 12.25"
    );

    // Without capital both the asset and the enabled defaults say so; a
    // disabled asset is not held to the list at all.
    let poor = after(
        &policy,
        PolicyChange::ClearCapital(Asset::new("USDT").expect("an asset")),
    );
    assert_eq!(
        kinds(&poor, &[sol()]),
        ["SOLUSDT no_capital", "defaults no_capital"]
    );
    let off = AssetEdit {
        enabled: Some(false),
        ..AssetEdit::default()
    };
    assert_eq!(
        kinds(&after(&poor, edit("SOLUSDT", off)), &[]),
        ["defaults no_capital"]
    );
}
