//! A refresh racing writers and other refreshes, step by step: the read,
//! the packing, and the write of one window, with a write in between.

use super::build::{Written, pack, read_window, write_window};
use crate::storage::segments as format;
use crate::{
    Candle, CandleStore, ExchangeId, Interval, MarketId, SegmentStore, SqliteStore, Symbol,
};
use rust_decimal::Decimal;

const DAY: i64 = 86_400_000;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64, close: &str) -> Candle {
    let close: Decimal = close.parse().expect("close");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(7),
    }
}

/// A store whose first day is dirty and closed: a candle of the second day
/// makes the first a window below the tip.
fn store() -> (tempfile::TempDir, SqliteStore) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    store
        .upsert_candles(
            &market(),
            &[candle(0, "1"), candle(60_000, "1"), candle(DAY, "1")],
        )
        .expect("upsert");
    (tempdir, store)
}

#[test]
fn a_write_between_the_read_and_the_write_back_keeps_the_window_dirty() {
    let (_tempdir, store) = store();
    let window = format::window(Interval::M1, 0).expect("window");
    let mut connection = store.open().expect("connection");
    let snapshot = read_window(&mut connection, &market(), &window)
        .expect("read")
        .expect("a dirty window");

    store
        .upsert_candles(&market(), &[candle(60_000, "2")])
        .expect("a later change");
    let packed = pack(&snapshot).expect("pack");
    assert!(matches!(
        write_window(
            &mut connection,
            &market(),
            window.start,
            snapshot.id,
            &packed
        )
        .expect("write"),
        Written::Marked
    ));

    let plan = store.resolve_segments(&market(), 0, 2 * DAY).expect("plan");
    assert!(plan.segments.is_empty(), "{plan:?}");
    assert_eq!(plan.tip.map(|tip| tip.first_open_ms), Some(0));

    let refresh = store.refresh_segments(&market()).expect("refresh");
    assert_eq!(
        (refresh.built, refresh.unchanged, refresh.blocked),
        (1, 0, 0)
    );
    let plan = store.resolve_segments(&market(), 0, 2 * DAY).expect("plan");
    let [segment] = plan.segments[..] else {
        panic!("one segment: {plan:?}");
    };
    let bytes = store
        .load_segment(&segment.hash)
        .expect("load")
        .expect("stored bytes");
    let candles = format::open(&bytes, &segment.hash).expect("segment");
    assert_eq!(candles, vec![candle(0, "1"), candle(60_000, "2")]);
}

#[test]
fn of_two_refreshes_packing_one_window_the_later_one_finds_it_done() {
    let (_tempdir, store) = store();
    let window = format::window(Interval::M1, 0).expect("window");
    let mut first = store.open().expect("connection");
    let mut second = store.open().expect("connection");
    let snapshot_first = read_window(&mut first, &market(), &window)
        .expect("read")
        .expect("a dirty window");
    let snapshot_second = read_window(&mut second, &market(), &window)
        .expect("read")
        .expect("a dirty window");
    assert_eq!(snapshot_first.id, snapshot_second.id);

    let packed = pack(&snapshot_first).expect("pack");
    assert!(matches!(
        write_window(
            &mut first,
            &market(),
            window.start,
            snapshot_first.id,
            &packed
        )
        .expect("write"),
        Written::Done
    ));
    let packed = pack(&snapshot_second).expect("pack");
    assert!(matches!(
        write_window(
            &mut second,
            &market(),
            window.start,
            snapshot_second.id,
            &packed
        )
        .expect("write"),
        Written::Elsewhere
    ));
    assert!(
        read_window(&mut second, &market(), &window)
            .expect("read")
            .is_none()
    );

    let plan = store.resolve_segments(&market(), 0, 2 * DAY).expect("plan");
    assert_eq!(plan.segments.len(), 1, "{plan:?}");
    assert_eq!(plan.tip.map(|tip| tip.first_open_ms), Some(DAY));
}
