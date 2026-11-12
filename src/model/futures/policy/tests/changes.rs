//! What a change stamps, what it leaves alone, and the changes a policy
//! refuses.

use super::*;

#[test]
fn a_change_stamps_what_it_changed_and_an_equal_value_changes_nothing() {
    let policy = configured();
    let stamps = |policy: &ProfilePolicy| {
        let sol = Symbol::new("SOLUSDT").expect("a symbol");
        let usdt = Asset::new("USDT").expect("an asset");
        [
            policy.revision().get(),
            policy.defaults().revision().get(),
            policy.asset(&sol).expect("the asset").revision().get(),
            policy.capital_in(&usdt).expect("capital").revision().get(),
        ]
    };
    assert_eq!(stamps(&policy), [4, 2, 3, 4]);

    // The same value, written with another scale, is no change: no
    // revision is made of it, and the stored text stays as it was.
    let same = [
        PolicyChange::SetCapital(money("15000.00", "USDT")),
        defaults(DefaultsEdit {
            risk_percent: Edit::Set(risk("1.0")),
            ..DefaultsEdit::default()
        }),
        defaults(DefaultsEdit::default()),
        edit("SOLUSDT", AssetEdit::default()),
        PolicyChange::Label(None),
        PolicyChange::ClearCapital(Asset::new("BTC").expect("an asset")),
    ];
    for change in same {
        assert_eq!(policy.apply(&change).expect("taken"), None, "{change:?}");
    }

    // Each change stamps its own part and leaves the others' stamps.
    let relabelled = after(
        &policy,
        PolicyChange::Label(Some(ProfileLabel::new("demo").expect("a label"))),
    );
    assert_eq!(stamps(&relabelled), [5, 2, 3, 4]);
    let disabled = edit(
        "SOLUSDT",
        AssetEdit {
            enabled: Some(false),
            ..AssetEdit::default()
        },
    );
    assert_eq!(stamps(&after(&relabelled, disabled)), [6, 2, 6, 4]);
    let more = PolicyChange::SetCapital(money("20000", "USDT"));
    assert_eq!(stamps(&after(&relabelled, more)), [6, 2, 3, 6]);
    // The policy it was made from is as it was: nothing changes in place.
    assert_eq!(policy.to_stored().expect("json"), CONFIGURED);
}

#[test]
fn no_change_leaves_what_is_enabled_without_its_settings() {
    let policy = configured();
    let new = ProfilePolicy::new(identity());
    let clear_leverage = defaults(DefaultsEdit {
        leverage: Edit::Clear,
        ..DefaultsEdit::default()
    });
    let enable = defaults(DefaultsEdit {
        enabled: Some(true),
        ..DefaultsEdit::default()
    });
    let other_line = PolicyChange::AddAsset(NewAsset {
        instrument: instrument(ExchangeId::BybitInverse, "BTCUSD", "BTC"),
        enabled: false,
        risk_percent: None,
        stop: StopFallback::Inherit,
        leverage: None,
    });
    let cases = [
        (
            refused(&policy, clear_leverage),
            "the defaults are enabled without both a risk percentage and a leverage; set them, \
             or disable the defaults",
        ),
        (
            refused(&new, enable),
            "the defaults are enabled without both a risk percentage and a leverage; set them, \
             or disable the defaults",
        ),
        (
            refused(&new, asset("SOLUSDT", true, Some("1"))),
            "asset SOLUSDT is enabled without a leverage, its own or the defaults'; set one, or \
             disable the asset",
        ),
        (
            refused(&policy, asset("SOLUSDT", true, None)),
            "asset SOLUSDT already has settings of its own; edit them",
        ),
        (
            refused(&policy, other_line),
            "BTCUSD is a bybit inverse contract, and profile main trades bybit linear",
        ),
        (
            refused(&policy, edit("BTCUSDT", AssetEdit::default())),
            "profile main holds no settings for BTCUSDT; add the asset first",
        ),
        (
            refused(&policy, PolicyChange::SetCapital(money("0", "USDT"))),
            "capital of 0 USDT is not above zero",
        ),
        (
            refused(&policy, PolicyChange::SetCapital(money("-5", "BTC"))),
            "capital of -5 BTC is not above zero",
        ),
    ];
    for (found, why) in cases {
        assert_eq!(found, format!("invalid policy change: {why}"));
    }
    // A disabled asset may lack them: it is settings kept for later.
    let kept = after(&new, asset("SOLUSDT", false, None));
    assert_eq!(kept.assets().len(), 1);
}
