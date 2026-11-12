//! What the shared cases do not reach, or do not say: the words of each
//! refusal and fault, the market-order rules where they differ from the
//! limit-order ones, a caller's own freshness limits, amounts beyond any
//! exact decimal, what can never be handed to sizing at all, and the
//! promises of the rules held over thousands of generated entries.

use quantforge::model::futures::{
    AccountSnapshot, EntryStop, FaceValueSource, Freshness, FuturesInstrument, InstrumentSpec,
    InverseContract, LeverageBracket, MarginPool, NewAsset, PolicyChange, PolicySnapshot,
    PoolScope, ProfilePolicy, RiskPercent, Sizing, SizingFault, SizingFaultKind, SizingInput,
    SizingRequest, StopFallback, StopPercent, SubmissionCheck, WalletBalance, margin_pool,
};
use quantforge::model::{
    Asset, Category, FaceValue, Leverage, Money, PositionSide, Price, Quantity, Refusal,
};
use rust_decimal::Decimal;

use crate::cases::{Case, Fixture, Given, decimal, fixture};

fn price(text: &str) -> Price {
    Price::new(decimal(text)).expect("a price")
}

/// The snapshot a case's entry is admitted under, on the pure model.
fn admitted(fixture: &Fixture, case: &Case) -> PolicySnapshot {
    let listed = case.admitted_on(fixture);
    case.policy(&listed)
        .entry_snapshot(&listed)
        .expect("an admitted entry")
}

/// What stops a case, in its own words: a refusal of sizing or of the
/// submission check, or a fault.
fn said(fixture: &Fixture, id: &str) -> String {
    let case = fixture.case(id);
    let given = Given::of(fixture, &case.inputs);
    let policy = admitted(fixture, case);
    let entry = match given.request(&case.inputs, &policy).size() {
        Err(fault) => return fault.to_string(),
        Ok(Sizing::Refused(refusal)) => return refusal.to_string(),
        Ok(Sizing::Sized(entry)) => entry,
    };
    let prices = case.inputs.prices.as_ref().expect("a case that is stopped");
    match entry.at_submission(prices, case.inputs.now_ms, &Freshness::default()) {
        Err(fault) => fault.to_string(),
        Ok(SubmissionCheck::Refused(refusal)) => refusal.to_string(),
        Ok(SubmissionCheck::Clear(_)) => panic!("{id} is not stopped"),
    }
}

#[test]
fn each_refusal_says_what_stands_in_the_way() {
    let fixture = fixture();
    for (id, words) in [
        (
            "refuse-wrong-side-stop",
            "stop_wrong_side: the alert's stop 240.00 is not below its expected entry 234.23, as \
             a long's must be",
        ),
        (
            "refuse-short-wrong-side-stop",
            "stop_wrong_side: the alert's stop 230.00 is not above its expected entry 234.23, as \
             a short's must be",
        ),
        (
            "refuse-no-stop",
            "no_stop: the alert brings no stop, and profile main gives SOLUSDT no fallback stop \
             percentage",
        ),
        (
            "refuse-stop-rounds-to-nothing",
            "no_stop: the stop rounds down to no price on SOLUSDT's tick of 0.0100, so the entry \
             has no stop the venue could hold",
        ),
        (
            "refuse-below-min-qty",
            "below_min_qty: a budget of 0.1 USDT sizes 0.000 of BTCUSDT, below its market-order \
             minimum of 0.001; a quantity is never rounded up",
        ),
        (
            "refuse-inverse-below-one-contract",
            "below_min_qty: a budget of 0.00003 BTC sizes 0 contracts of BTCUSD_PERP, below its \
             market-order minimum of 1 contracts; a quantity is never rounded up",
        ),
        (
            "refuse-above-market-max",
            "above_market_max: the entry sizes 198.117 of BTCUSDT, above its market-order \
             maximum of 120; lower the capital or the risk percentage",
        ),
        (
            "refuse-below-min-notional",
            "below_min_notional: 0.02 of SOLUSDT is worth 4.6846 USDT, below its minimum \
             notional of 5 USDT",
        ),
        (
            "refuse-leverage-above-every-tier",
            "above_leverage_bracket: no leverage bracket of BTCUSDT allows 150x; lower the \
             leverage",
        ),
        (
            "refuse-inverse-above-leverage-bracket",
            "above_leverage_bracket: a position of 400.826887169912 SOL is above the 200 SOL \
             that SOLUSD's brackets allow at 50x; lower the leverage, the capital or the risk \
             percentage",
        ),
        (
            "refuse-insufficient-margin",
            "insufficient_margin: the entry needs 331.36226955 USDT of margin, and the pool has \
             300 USDT free: 400 USDT available less 100 USDT reserved",
        ),
        (
            "refuse-reservations-above-the-pool",
            "insufficient_margin: the entry needs 190.95 USDT of margin, and the pool has -300 \
             USDT free: 400 USDT available less 700 USDT reserved",
        ),
        (
            "refuse-usd-pool-short-by-the-index",
            "insufficient_margin: the entry needs 202.62672618316 USD of margin, and the pool \
             has 202.6 USD free: 202.60 USD available less 0 USD reserved",
        ),
        (
            "refuse-wallet-below-risk",
            "settlement_wallet_below_risk: the BTC wallet holds 0.004 BTC, less than the 0.005 \
             BTC a stop-out would cost",
        ),
        (
            "refuse-no-settlement-wallet",
            "settlement_wallet_below_risk: the account holds no USDT to pay the 150 USDT a \
             stop-out would cost",
        ),
        (
            "submit-mark-crossed",
            "stop_crossed: the mark price 199.95 has reached the stop 200.0000",
        ),
        (
            "refuse-bid-exactly-at-the-stop",
            "stop_crossed: the bid 200.00 has reached the stop 200.0000",
        ),
        (
            "submit-book-crossed",
            "stop_crossed: the ask 240.01 has reached the stop 240.000",
        ),
        (
            "submit-drift-refused",
            "risk_drift: at the ask 244.00 the entry would risk 18.92 USDT, more than 1.25 × its \
             budget of 15 USDT",
        ),
        (
            "refuse-inverse-drift",
            "risk_drift: at the ask 84800.0 the entry would risk 0.006289662664 BTC, more than \
             1.25 × its budget of 0.005 BTC",
        ),
    ] {
        assert_eq!(said(&fixture, id), words, "{id}");
    }
}

#[test]
fn each_fault_names_the_input_and_what_is_wrong_with_it() {
    let fixture = fixture();
    for (id, words) in [
        (
            "fault-rules-of-another-instrument",
            "sizing cannot trust the instrument rules: they are binance_linear BTCUSDT's, and \
             the entry was admitted on binance_linear SOLUSDT",
        ),
        (
            "fault-account-of-another-key",
            "sizing cannot trust the account snapshot: it is another account's than the one the \
             entry's profile trades on",
        ),
        (
            "fault-rules-too-old",
            "sizing cannot trust the instrument rules: read 3600001 ms ago, and nothing older \
             than 3600000 ms is taken",
        ),
        (
            "fault-no-brackets",
            "sizing cannot trust the leverage brackets: the venue gave none for SOLUSDT",
        ),
        (
            "fault-brackets-in-another-asset",
            "sizing cannot trust the leverage brackets: a cap of 1000000 USD cannot hold a \
             position settled in USDT",
        ),
        (
            "fault-taker-fee-below-zero",
            "sizing cannot trust the fee rates: a taker fee of -0.0001 is below zero",
        ),
        (
            "fault-account-too-old",
            "sizing cannot trust the account snapshot: read 10001 ms ago, and nothing older \
             than 10000 ms is taken",
        ),
        (
            "fault-account-ahead-of-the-clock",
            "sizing cannot trust the account snapshot: timed 10001 ms ahead of the clock, and \
             nothing further than 10000 ms from it is taken; check the clock",
        ),
        (
            "fault-no-pool",
            "sizing cannot trust the margin pool: the account reports no USDT pool and no USD \
             pool",
        ),
        (
            "fault-two-pools",
            "sizing cannot trust the margin pool: the account reports 2 pools an entry settled \
             in USDT could draw on, and sizing does not choose between them",
        ),
        (
            "fault-pool-in-another-unit",
            "sizing cannot trust the margin pool: its available margin is stated in USDT, and \
             the pool is in USD",
        ),
        (
            "fault-reservation-below-zero",
            "sizing cannot trust the margin reservation: -1 USDT is below zero",
        ),
        (
            "fault-reservation-in-another-unit",
            "sizing cannot trust the margin reservation: it is stated in USD, and the pool is \
             in USDT",
        ),
        (
            "fault-two-settlement-wallets",
            "sizing cannot trust the settlement wallet: the account reports more than one USDT \
             wallet",
        ),
        (
            "fault-usd-pool-without-an-index",
            "sizing cannot trust the USD index price: the entry's margin pool is in USD, and no \
             index price of USDT was given to measure it at",
        ),
        (
            "fault-index-of-another-asset",
            "sizing cannot trust the USD index price: it is BTC's, and the entry settles in USDT",
        ),
        (
            "fault-index-without-a-source",
            "sizing cannot trust the USD index price: it does not say which of the venue's \
             indexes it is",
        ),
        (
            "fault-prices-of-another-instrument",
            "sizing cannot trust the prices: they are binance_linear BTCUSDT's, and the entry \
             is on binance_linear SOLUSDT",
        ),
        (
            "fault-no-bid",
            "sizing cannot trust the prices: the snapshot lacks a bid, an ask or the time its \
             book was read",
        ),
        (
            "fault-book-crossed",
            "sizing cannot trust the prices: a bid of 243.10 above an ask of 243.00 is not a book",
        ),
        (
            "fault-mark-too-old",
            "sizing cannot trust the prices: the mark price, read 5001 ms ago, and nothing \
             older than 5000 ms is taken",
        ),
        (
            "fault-quote-too-old",
            "sizing cannot trust the prices: the book, read 5001 ms ago, and nothing older than \
             5000 ms is taken",
        ),
    ] {
        assert_eq!(said(&fixture, id), words, "{id}");
    }
}

#[test]
fn fills_that_cannot_be_the_entrys_say_why() {
    let fixture = fixture();
    let case = fixture.case("prd-p9-binance-linear-long");
    let given = Given::of(&fixture, &case.inputs);
    let policy = admitted(&fixture, case);
    let Sizing::Sized(entry) = given
        .request(&case.inputs, &policy)
        .size()
        .expect("an answer")
    else {
        panic!("the entry is sized");
    };
    let contracts = Quantity::for_category(Category::Inverse, decimal("4")).expect("contracts");
    let at = price("234.23");
    for (fills, words) in [
        (vec![], "sizing cannot trust the fills: no fill was given"),
        (
            vec![(at, base("0.00")), (at, base("0"))],
            "sizing cannot trust the fills: the fills add up to nothing",
        ),
        (
            vec![(at, base("4.38")), (at, base("0.01"))],
            "sizing cannot trust the fills: they add up to 4.39, more than the 4.38 the entry \
             was sized for",
        ),
        (
            vec![(at, contracts)],
            "sizing cannot trust the fills: a fill of 4 contracts is in contracts, and the entry \
             is sized in base quantity",
        ),
    ] {
        let fault = entry.fill_risk(&fills).expect_err("a fault");
        assert_eq!(fault.to_string(), words);
        assert_eq!(fault.input(), Some(SizingInput::Fills));
    }
    // All of it filled, in two fills, is the entry's own.
    let whole = entry
        .fill_risk(&[(at, base("4.00")), (at, base("0.38"))])
        .expect("a report");
    assert_eq!(whole.filled, entry.quantity());
    assert_eq!(whole.deviation.to_string(), "0 USDT");
}

/// The request of a case, on the pure model, for a test that changes it.
struct Setup<'a> {
    fixture: &'a Fixture,
    case: &'a Case,
    given: Given,
    policy: PolicySnapshot,
    account: AccountSnapshot,
}

impl<'a> Setup<'a> {
    fn of(fixture: &'a Fixture, id: &str) -> Self {
        let case = fixture.case(id);
        Self {
            fixture,
            case,
            given: Given::of(fixture, &case.inputs),
            policy: admitted(fixture, case),
            account: case.inputs.account.clone(),
        }
    }

    fn request(&self) -> SizingRequest<'_> {
        let mut request = self.given.request(&self.case.inputs, &self.policy);
        request.account = &self.account;
        request
    }

    /// Changes the instrument's rules as the venue lists them.
    fn relist(&mut self, change: impl FnOnce(&mut InstrumentSpec)) {
        let mut spec = InstrumentSpec::from(self.given.instrument.clone());
        change(&mut spec);
        self.given.instrument = FuturesInstrument::try_from(spec).expect("an instrument");
    }

    /// Gives the profile another policy for the same asset, at 1x.
    fn repolicy(&mut self, capital: &str, risk_percent: &str, stop: StopFallback) {
        let case = self.case;
        let listed = case.admitted_on(self.fixture);
        let asset = listed.id().settle.clone();
        let changes = [
            PolicyChange::SetCapital(Money::new(decimal(capital), asset)),
            PolicyChange::AddAsset(NewAsset {
                instrument: listed.id().clone(),
                enabled: true,
                risk_percent: Some(RiskPercent::new(decimal(risk_percent)).expect("a risk")),
                stop,
                leverage: Some(Leverage::new(Decimal::ONE).expect("a leverage")),
            }),
        ];
        let policy = changes.iter().fold(
            ProfilePolicy::new(case.inputs.profile.clone()),
            |policy, change| policy.apply(change).expect("taken").expect("a revision"),
        );
        self.policy = policy.entry_snapshot(&listed).expect("an admitted entry");
    }
}

fn base(text: &str) -> Quantity {
    Quantity::for_category(Category::Linear, decimal(text)).expect("a quantity")
}

fn sized(request: &SizingRequest<'_>) -> Quantity {
    match request.size().expect("an answer") {
        Sizing::Sized(entry) => entry.quantity(),
        Sizing::Refused(refusal) => panic!("refused: {refusal}"),
    }
}

fn refusal(request: &SizingRequest<'_>) -> Refusal {
    match request.size().expect("an answer") {
        Sizing::Refused(refusal) => refusal.refusal,
        Sizing::Sized(entry) => panic!("sized {}", entry.quantity()),
    }
}

/// An entry is a market order: where a venue lists a coarser step, a
/// higher minimum or a lower maximum for market orders than for limit
/// orders, the market-order ones decide.
#[test]
fn the_quantity_is_held_to_the_market_order_step_and_limits_not_the_limit_order_ones() {
    // 150 / 34.23 is 4.382…: 4.38 on a 0.01 step, 4.3 on a 0.1 step.
    let fixture = fixture();
    let mut setup = Setup::of(&fixture, "prd-p9-binance-linear-long");
    assert_eq!(sized(&setup.request()), base("4.38"));
    setup.relist(|spec| spec.market_step = base("0.1"));
    assert_eq!(sized(&setup.request()), base("4.3"));
    setup.relist(|spec| spec.step = base("1"));
    assert_eq!(
        sized(&setup.request()),
        base("4.3"),
        "the limit step decides nothing"
    );

    setup.relist(|spec| spec.market_min_qty = base("4.4"));
    assert_eq!(refusal(&setup.request()), Refusal::BelowMinQty);
    setup.relist(|spec| {
        spec.market_min_qty = base("0.1");
        spec.min_qty = base("5");
    });
    assert_eq!(
        sized(&setup.request()),
        base("4.3"),
        "the limit minimum decides nothing"
    );

    setup.relist(|spec| spec.market_max_qty = base("4.2"));
    assert_eq!(refusal(&setup.request()), Refusal::AboveMarketMax);
    setup.relist(|spec| spec.market_max_qty = base("4.3"));
    assert_eq!(sized(&setup.request()), base("4.3"));
}

/// A tick or a step written with more decimal places than it needs must
/// not put a stop or a quantity out of reach: 16 on a step of 0.1 written
/// with 28 places is more than a decimal counts in at 28 places, and is
/// still 16.
#[test]
fn a_grid_written_with_more_places_than_a_decimal_counts_in_still_sizes() {
    let fixture = fixture();
    // 150 / 9.37 is 16.008…: 16.0 on a step of 0.1.
    let mut setup = Setup::of(&fixture, "bybit-linear-short");
    assert_eq!(sized(&setup.request()).to_string(), "16.0");
    setup.relist(|spec| {
        spec.market_step = base("0.1000000000000000000000000000");
        spec.tick = price("0.0100000000000000000000000000");
    });
    let Sizing::Sized(entry) = setup.request().size().expect("an answer") else {
        panic!("the entry is sized");
    };
    assert_eq!(entry.quantity(), base("16"));
    assert_eq!(entry.quantity().to_string(), "16");
    // 243.60 at 28 places is past a decimal too, and is still the stop.
    assert_eq!(entry.stop().price, price("243.6"));
    assert_eq!(entry.stop().price.to_string(), "243.6");
    assert_eq!(entry.position().risk.to_string(), "149.92 USDT");
    // A step it can count in keeps its scale, zeros and all.
    setup.relist(|spec| spec.market_step = base("0.1000"));
    assert_eq!(sized(&setup.request()).to_string(), "16.0000");
}

#[test]
fn a_quantity_of_nothing_is_below_every_minimum_even_one_of_nothing() {
    let fixture = fixture();
    let mut setup = Setup::of(&fixture, "refuse-below-min-qty");
    setup.relist(|spec| spec.market_min_qty = base("0"));
    match setup.request().size().expect("an answer") {
        Sizing::Refused(refusal) => {
            assert_eq!(refusal.refusal, Refusal::BelowMinQty);
            let position = refusal.figures.position.expect("a quantity");
            assert!(position.quantity.is_zero());
            assert!(position.risk.is_zero());
        }
        Sizing::Sized(entry) => panic!("sized {}", entry.quantity()),
    }
}

#[test]
fn an_instrument_without_a_minimum_notional_is_held_to_none() {
    let fixture = fixture();
    let mut setup = Setup::of(&fixture, "refuse-below-min-notional");
    assert_eq!(refusal(&setup.request()), Refusal::BelowMinNotional);
    setup.relist(|spec| spec.min_notional = None);
    assert_eq!(sized(&setup.request()), base("0.02"));
}

#[test]
fn a_caller_may_set_its_own_freshness_limits() {
    let stale = |fault: SizingFault, input: SizingInput| {
        assert_eq!(fault.kind(), SizingFaultKind::Stale);
        assert_eq!(fault.input(), Some(input));
    };
    // An account snapshot 10,001 ms old is past the default, and within a
    // limit of 10,001 ms.
    let fixture = fixture();
    let setup = Setup::of(&fixture, "fault-account-too-old");
    let mut request = setup.request();
    stale(request.size().expect_err("stale"), SizingInput::Account);
    request.freshness.account_ms = 10_001;
    assert_eq!(sized(&request), base("4.38"));

    // The cases' rules are a minute old, their brackets a minute and a
    // half, their fees two: each is held to the rules' limit.
    let setup = Setup::of(&fixture, "prd-p9-binance-linear-long");
    for (limit, input) in [
        (59_999, Some(SizingInput::Rules)),
        (89_999, Some(SizingInput::Brackets)),
        (119_999, Some(SizingInput::Fees)),
        (120_000, None),
    ] {
        let mut request = setup.request();
        request.freshness.rules_ms = limit;
        match (request.size(), input) {
            (Err(fault), Some(input)) => stale(fault, input),
            (Ok(Sizing::Sized(_)), None) => {}
            (other, _) => panic!("with rules at most {limit} ms old: {other:?}"),
        }
    }
    // A limit below zero trusts nothing, not even an input read just now.
    let mut request = setup.request();
    request.freshness.account_ms = -1;
    request.now_ms = setup.account.observed_at_ms;
    stale(request.size().expect_err("stale"), SizingInput::Account);

    // A mark price 300 ms old and a book 200 ms old, at submission.
    let setup = Setup::of(&fixture, "submit-drift-accepted");
    let case = setup.case;
    let Sizing::Sized(entry) = setup.request().size().expect("an answer") else {
        panic!("the entry is sized");
    };
    let prices = case.inputs.prices.as_ref().expect("prices");
    for (limit, clear) in [(300, true), (299, false), (0, false)] {
        let limits = Freshness {
            prices_ms: limit,
            ..Freshness::default()
        };
        match (
            entry.at_submission(prices, case.inputs.now_ms, &limits),
            clear,
        ) {
            (Ok(SubmissionCheck::Clear(estimate)), true) => {
                assert_eq!(estimate.price, price("243.00"), "a long fills at the ask");
                assert_eq!(estimate.risk.to_string(), "18.49 USDT");
            }
            (Err(fault), false) => stale(fault, SizingInput::Prices),
            (other, _) => panic!("with prices at most {limit} ms old: {other:?}"),
        }
    }
}

#[test]
fn an_amount_beyond_an_exact_decimal_is_a_fault_and_never_a_panic() {
    // 7 × 10^28 USDT, all of it at risk, a cent from the stop: 7 × 10^30
    // SOL, which no exact decimal holds.
    let fixture = fixture();
    let mut setup = Setup::of(&fixture, "prd-p9-bybit-linear-long");
    setup.repolicy("70000000000000000000000000000", "100", StopFallback::Off);
    let mut request = setup.request();
    request.signal_stop = Some(price("234.22"));
    let fault = request.size().expect_err("a fault");
    assert_eq!(fault.kind(), SizingFaultKind::OutOfRange);
    assert_eq!(fault.input(), None);
    assert_eq!(
        fault.to_string(),
        "sizing cannot state a figure: the quantity is beyond what an exact decimal holds"
    );
    // A capital an exact decimal can still count the quantity of is sized,
    // and refused for what it is: far above the market-order maximum.
    setup.repolicy("1000000000000000000000000", "100", StopFallback::Off);
    let mut request = setup.request();
    request.signal_stop = Some(price("234.22"));
    assert_eq!(refusal(&request), Refusal::AboveMarketMax);
}

/// The sized entry says where each figure came from: the stop asked for,
/// the pool and, for a USD pool, the index price with its source and time.
#[test]
fn a_sized_entry_keeps_what_it_was_worked_out_from() {
    let fixture = fixture();
    let setup = Setup::of(&fixture, "usd-pool-linear-fits");
    let case = setup.case;
    let Sizing::Sized(entry) = setup.request().size().expect("an answer") else {
        panic!("the entry is sized");
    };
    assert_eq!(entry.instrument(), setup.given.instrument.id());
    assert_eq!(entry.side(), PositionSide::Long);
    assert_eq!(entry.expected_price(), price("234.23"));
    assert_eq!(
        entry.leverage(),
        Leverage::new(decimal("5")).expect("a leverage")
    );
    assert_eq!(entry.stop().asked, EntryStop::Signal(price("200.00")));
    assert_eq!(
        entry.stop().price.to_string(),
        "200.000",
        "on the tick's own scale"
    );
    assert_eq!(entry.quantity().to_string(), "4.3");
    assert_eq!(entry.budget().to_string(), "150 USDT");
    assert_eq!(entry.position().unit_loss.to_string(), "34.23 USDT");
    assert_eq!(entry.notional().quote.to_string(), "1007.189 USDT");
    assert_eq!(entry.margin().required.to_string(), "202.5457079 USDT");
    assert_eq!(entry.bracket_cap().to_string(), "60000000 USDT");
    let pool = entry.pool();
    assert_eq!(pool.scope, PoolScope::AccountUsd);
    assert_eq!(pool.required.to_string(), "202.62672618316 USD");
    assert_eq!(pool.free.to_string(), "202.63 USD");
    let index = pool.index.as_ref().expect("the index it was measured at");
    assert_eq!(index.source, "USDT index price");
    assert_eq!(index.price, price("1.0004"));
    assert_eq!(index.observed_at_ms, case.inputs.now_ms - 1_000);
    assert!(entry.warnings().is_empty());
    assert_eq!(Sizing::Sized(entry.clone()).figures(), entry.figures());

    // An asset's own pool needs no conversion, and records none.
    let setup = Setup::of(&fixture, "linear-short-percent-stop");
    let Sizing::Sized(entry) = setup.request().size().expect("an answer") else {
        panic!("the entry is sized");
    };
    let percent = StopPercent::new(decimal("3")).expect("a stop percentage");
    assert_eq!(entry.stop().asked, EntryStop::Fallback(percent));
    assert_eq!(
        entry.pool().scope,
        PoolScope::Asset(Asset::new("USDT").expect("an asset"))
    );
    assert_eq!(entry.pool().index, None);
}

#[test]
fn the_pool_an_entry_draws_on_is_named_for_whoever_totals_its_reservations() {
    let fixture = fixture();
    let usdt = Asset::new("USDT").expect("an asset");
    let own = &fixture.case("prd-p9-binance-linear-long").inputs.account;
    assert_eq!(
        margin_pool(own, &usdt).expect("a pool").scope,
        PoolScope::Asset(usdt.clone())
    );
    let usd = &fixture.case("usd-pool-linear-fits").inputs.account;
    assert_eq!(
        margin_pool(usd, &usdt).expect("a pool").scope,
        PoolScope::AccountUsd
    );
    // A coin-margined account has a pool for each coin.
    let coins = &fixture.case("several-wallets-and-pools").inputs.account;
    for coin in ["ETH", "BTC"] {
        let coin = Asset::new(coin).expect("an asset");
        let pool = margin_pool(coins, &coin).expect("a pool");
        assert_eq!(pool.scope, PoolScope::Asset(coin));
    }
    let none = margin_pool(coins, &usdt).expect_err("no pool");
    assert_eq!(
        (none.kind(), none.input()),
        (SizingFaultKind::Missing, Some(SizingInput::Pool))
    );
    let both = &fixture.case("fault-two-pools").inputs.account;
    let two = margin_pool(both, &usdt).expect_err("two pools");
    assert_eq!(
        (two.kind(), two.input()),
        (SizingFaultKind::Ambiguous, Some(SizingInput::Pool))
    );
}

/// Zero and negative prices, a capital of nothing, a risk of nothing or
/// of more than everything, a leverage below 1: none can be built, so none
/// reaches sizing.
#[test]
fn what_sizing_could_not_work_with_cannot_be_handed_to_it() {
    for bad in ["0", "0.00", "-234.23"] {
        assert!(Price::new(decimal(bad)).is_err(), "a price of {bad}");
    }
    for bad in ["0", "-1", "100.01"] {
        assert!(RiskPercent::new(decimal(bad)).is_err(), "a risk of {bad} %");
    }
    for bad in ["0", "100", "-3"] {
        assert!(StopPercent::new(decimal(bad)).is_err(), "a stop of {bad} %");
    }
    for bad in ["0", "0.99", "-5"] {
        assert!(Leverage::new(decimal(bad)).is_err(), "a leverage of {bad}");
    }
    assert!(FaceValue::new(Decimal::ZERO).is_err());
    let fixture = fixture();
    let case = fixture.case("prd-p9-binance-linear-long");
    let policy = ProfilePolicy::new(case.inputs.profile.clone());
    let usdt = Asset::new("USDT").expect("an asset");
    for bad in ["0", "-15000"] {
        let capital = PolicyChange::SetCapital(Money::new(decimal(bad), usdt.clone()));
        assert!(policy.apply(&capital).is_err(), "a capital of {bad}");
    }
    // A snapshot is checked again when it is read back: one with its
    // capital in another asset than its instrument settles in is refused.
    let mut stored = case.inputs.snapshot.clone().expect("a snapshot");
    assert!(serde_json::from_value::<PolicySnapshot>(stored.clone()).is_ok());
    stored["capital"]["asset"] = "BTC".into();
    assert!(serde_json::from_value::<PolicySnapshot>(stored).is_err());
    // An inverse contract without its face value is not an instrument.
    let mut inverse = fixture.instruments["bybit inverse BTCUSD"].clone();
    assert!(FuturesInstrument::try_from(inverse.clone()).is_ok());
    inverse.inverse = None;
    assert!(FuturesInstrument::try_from(inverse).is_err());
}

/// A fixed sequence of numbers.
struct Sequence(u64);

impl Sequence {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 20) % bound
    }

    /// A number of `places` decimal places from `low` up to `high`, as
    /// whole units of the last place.
    fn between(&mut self, low: u64, high: u64, places: u32) -> Decimal {
        let units = low + self.below(high - low + 1);
        Decimal::new(i64::try_from(units).expect("small"), places)
    }
}

/// The rules' promises, held over generated entries on a linear and on an
/// inverse instrument of every face value: the stop on its tick, on the
/// protective side and never tighter than asked; the quantity on its
/// step; its risk within the budget; and one more step past it.
#[test]
fn every_sized_entry_keeps_the_rules_promises() {
    let fixture = fixture();
    let mut numbers = Sequence(35);
    let mut sized = 0;
    for round in 0..6_000 {
        let inverse = round % 2 == 1;
        let id = if inverse {
            "bybit-inverse-long"
        } else {
            "prd-p9-bybit-linear-long"
        };
        let mut setup = Setup::of(&fixture, id);
        let face = [1u32, 10, 100][numbers.below(3) as usize];
        let tick = [
            Decimal::new(1, 2),
            Decimal::new(5, 1),
            Decimal::new(25, 2),
            Decimal::ONE,
        ][numbers.below(4) as usize];
        let step = if inverse {
            Decimal::ONE
        } else {
            [Decimal::new(1, 3), Decimal::new(1, 1), Decimal::new(5, 2)][numbers.below(3) as usize]
        };
        setup.relist(|spec| {
            let category = spec.id.category();
            let quantity = |value| Quantity::for_category(category, value).expect("a quantity");
            spec.tick = Price::new(tick).expect("a tick");
            spec.market_step = quantity(step);
            spec.market_min_qty = quantity(step);
            spec.market_max_qty = quantity(Decimal::from(u32::MAX));
            spec.min_notional = None;
            if inverse {
                spec.inverse = Some(InverseContract {
                    face_value: FaceValue::new(Decimal::from(face)).expect("a face value"),
                    source: FaceValueSource::Metadata,
                });
            }
        });
        // An account and a bracket that hold any of it: only the stop and
        // the quantity are judged here.
        let settle = setup.given.instrument.id().settle.clone();
        let plenty = Money::new(Decimal::from(u64::MAX), settle);
        setup.given.brackets = vec![LeverageBracket {
            max_leverage: Leverage::new(Decimal::ONE).expect("a leverage"),
            cap: plenty.clone(),
        }];
        setup.account.pools = vec![MarginPool {
            scope: PoolScope::Asset(plenty.asset().clone()),
            available: plenty.clone(),
        }];
        setup.account.wallets = vec![WalletBalance {
            balance: plenty.clone(),
            available: plenty,
        }];

        let entry_price = Price::new(numbers.between(500, 90_000, 1)).expect("a price");
        let long = numbers.below(2) == 0;
        let percent = numbers.between(5, 4_000, 2);
        let by_percent = numbers.below(2) == 0;
        let capital = if inverse {
            numbers.between(1, 500_000, 4)
        } else {
            numbers.between(100, 900_000_000, 2)
        };
        let risk = numbers.between(1, 50, 1);
        let fallback = if by_percent {
            StopFallback::Percent(StopPercent::new(percent).expect("a stop percentage"))
        } else {
            StopFallback::Off
        };
        setup.repolicy(&capital.to_string(), &risk.to_string(), fallback);
        let e = entry_price.value();
        let hundred = Decimal::ONE_HUNDRED;
        let asked = if long {
            e * (hundred - percent) / hundred
        } else {
            e * (hundred + percent) / hundred
        };
        let mut request = setup.request();
        request.side = if long {
            PositionSide::Long
        } else {
            PositionSide::Short
        };
        request.expected_price = entry_price;
        request.signal_stop = (!by_percent).then(|| Price::new(asked).expect("a stop"));
        let at = format!(
            "round {round}: {} {capital} at {risk} %, E {e}, stop {asked}, tick {tick}, step \
             {step}, face {face}",
            request.side
        );
        let entry = match request
            .size()
            .unwrap_or_else(|fault| panic!("{at}: {fault}"))
        {
            Sizing::Sized(entry) => entry,
            // A budget too small for one step: the cases cover refusals.
            Sizing::Refused(refusal) => {
                assert_eq!(refusal.refusal, Refusal::BelowMinQty, "{at}: {refusal}");
                continue;
            }
        };
        sized += 1;
        let (s, q) = (entry.stop().price.value(), entry.quantity().value());
        assert!((s % tick).is_zero(), "{at}: the stop {s} is off the tick");
        assert!(
            (q % step).is_zero(),
            "{at}: the quantity {q} is off the step"
        );
        // Away from the entry: never tighter than asked, and less than a
        // tick looser.
        if long {
            assert!(s < e && s <= asked && asked - s < tick, "{at}: stop {s}");
        } else {
            assert!(s > e && s >= asked && s - asked < tick, "{at}: stop {s}");
        }
        // The risk within the budget, and one more step past it. An
        // inverse loss is F × |1/E − 1/S′| a contract: both sides are
        // multiplied through by E × S′, so nothing is divided.
        let (budget, apart) = (capital * risk / hundred, (e - s).abs());
        let (lost, more, within) = if inverse {
            let face = Decimal::from(face);
            (q * face * apart, (q + step) * face * apart, budget * e * s)
        } else {
            (q * apart, (q + step) * apart, budget)
        };
        assert!(lost <= within, "{at}: {q} risks more than the budget");
        assert!(more > within, "{at}: {q} is not the most the budget allows");
        assert_eq!(entry.budget().amount(), budget, "{at}");
    }
    assert!(sized > 4_000, "only {sized} entries were sized");
}
