//! The segment store's steps, and the check that every shared case of
//! `tests/fixtures/segment_cases.json` packs to its own name in a store.

use quantforge::storage::segments;
use quantforge::{
    Candle, CandleStore, ExchangeId, Interval, MarketId, SegmentHash, SegmentStore, Symbol,
};
use serde::Deserialize;

use super::cases::{CandleText, Cases, PlanText, SegmentText, TipText, read};
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[(
    "segments.spec_cases",
    "every case of the segment spec, stored and closed by a later candle, packs to the case's name",
)];

pub fn check<S: CandleStore + SegmentStore>(id: &str) -> Coded<S> {
    match id {
        "segments.spec_cases" => spec_cases,
        other => panic!("no segment check {other}"),
    }
}

fn hash(text: &str) -> SegmentHash {
    text.parse()
        .unwrap_or_else(|error| panic!("{text}: {error}"))
}

/// Runs a segment step; `false` for a step of another port.
pub fn step<S: SegmentStore + ?Sized>(cases: &Cases, store: &S, step: &Step) -> bool {
    match step {
        Step::Refresh { market, expect } => {
            let done = store
                .refresh_segments(&cases.market(market))
                .expect("refresh");
            assert_eq!(
                (done.built, done.unchanged, done.blocked),
                (expect.built, expect.unchanged, expect.blocked),
                "refresh of {market}: built, unchanged, blocked"
            );
        }
        Step::Resolve {
            market,
            first,
            last,
            expect,
            refused,
        } => {
            let outcome = store.resolve_segments(&cases.market(market), *first, *last);
            match (expect, refused) {
                (Some(expect), None) => {
                    let plan = outcome.expect("resolve");
                    plan_is(store, &plan, expect, &format!("{market} {first}..={last}"));
                }
                (None, Some(refused)) => expect_refused(outcome, refused, "resolve"),
                _ => panic!("a resolve expects a plan or a refusal"),
            }
        }
        Step::LoadSegment { hash: name, expect } => {
            let stored = store.load_segment(&hash(name)).expect("load segment");
            match (stored, expect) {
                (None, None) => {}
                (Some(bytes), Some(expect)) => {
                    let candles = segments::open(&bytes, &hash(name)).expect("open");
                    assert_eq!(candles.len(), expect.bar_count, "{name}");
                }
                (stored, expect) => panic!(
                    "{name}: stored {:?}, expected {expect:?}",
                    stored.map(|bytes| bytes.len())
                ),
            }
        }
        _ => return false,
    }
    true
}

/// A plan is the one expected, and every segment it lists loads, opens
/// under its name to as many candles as it says, and is as long as it says.
fn plan_is<S: SegmentStore + ?Sized>(
    store: &S,
    plan: &quantforge::SegmentPlan,
    expect: &PlanText,
    what: &str,
) {
    let found: Vec<SegmentText> = plan
        .segments
        .iter()
        .map(|segment| SegmentText {
            window_start_ms: segment.window_start_ms,
            first_open_ms: segment.first_open_ms,
            last_open_ms: segment.last_open_ms,
            bar_count: segment.bar_count,
            hash: segment.hash.to_string(),
            payload_len: segment.payload_len,
        })
        .collect();
    assert_eq!(found, expect.segments, "segments of {what}");
    let tip = plan.tip.map(|tip| TipText {
        first_open_ms: tip.first_open_ms,
        last_open_ms: tip.last_open_ms,
    });
    assert_eq!(tip, expect.tip, "tip of {what}");
    for segment in &plan.segments {
        let bytes = store
            .load_segment(&segment.hash)
            .expect("load")
            .unwrap_or_else(|| panic!("{} is listed but not stored", segment.hash));
        assert_eq!(segment.stored_len, bytes.len() as u64, "{}", segment.hash);
        let candles = segments::open(&bytes, &segment.hash).expect("open");
        assert_eq!(
            candles.len(),
            segment.bar_count as usize,
            "{}",
            segment.hash
        );
    }
}

/// Every interval's wire form.
const INTERVALS: [&str; 15] = [
    "1s", "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d", "3d", "1w",
];

#[derive(Debug, Deserialize)]
struct SpecCases {
    cases: Vec<SpecCase>,
}

#[derive(Debug, Deserialize)]
struct SpecCase {
    name: String,
    candles: Vec<CandleText>,
    hash: String,
}

/// Every shared case of the segment spec, stored under a market of its
/// interval and closed by a candle of the next window, is packed to the
/// name the case gives, and loads to the case's candles with their text.
fn spec_cases<S: CandleStore + SegmentStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    let spec: SpecCases = read("tests/fixtures/segment_cases.json");
    for (index, case) in spec.cases.iter().enumerate() {
        let candles: Vec<Candle> = case.candles.iter().map(CandleText::candle).collect();
        let step = candles[0].close_time_ms - candles[0].open_time_ms + 1;
        let interval = INTERVALS
            .into_iter()
            .map(|token| token.parse::<Interval>().expect("interval"))
            .find(|interval| interval.step_ms() == step)
            .unwrap_or_else(|| panic!("{}: no interval of {step} ms", case.name));
        // A market of its own for each case, so no case closes another.
        let market = MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new(format!("CASE{index}USDT")).expect("symbol"),
            interval,
        );
        let window = segments::window(interval, candles[0].open_time_ms).expect("window");
        let mut closing = candles[0].clone();
        closing.open_time_ms = window.end;
        closing.close_time_ms = window.end + step - 1;
        store
            .upsert_candles(&market, &candles)
            .expect("the case's candles");
        store
            .upsert_candles(&market, &[closing])
            .expect("a candle of the next window");
        let done = store.refresh_segments(&market).expect("refresh");
        assert_eq!(done.built, 1, "{}", case.name);
        let plan = store
            .resolve_segments(&market, window.start, window.end - 1)
            .expect("plan");
        let [segment] = plan.segments.as_slice() else {
            panic!("{}: one segment expected: {plan:?}", case.name);
        };
        assert_eq!(segment.hash.to_string(), case.hash, "{}", case.name);
        let bytes = store
            .load_segment(&segment.hash)
            .expect("load")
            .expect("stored bytes");
        let opened: Vec<CandleText> = segments::open(&bytes, &segment.hash)
            .expect("open")
            .iter()
            .map(CandleText::of)
            .collect();
        assert_eq!(opened, case.candles, "{}", case.name);
    }
}
