//! The SQLite store packing segments as candles are written: the same
//! candles always pack to the same segments, a refresh packs only the
//! windows whose candles changed, and a plan trusts the index only below
//! the first window that still has to be packed.

use quantforge::storage::segments::{self, WINDOW_BARS};
use quantforge::{
    Candle, CandleQuery, CandleStore, ExchangeId, Interval, MarketId, SegmentPlan, SegmentStore,
    SegmentTip, SqliteStore, StorageError, Symbol,
};
use rust_decimal::Decimal;
use tempfile::TempDir;

pub const MINUTE: i64 = 60_000;
pub const DAY: i64 = 86_400_000;
/// 2026-10-20T00:00:00Z, a UTC midnight.
const T: i64 = 1_792_454_400_000;

pub fn market(interval: Interval) -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        interval,
    )
}

pub fn m1() -> MarketId {
    market(Interval::M1)
}

pub fn candle(interval: Interval, open_time_ms: i64, close: &str) -> Candle {
    let close: Decimal = close.parse().expect("close");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + interval.step_ms() - 1,
        open: close,
        high: close,
        low: close,
        close,
        volume: "0.50000000".parse().expect("volume"),
        trades: Some(open_time_ms.rem_euclid(97) as u64),
    }
}

/// `count` one-minute candles from `from`, each with its own close.
pub fn minutes(from: i64, count: i64) -> Vec<Candle> {
    (0..count)
        .map(|index| {
            candle(
                Interval::M1,
                from + index * MINUTE,
                &format!("{}.{:02}", 100 + index % 50, index % 100),
            )
        })
        .collect()
}

pub fn store() -> (TempDir, SqliteStore) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    (tempdir, store)
}

/// What a refresh did: built, unchanged, blocked.
pub fn refresh(store: &SqliteStore, market: &MarketId) -> (usize, usize, usize) {
    let refresh = store.refresh_segments(market).expect("refresh");
    (refresh.built, refresh.unchanged, refresh.blocked)
}

pub fn plan(store: &SqliteStore, market: &MarketId, first: i64, last: i64) -> SegmentPlan {
    store.resolve_segments(market, first, last).expect("plan")
}

fn windows(plan: &SegmentPlan) -> Vec<i64> {
    plan.segments
        .iter()
        .map(|segment| segment.window_start_ms)
        .collect()
}

fn tip(first_open_ms: i64, last_open_ms: i64) -> Option<SegmentTip> {
    Some(SegmentTip {
        first_open_ms,
        last_open_ms,
    })
}

/// The candles a stored segment packs, with every decimal as its text.
fn opened(store: &SqliteStore, plan: &SegmentPlan, index: usize) -> Vec<String> {
    let segment = plan.segments[index];
    let bytes = store
        .load_segment(&segment.hash)
        .expect("load")
        .expect("stored bytes");
    texts(&segments::open(&bytes, &segment.hash).expect("open"))
}

fn texts(candles: &[Candle]) -> Vec<String> {
    candles
        .iter()
        .map(|candle| {
            format!(
                "{} {} {} {} {} {} {} {:?}",
                candle.open_time_ms,
                candle.close_time_ms,
                candle.open,
                candle.high,
                candle.low,
                candle.close,
                candle.volume,
                candle.trades
            )
        })
        .collect()
}

#[test]
fn two_stores_filled_in_different_orders_pack_the_same_segments() {
    // Two whole days, then the first minute of a third: its tip.
    let candles = minutes(T, 2 * 1440 + 1);
    let (_first_dir, first) = store();
    first.upsert_candles(&m1(), &candles).expect("upsert");
    let (_second_dir, second) = store();
    for chunk in candles.rchunks(500) {
        second.upsert_candles(&m1(), chunk).expect("upsert");
    }

    assert_eq!(refresh(&first, &m1()), (2, 0, 0));
    assert_eq!(refresh(&second, &m1()), (2, 0, 0));
    let plan_first = plan(&first, &m1(), T, T + 3 * DAY);
    assert_eq!(plan(&second, &m1(), T, T + 3 * DAY), plan_first);
    assert_eq!(windows(&plan_first), vec![T, T + DAY]);
    assert_eq!(plan_first.tip, tip(T + 2 * DAY, T + 3 * DAY));
    for segment in &plan_first.segments {
        assert_eq!(
            first.load_segment(&segment.hash).expect("load"),
            second.load_segment(&segment.hash).expect("load")
        );
    }

    // A segment is named after the payload of exactly its window's candles.
    let day = segments::encode(&candles[..WINDOW_BARS as usize]).expect("payload");
    let segment = plan_first.segments[0];
    assert_eq!(segment.hash, segments::hash(&day));
    assert_eq!(
        (
            segment.first_open_ms,
            segment.last_open_ms,
            segment.bar_count
        ),
        (T, T + 1439 * MINUTE, 1440)
    );
    assert_eq!(segment.payload_len, day.len() as u64);
}

#[test]
fn a_second_refresh_and_an_identical_resync_change_nothing() {
    let (_tempdir, store) = store();
    let candles = minutes(T, 1441);
    store.upsert_candles(&m1(), &candles).expect("upsert");
    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let before = plan(&store, &m1(), T, T + DAY);

    assert_eq!(refresh(&store, &m1()), (0, 0, 0));
    assert_eq!(
        store.upsert_candles(&m1(), &candles).expect("resync"),
        candles.len()
    );
    assert_eq!(refresh(&store, &m1()), (0, 0, 0));
    assert_eq!(plan(&store, &m1(), T, T + DAY), before);
}

#[test]
fn a_window_changed_and_changed_back_keeps_its_segment() {
    let (_tempdir, store) = store();
    let candles = minutes(T, 1441);
    store.upsert_candles(&m1(), &candles).expect("upsert");
    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let before = plan(&store, &m1(), T, T + DAY);

    store
        .upsert_candles(&m1(), &[candle(Interval::M1, T, "999")])
        .expect("a change");
    store
        .upsert_candles(&m1(), &candles[..1])
        .expect("and back");
    assert_eq!(refresh(&store, &m1()), (0, 1, 0));
    assert_eq!(plan(&store, &m1(), T, T + DAY), before);
}

#[test]
fn the_window_of_the_newest_candle_stays_in_the_tip_until_a_later_window_starts() {
    let (_tempdir, store) = store();
    store
        .upsert_candles(&m1(), &minutes(T, 1440))
        .expect("upsert");
    assert_eq!(refresh(&store, &m1()), (0, 0, 0));
    let whole_day = plan(&store, &m1(), T, T + DAY);
    assert!(whole_day.segments.is_empty());
    assert_eq!(whole_day.tip, tip(T, T + DAY));

    store
        .upsert_candles(&m1(), &[candle(Interval::M1, T + DAY, "1")])
        .expect("upsert");
    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let next = plan(&store, &m1(), T, T + DAY);
    assert_eq!(windows(&next), vec![T]);
    assert_eq!(next.tip, tip(T + DAY, T + DAY));
}

#[test]
fn a_backfill_reopens_its_window_at_once_and_the_next_refresh_packs_only_that_window() {
    let (_tempdir, store) = store();
    // The first day misses its hundredth minute.
    let mut candles = minutes(T, 2 * 1440 + 1);
    let missing = candles.remove(100);
    store.upsert_candles(&m1(), &candles).expect("upsert");
    assert_eq!(refresh(&store, &m1()), (2, 0, 0));
    let before = plan(&store, &m1(), T, T + 3 * DAY);
    assert_eq!(before.segments[0].bar_count, 1439);

    store
        .upsert_candles(&m1(), std::slice::from_ref(&missing))
        .expect("backfill");
    let reopened = plan(&store, &m1(), T, T + 3 * DAY);
    assert!(reopened.segments.is_empty(), "{reopened:?}");
    assert_eq!(reopened.tip, tip(T, T + 3 * DAY));

    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let after = plan(&store, &m1(), T, T + 3 * DAY);
    assert_eq!(windows(&after), vec![T, T + DAY]);
    assert_eq!(after.segments[0].bar_count, 1440);
    assert_ne!(after.segments[0].hash, before.segments[0].hash);
    assert_eq!(after.segments[1], before.segments[1]);
    // The segment it replaced keeps its name and its candles.
    assert_eq!(opened(&store, &before, 0), texts(&candles[..1439]));
}

#[test]
fn a_candle_whose_scale_changes_is_a_changed_candle() {
    let (_tempdir, store) = store();
    store
        .upsert_candles(
            &m1(),
            &[
                candle(Interval::M1, T, "1.10"),
                candle(Interval::M1, T + DAY, "1"),
            ],
        )
        .expect("upsert");
    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let before = plan(&store, &m1(), T, T);

    store
        .upsert_candles(&m1(), &[candle(Interval::M1, T, "1.1")])
        .expect("same value, other scale");
    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let after = plan(&store, &m1(), T, T);
    assert_ne!(after.segments[0].hash, before.segments[0].hash);
    assert_eq!(
        opened(&store, &after, 0),
        texts(&[candle(Interval::M1, T, "1.1")])
    );
}

#[test]
fn a_plan_lists_the_segments_overlapping_a_range_then_the_tip() {
    let (_tempdir, store) = store();
    store
        .upsert_candles(&m1(), &minutes(T, 3 * 1440 + 1))
        .expect("upsert");
    assert_eq!(refresh(&store, &m1()), (3, 0, 0));

    let cases = [
        // Before every candle.
        ((T - 10 * DAY, T - 1), vec![], None),
        (
            (T + 30 * MINUTE, T + DAY + 10 * MINUTE),
            vec![T, T + DAY],
            None,
        ),
        // The last candle of one window and the first of the next.
        ((T + DAY - MINUTE, T + DAY), vec![T, T + DAY], None),
        ((T + DAY, T + DAY), vec![T + DAY], None),
        (
            (T + 2 * DAY + 5, T + 4 * DAY),
            vec![T + 2 * DAY],
            tip(T + 3 * DAY, T + 4 * DAY),
        ),
        (
            (T + 3 * DAY, T + 5 * DAY),
            vec![],
            tip(T + 3 * DAY, T + 5 * DAY),
        ),
        // After every candle.
        (
            (T + 10 * DAY, T + 11 * DAY),
            vec![],
            tip(T + 10 * DAY, T + 11 * DAY),
        ),
    ];
    for ((first, last), expected_windows, expected_tip) in cases {
        let plan = plan(&store, &m1(), first, last);
        assert_eq!(windows(&plan), expected_windows, "{first}..={last}");
        assert_eq!(plan.tip, expected_tip, "{first}..={last}");
    }

    assert!(matches!(
        store.resolve_segments(&m1(), T + 1, T),
        Err(StorageError::InvalidArgument(message)) if message.contains("after the last")
    ));
}

#[test]
fn a_market_without_candles_is_all_tip() {
    let (_tempdir, store) = store();
    assert_eq!(refresh(&store, &m1()), (0, 0, 0));
    let plan = plan(&store, &m1(), T, T + DAY);
    assert!(plan.segments.is_empty());
    assert_eq!(plan.tip, tip(T, T + DAY));
}

#[test]
fn three_day_and_weekly_candles_stay_in_the_tip_of_their_long_windows() {
    // 2017-08-17 for the 3d grid, and a Monday, 2017-08-14, for the weekly.
    for (interval, first) in [
        (Interval::D3, 1_502_928_000_000),
        (Interval::W1, 1_502_668_800_000),
    ] {
        let (_tempdir, store) = store();
        let market = market(interval);
        let candles: Vec<Candle> = (0..100)
            .map(|index| candle(interval, first + index * interval.step_ms(), "100"))
            .collect();
        store.upsert_candles(&market, &candles).expect("upsert");
        assert_eq!(refresh(&store, &market), (0, 0, 0), "{interval}");
        let last = first + 99 * interval.step_ms();
        let plan = plan(&store, &market, first, last);
        assert!(plan.segments.is_empty(), "{interval}");
        assert_eq!(plan.tip, tip(first, last), "{interval}");
    }
}

#[test]
fn a_window_that_cannot_be_packed_keeps_itself_and_every_later_window_in_the_tip() {
    let (_tempdir, store) = store();
    // Candles every 30 seconds: more than a window of one-minute bars holds.
    let crowded: Vec<Candle> = (0..i64::from(WINDOW_BARS) + 1)
        .map(|index| candle(Interval::M1, T + index * 30_000, "1"))
        .collect();
    store.upsert_candles(&m1(), &crowded).expect("upsert");
    store
        .upsert_candles(&m1(), &minutes(T + DAY, 10))
        .expect("upsert");
    store
        .upsert_candles(&m1(), &[candle(Interval::M1, T + 2 * DAY, "1")])
        .expect("upsert");

    assert_eq!(refresh(&store, &m1()), (1, 0, 1));
    let plan = plan(&store, &m1(), T, T + 3 * DAY);
    assert!(plan.segments.is_empty(), "{plan:?}");
    assert_eq!(plan.tip, tip(T, T + 3 * DAY));
    // Every refresh tries it again, and leaves it.
    assert_eq!(refresh(&store, &m1()), (0, 0, 1));
}

#[test]
fn stored_text_not_in_its_exact_form_is_left_unpacked_rather_than_corrected() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("market.sqlite");
    let store = SqliteStore::new(&path);
    CandleStore::init(&store).expect("init");
    store.upsert_candles(&m1(), &minutes(T, 3)).expect("upsert");
    store
        .upsert_candles(&m1(), &[candle(Interval::M1, T + DAY, "1")])
        .expect("upsert");
    // Written behind the store's back, as an older or foreign writer might.
    rusqlite::Connection::open(&path)
        .expect("open")
        .execute(
            "UPDATE candles SET close = '0100.00' WHERE open_time_ms = ?1",
            [T],
        )
        .expect("doctor a close");

    assert_eq!(refresh(&store, &m1()), (0, 0, 1));
    assert!(plan(&store, &m1(), T, T).segments.is_empty());
}

#[test]
fn a_stored_segment_opens_to_the_candles_it_packs() {
    let (_tempdir, store) = store();
    let candles = minutes(T, 1441);
    store.upsert_candles(&m1(), &candles).expect("upsert");
    assert_eq!(refresh(&store, &m1()), (1, 0, 0));
    let plan = plan(&store, &m1(), T, T);
    let segment = plan.segments[0];
    let bytes = store
        .load_segment(&segment.hash)
        .expect("load")
        .expect("stored bytes");
    assert_eq!(segment.stored_len, bytes.len() as u64);
    assert_eq!(
        segment.payload_len,
        segments::decompress(&bytes).expect("payload").len() as u64
    );
    assert_eq!(opened(&store, &plan, 0), texts(&candles[..1440]));

    let unknown = segments::hash(b"no segment");
    assert_eq!(store.load_segment(&unknown).expect("load"), None);
}

#[test]
fn a_candle_whose_window_cannot_be_written_is_refused_before_anything_is_stored() {
    let (_tempdir, store) = store();
    let mut far = candle(Interval::M1, T, "1");
    far.open_time_ms = i64::MAX - 100;
    far.close_time_ms = i64::MAX;
    let refused = store
        .upsert_candles(&m1(), &[candle(Interval::M1, T, "1"), far])
        .expect_err("refused");
    assert!(
        matches!(&refused, StorageError::InvalidArgument(message) if message.contains(&(i64::MAX - 100).to_string())),
        "{refused}"
    );
    assert!(
        store
            .load_candles(&m1(), CandleQuery::default())
            .expect("load")
            .is_empty()
    );
}
