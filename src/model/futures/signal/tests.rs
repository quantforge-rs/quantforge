//! The signal rules one at a time, on a profile small enough to read.
//! Every venue, family and side, and the policy's own refusals, are held to
//! an independent model in `tests/signals/`.

use super::*;
use crate::model::futures::{
    ContractKind, DefaultsEdit, Edit, EntryIntent, ExitIntent, FaceValueSource, FuturesInstrument,
    InstrumentSpec, IntentKind, InverseContract, NewAsset, OwnerTag, PolicyChange, PositionKey,
    ProfileIdentity, ProfilePolicy, RiskPercent, StopFallback, StopPercent, TickerSpelling,
    TradingStatus,
};
use crate::model::{
    AccountRef, Asset, Environment, ExchangeId, FaceValue, InstrumentId, Leverage, Money, Quantity,
    Symbol, Venue, VenueAccountId,
};
use rust_decimal::Decimal;
use std::str::FromStr;

mod records;
mod tickers;

/// 2026-10-20T00:15:00Z, when the alerts here fired.
const FIRED: TimestampMs = 1_792_455_300_000;

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).expect("a decimal")
}

fn asset(code: &str) -> Asset {
    Asset::new(code).expect("an asset")
}

fn price(text: &str) -> Price {
    Price::new(decimal(text)).expect("a price")
}

/// An instrument of `line`: a contract of `kind` on `base` and `quote`.
fn listed(
    line: ExchangeId,
    symbol: &str,
    base: &str,
    quote: &str,
    kind: ContractKind,
) -> FuturesInstrument {
    let category = line.category();
    let quantity =
        |text: &str| Quantity::for_category(category, decimal(text)).expect("a quantity");
    let inverse = category == Category::Inverse;
    FuturesInstrument::try_from(InstrumentSpec {
        id: InstrumentId::new(
            line,
            Symbol::new(symbol).expect("a symbol"),
            asset(if inverse { base } else { quote }),
        ),
        base: asset(base),
        quote: asset(quote),
        contract: kind,
        status: TradingStatus::Trading,
        delist_at_ms: None,
        inverse: inverse.then(|| InverseContract {
            face_value: FaceValue::new(Decimal::ONE).expect("a face value"),
            source: FaceValueSource::DocumentedRule,
        }),
        tick: price("0.01"),
        step: quantity("1"),
        market_step: quantity("1"),
        min_qty: quantity("1"),
        market_min_qty: quantity("1"),
        market_max_qty: quantity("1000"),
        min_notional: None,
        leverage: None,
        observed_at_ms: FIRED,
    })
    .expect("an instrument")
}

/// What Bybit lists here: on its linear line a perpetual the profile has
/// settings for, one it has none for, a dated future and two perpetuals of
/// one pair; and an inverse perpetual, on the other line.
fn venue() -> Vec<FuturesInstrument> {
    let linear = ExchangeId::BybitLinear;
    let dated = ContractKind::Dated {
        delivery_ms: FIRED + 1,
    };
    vec![
        listed(linear, "SOLUSDT", "SOL", "USDT", ContractKind::Perpetual),
        listed(linear, "BTCUSDT", "BTC", "USDT", ContractKind::Perpetual),
        listed(linear, "BTC-26DEC26", "BTC", "USDT", dated),
        listed(linear, "XRPUSDT", "XRP", "USDT", ContractKind::Perpetual),
        listed(linear, "XRPUSDT2", "XRP", "USDT", ContractKind::Perpetual),
        listed(
            ExchangeId::BybitInverse,
            "ETHUSD",
            "ETH",
            "USD",
            ContractKind::Perpetual,
        ),
    ]
}

/// Profile `main` on Bybit's linear line: enabled defaults with a 2 %
/// fallback stop, capital in USDT, and settings of its own for SOLUSDT.
fn policy() -> ProfilePolicy {
    let account = VenueAccountId::issued("12345").expect("an account id");
    let identity = ProfileIdentity::new(
        ProfileId::new("main").expect("a profile id"),
        AccountRef::new(Venue::Bybit, Environment::Demo, account).expect("an account"),
        Category::Linear,
        OwnerTag::new("k3x9p2").expect("an owner tag"),
    )
    .expect("an identity");
    let changes = [
        PolicyChange::Defaults(DefaultsEdit {
            enabled: Some(true),
            risk_percent: Edit::Set(RiskPercent::new(Decimal::ONE).expect("a risk")),
            stop_percent: Edit::Set(StopPercent::new(Decimal::TWO).expect("a stop")),
            leverage: Edit::Set(Leverage::new(Decimal::TEN).expect("a leverage")),
        }),
        PolicyChange::SetCapital(Money::new(decimal("1000"), asset("USDT"))),
        PolicyChange::AddAsset(NewAsset {
            instrument: venue()[0].id().clone(),
            enabled: true,
            risk_percent: None,
            stop: StopFallback::Inherit,
            leverage: None,
        }),
    ];
    changes
        .iter()
        .fold(ProfilePolicy::new(identity), |policy, change| {
            policy.apply(change).expect("a change").expect("a revision")
        })
}

/// An alert for profile `main` on `ticker`, fired at [`FIRED`].
fn signal(ticker: &str, action: SignalAction) -> ExternalSignal {
    ExternalSignal {
        id: SignalId::new(format!("trend:{ticker}")).expect("a signal id"),
        payload_hash: SignalHash::from_digest([7; 32]),
        profile: ProfileId::new("main").expect("a profile id"),
        ticker: ticker.to_string(),
        exchange: None,
        category: None,
        side: PositionSide::Long,
        action,
        bar_time_ms: FIRED - 900_000,
        fire_time_ms: FIRED,
    }
}

fn entry(ticker: &str) -> ExternalSignal {
    signal(
        ticker,
        SignalAction::Entry {
            expected_price: price("234.23"),
            stop: None,
        },
    )
}

fn exit(ticker: &str) -> ExternalSignal {
    signal(ticker, SignalAction::Exit)
}

fn refused(result: Result<impl std::fmt::Debug, SignalRefusal>) -> Refusal {
    result.expect_err("refused").refusal
}

#[test]
fn an_age_limit_is_from_5_to_300_seconds() {
    assert_eq!(SignalAgeLimit::default(), SignalAgeLimit::DEFAULT);
    assert_eq!(SignalAgeLimit::DEFAULT.secs(), 60);
    assert_eq!(SignalAgeLimit::DEFAULT.ms(), 60_000);
    for secs in [5, 6, 60, 299, 300] {
        let limit = SignalAgeLimit::new(secs).expect("a limit");
        assert_eq!((limit.secs(), limit.ms()), (secs, i64::from(secs) * 1_000));
    }
    for secs in [0, 1, 4, 301, 3_600, u32::MAX] {
        let error = SignalAgeLimit::new(secs).expect_err("refused").to_string();
        assert_eq!(
            error,
            format!("invalid signal age limit \"{secs} s\": from 5 to 300 seconds")
        );
    }
    assert_eq!(SIGNAL_FUTURE_ALLOWANCE_MS, 5_000);
}

#[test]
fn an_entry_is_timely_from_five_seconds_ahead_to_its_limit_and_an_exit_always() {
    let entry = entry("SOLUSDT.P");
    let exit = exit("SOLUSDT.P");
    for secs in SignalAgeLimit::MIN_SECS..=SignalAgeLimit::MAX_SECS {
        let limit = SignalAgeLimit::new(secs).expect("a limit");
        let at = |now_ms: TimestampMs| {
            entry
                .timely(now_ms, limit)
                .map_err(|refused| refused.refusal)
        };
        assert_eq!(at(FIRED), Ok(()));
        assert_eq!(at(FIRED + limit.ms()), Ok(()), "{secs}");
        assert_eq!(at(FIRED + limit.ms() + 1), Err(Refusal::Stale), "{secs}");
        assert_eq!(at(FIRED - SIGNAL_FUTURE_ALLOWANCE_MS), Ok(()));
        assert_eq!(
            at(FIRED - SIGNAL_FUTURE_ALLOWANCE_MS - 1),
            Err(Refusal::FutureTimestamp)
        );
    }
    // No clock refuses an exit, and none makes the arithmetic fail.
    for now_ms in [
        i64::MIN,
        0,
        FIRED - 86_400_000,
        FIRED + 86_400_000,
        i64::MAX,
    ] {
        assert_eq!(
            exit.timely(now_ms, SignalAgeLimit::DEFAULT),
            Ok(()),
            "{now_ms}"
        );
        assert!(
            entry.timely(now_ms, SignalAgeLimit::DEFAULT).is_err(),
            "{now_ms}"
        );
    }
    let mut ancient = entry.clone();
    ancient.fire_time_ms = i64::MIN;
    assert_eq!(
        refused(ancient.timely(i64::MAX, SignalAgeLimit::DEFAULT)),
        Refusal::Stale
    );
    ancient.fire_time_ms = i64::MAX;
    assert_eq!(
        refused(ancient.timely(i64::MIN, SignalAgeLimit::DEFAULT)),
        Refusal::FutureTimestamp
    );
}

#[test]
fn an_age_is_said_in_seconds_to_the_millisecond() {
    let entry = entry("SOLUSDT.P");
    let said = |now_ms: TimestampMs| {
        entry
            .timely(now_ms, SignalAgeLimit::DEFAULT)
            .expect_err("refused")
            .message
    };
    assert_eq!(
        said(FIRED + 75_000),
        "the alert fired 75 s ago, and profile main takes entries up to 60 s old"
    );
    assert_eq!(
        said(FIRED + 60_001),
        "the alert fired 60.001 s ago, and profile main takes entries up to 60 s old"
    );
    assert_eq!(
        said(FIRED - 5_040),
        "the alert is timed 5.040 s ahead of the clock, and at most 5 s is allowed; check the \
         server's clock"
    );
}

#[test]
fn a_signal_is_for_a_profile_only_when_everything_it_asserts_is_that_profiles() {
    let policy = policy();
    let identity = policy.identity();
    let signal = entry("SOLUSDT.P");
    assert_eq!(signal.addressed_to(identity), Ok(()));

    let mut other = signal.clone();
    other.profile = ProfileId::new("other").expect("a profile id");
    assert_eq!(
        refused(other.addressed_to(identity)),
        Refusal::AmbiguousSelection
    );

    // An exchange is the venue's name in capitals, and nothing near it.
    let own = identity.account_ref().venue();
    for venue in Venue::ALL {
        let mut asserting = signal.clone();
        asserting.exchange = Some(venue.as_str().to_ascii_uppercase());
        assert_eq!(
            asserting.addressed_to(identity).is_ok(),
            venue == own,
            "{venue}"
        );
    }
    for exchange in ["bybit", "Bybit", "BYBIT ", "BYBIT_", "", "COINBASE"] {
        let mut asserting = signal.clone();
        asserting.exchange = Some(exchange.to_string());
        assert_eq!(
            refused(asserting.addressed_to(identity)),
            Refusal::AmbiguousSelection,
            "{exchange:?}"
        );
    }
    for category in Category::ALL {
        let mut asserting = signal.clone();
        asserting.category = Some(category);
        assert_eq!(
            asserting.addressed_to(identity).is_ok(),
            category == identity.category(),
            "{category}"
        );
    }
    // An exit is held to what it asserts like an entry.
    let mut exit = exit("SOLUSDT.P");
    exit.category = Some(Category::Inverse);
    assert_eq!(
        refused(exit.addressed_to(identity)),
        Refusal::AmbiguousSelection
    );
}
