//! The segment store's steps, and the checks coded here: every shared case
//! of `tests/fixtures/segment_cases.json` packs to its own name in a store;
//! altered bytes are refused when opened; a failed write-back changes
//! nothing; and a plan never lists a window without the corrections
//! committed before it, however refreshes race writers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use quantforge::storage::segments::{self, SegmentError};
use quantforge::{
    Candle, CandleQuery, CandleStore, ExchangeId, Interval, MarketId, SegmentHash, SegmentStore,
    Symbol,
};
use rust_decimal::Decimal;

use super::cases::{CandleText, Cases, PlanText, SegmentText, TipText, segment_spec_cases};
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "segments.spec_cases",
        "every case of the segment spec, stored and closed by a later candle, packs to the case's name",
    ),
    (
        "segments.altered_bytes_refused",
        "bytes altered behind the store's back load as stored and are refused when opened: another segment's bytes, bytes that are no segment, and a payload of a format this build does not read",
    ),
    (
        "segments.failed_write_back_changes_nothing",
        "a write-back that fails stores no segment and no index entry and leaves every mark, so the next refresh packs the window",
    ),
    (
        "segments.refreshes_racing_writers",
        "with refreshes racing writers, every plan that lists a window packs each correction committed before it, and every window ends indexed under exactly its final candles",
    ),
];

pub fn check<S: CandleStore + SegmentStore + Send>(id: &str) -> Coded<S> {
    match id {
        "segments.spec_cases" => spec_cases,
        "segments.altered_bytes_refused" => altered_bytes_refused,
        "segments.failed_write_back_changes_nothing" => failed_write_back_changes_nothing,
        "segments.refreshes_racing_writers" => refreshes_racing_writers,
        other => panic!("no segment check {other}"),
    }
}

/// 2026-10-20T00:00:00Z: the start of a `1m` window, one day long.
const WINDOW: i64 = 1_792_454_400_000;
const DAY: i64 = 86_400_000;
const MINUTE: i64 = 60_000;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitLinear,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64, close: &str) -> Candle {
    let close = Decimal::from_str_exact(close).expect("a decimal");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + MINUTE - 1,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(3),
    }
}

/// Two candles in each of `windows` windows from [`WINDOW`], and the candle
/// of the next one, which closes them.
fn closed_windows(windows: i64) -> Vec<Candle> {
    let mut candles: Vec<Candle> = (0..windows)
        .flat_map(|window| {
            let start = WINDOW + window * DAY;
            [candle(start, "100.10"), candle(start + MINUTE, "100.20")]
        })
        .collect();
    candles.push(candle(WINDOW + windows * DAY, "100.30"));
    candles
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

/// Every shared case of the segment spec, stored under a market of its
/// interval and closed by a candle of the next window, is packed to the
/// name the case gives, and loads to the case's candles with their text.
fn spec_cases<S: CandleStore + SegmentStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    for (index, case) in segment_spec_cases().iter().enumerate() {
        let candles: Vec<Candle> = case.candles.iter().map(CandleText::candle).collect();
        let interval = case.interval();
        let step = interval.step_ms();
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

/// A store hands back the bytes it holds, whatever was done to them, and a
/// reader opening them under the name it asked for finds out: a store
/// checks nothing when it loads, and a plan reads no bytes at all.
fn altered_bytes_refused<S: CandleStore + SegmentStore>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &fixture.raw);
    let market = market();
    store
        .upsert_candles(&market, &closed_windows(2))
        .expect("candles");
    store.refresh_segments(&market).expect("refresh");
    let last = WINDOW + 2 * DAY - 1;
    let plan = store
        .resolve_segments(&market, WINDOW, last)
        .expect("a plan");
    let [first, second] = plan.segments.as_slice() else {
        panic!("two segments expected: {plan:?}");
    };
    let stored = |name: &SegmentHash| {
        store
            .load_segment(name)
            .expect("load")
            .unwrap_or_else(|| panic!("{name} is not stored"))
    };
    let second_bytes = stored(&second.hash);

    // Another segment's bytes under this name.
    raw.execute(&format!(
        "UPDATE segments SET data = (SELECT data FROM segments WHERE hash = '{}') \
         WHERE hash = '{}'",
        second.hash, first.hash
    ));
    let swapped = stored(&first.hash);
    assert_eq!(swapped, second_bytes, "loaded as stored");
    let opened = segments::open(&swapped, &first.hash);
    assert!(
        matches!(opened, Err(SegmentError::HashMismatch { .. })),
        "{opened:?}"
    );

    // Bytes that are no segment.
    let junk = b"not a segment";
    raw.execute(&format!(
        "UPDATE segments SET data = {} WHERE hash = '{}'",
        raw.blob(junk),
        second.hash
    ));
    assert_eq!(stored(&second.hash), junk, "loaded as stored");
    let opened = segments::open(junk, &second.hash);
    assert!(
        matches!(opened, Err(SegmentError::NotOneFrame)),
        "{opened:?}"
    );

    // A payload of a format this build does not read, under its true name.
    let mut payload = segments::decompress(&second_bytes).expect("the payload");
    payload[8..12].copy_from_slice(&2_u32.to_le_bytes());
    let name = segments::hash(&payload);
    let bytes = segments::compress(&payload).expect("the stored bytes");
    raw.execute(&format!(
        "INSERT INTO segments(hash, format, payload_len, data) VALUES ('{name}', 2, {}, {})",
        payload.len(),
        raw.blob(&bytes)
    ));
    assert_eq!(stored(&name), bytes, "loaded as stored");
    let opened = segments::open(&bytes, &name);
    assert!(
        matches!(opened, Err(SegmentError::UnsupportedFormat(2))),
        "{opened:?}"
    );

    // The plan still lists both windows, as long as their bytes are now.
    let plan = store
        .resolve_segments(&market, WINDOW, last)
        .expect("a plan");
    let lengths: Vec<u64> = plan
        .segments
        .iter()
        .map(|segment| segment.stored_len)
        .collect();
    assert_eq!(lengths, [swapped.len() as u64, junk.len() as u64]);
}

/// The index refuses the entry after the blob went in: the blob goes with
/// it, every mark stays, and the window packs once the index takes it.
fn failed_write_back_changes_nothing<S: CandleStore + SegmentStore>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &fixture.raw);
    let market = market();
    store
        .upsert_candles(&market, &closed_windows(1))
        .expect("candles");
    let marks = "SELECT CAST(id AS TEXT) FROM segment_dirty ORDER BY id";
    let before = raw.select(marks);
    assert_eq!(before.len(), 2, "the closed window and the tip");
    let count = |table: &str| raw.select(&format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}"));
    let none = [[Some("0".to_string())]];

    let [refuse, allow] = raw.write_refusal("segment_index");
    raw.execute(&refuse);
    store
        .refresh_segments(&market)
        .expect_err("the index refuses the entry");
    assert_eq!(count("segments"), none, "no blob without its entry");
    assert_eq!(count("segment_index"), none);
    assert_eq!(raw.select(marks), before, "the marks stay as they were");
    let plan = store
        .resolve_segments(&market, WINDOW, WINDOW + DAY - 1)
        .expect("a plan");
    assert!(plan.segments.is_empty(), "{plan:?}");

    raw.execute(&allow);
    let done = store.refresh_segments(&market).expect("refresh");
    assert_eq!(done.built, 1, "{done:?}");
    let plan = store
        .resolve_segments(&market, WINDOW, WINDOW + DAY - 1)
        .expect("a plan");
    assert_eq!(plan.segments.len(), 1, "{plan:?}");
}

/// Two writers each correct a window of their own in bursts, while two
/// refreshes run until they stop. A plan listing a window says nothing of
/// it is waiting, so from the moment a correction commits, every plan that
/// lists the window must pack it, however the refreshes interleaved. A
/// write-back that published what it read before the burst ended is caught
/// by the next plan, and a later correction cannot hide it.
fn refreshes_racing_writers<S: CandleStore + SegmentStore + Send>(fixture: &Fixture<S>) {
    const WRITERS: i64 = 2;
    const ROUNDS: i64 = 20;
    const BURST: i64 = 3;
    let market = market();
    fixture
        .store
        .upsert_candles(&market, &closed_windows(WRITERS))
        .expect("candles");
    let refreshes: Vec<S> = (0..2).map(|_| (fixture.reopen)()).collect();
    let writers: Vec<S> = (0..WRITERS).map(|_| (fixture.reopen)()).collect();
    let writing = AtomicBool::new(true);
    std::thread::scope(|scope| {
        for store in refreshes {
            let (market, writing) = (&market, &writing);
            scope.spawn(move || {
                while writing.load(Ordering::Relaxed) {
                    store.refresh_segments(market).expect("a refresh");
                    // Room for a writer: a spinning refresh only starves it.
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
        }
        let writers: Vec<_> = writers
            .into_iter()
            .zip(0_i64..)
            .map(|(store, writer)| {
                let market = &market;
                scope.spawn(move || {
                    let window = WINDOW + writer * DAY;
                    for round in 0..ROUNDS {
                        let mut close = String::new();
                        for step in 0..BURST {
                            close = format!("{}.{step}", 200 + round);
                            store
                                .upsert_candles(market, &[candle(window, &close)])
                                .expect("a correction");
                        }
                        published_with(&store, market, window, &close);
                    }
                })
            })
            .collect();
        let finished: Vec<_> = writers.into_iter().map(|writer| writer.join()).collect();
        // Before anything can fail, or the refreshes would run on.
        writing.store(false, Ordering::Relaxed);
        for writer in finished {
            if let Err(panic) = writer {
                std::panic::resume_unwind(panic);
            }
        }
    });

    // Nothing is left to pack, and every candle is in its segment.
    let store = &fixture.store;
    let done = store.refresh_segments(&market).expect("the last refresh");
    assert_eq!(done.built, 0, "{done:?}");
    let plan = store
        .resolve_segments(&market, WINDOW, WINDOW + WRITERS * DAY - 1)
        .expect("a plan");
    assert_eq!(plan.segments.len(), WRITERS as usize, "{plan:?}");
    assert_eq!(plan.tip, None, "{plan:?}");
    for segment in &plan.segments {
        let stored: Vec<CandleText> = store
            .load_candles(
                &market,
                CandleQuery {
                    start_time_ms: Some(segment.window_start_ms),
                    end_time_ms: Some(segment.window_start_ms + DAY - 1),
                    limit: None,
                },
            )
            .expect("the candles")
            .iter()
            .map(CandleText::of)
            .collect();
        let packed: Vec<CandleText> = packed(store, &segment.hash)
            .iter()
            .map(CandleText::of)
            .collect();
        assert_eq!(packed, stored, "window {}", segment.window_start_ms);
    }
}

/// The candles of a stored segment.
fn packed<S: SegmentStore + ?Sized>(store: &S, name: &SegmentHash) -> Vec<Candle> {
    let bytes = store
        .load_segment(name)
        .expect("load")
        .unwrap_or_else(|| panic!("{name} is listed but not stored"));
    segments::open(&bytes, name).expect("open")
}

/// Waits for a plan to list `window`, and holds what it lists to the
/// correction that just committed.
fn published_with<S: SegmentStore>(store: &S, market: &MarketId, window: i64, close: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let plan = store
            .resolve_segments(market, window, window)
            .expect("a plan");
        if let [segment] = plan.segments.as_slice() {
            let candles = packed(store, &segment.hash);
            assert_eq!(
                candles[0].close.to_string(),
                close,
                "window {window} is listed without its correction"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "window {window} was never packed"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
