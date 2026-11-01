//! How a batch becomes one statement: the last candle of each open time,
//! in open-time order, and the windows that duplicates which differ mark
//! whatever is stored, as SQLite's row by row writes would.

use crate::storage::postgres::candles::Batch;
use crate::{Candle, ExchangeId, Interval, MarketId, StorageError, Symbol};
use rust_decimal::Decimal;

const DAY: i64 = 86_400_000;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64, close: &str) -> Candle {
    let close = Decimal::from_str_exact(close).expect("a decimal");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms.saturating_add(59_999),
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(7),
    }
}

#[test]
fn the_rows_are_in_open_time_order_with_their_windows() {
    let batch = Batch::of(
        &market(),
        &[candle(DAY + 60_000, "2"), candle(0, "1"), candle(DAY, "3")],
    )
    .expect("a batch");
    assert_eq!(batch.open_time_ms, [0, DAY, DAY + 60_000]);
    assert_eq!(batch.window_start_ms, [0, DAY, DAY]);
    assert_eq!(batch.close, ["1", "3", "2"]);
    assert_eq!(batch.trades, [Some(7); 3]);
    assert!(batch.forced.is_empty());
}

#[test]
fn the_last_duplicate_wins_and_marks_its_window_when_the_duplicates_differ() {
    let batch = Batch::of(
        &market(),
        &[candle(DAY, "1"), candle(0, "5"), candle(DAY, "2")],
    )
    .expect("a batch");
    assert_eq!(batch.open_time_ms, [0, DAY]);
    assert_eq!(batch.close, ["5", "2"]);
    // Written in turn, the second changes the row the first wrote.
    assert_eq!(batch.forced.iter().copied().collect::<Vec<_>>(), [DAY]);
}

#[test]
fn duplicates_alike_mark_nothing_but_a_change_of_scale_is_a_change() {
    let alike = Batch::of(&market(), &[candle(0, "1.10"), candle(0, "1.10")]).expect("a batch");
    assert!(alike.forced.is_empty());
    // Equal numbers, different text: two different candles.
    let rescaled = Batch::of(&market(), &[candle(0, "1.10"), candle(0, "1.1")]).expect("a batch");
    assert_eq!(rescaled.close, ["1.1"]);
    assert_eq!(rescaled.forced.iter().copied().collect::<Vec<_>>(), [0]);
}

#[test]
fn a_trade_count_beyond_i64_is_stored_as_the_signed_column_holds_it() {
    let mut beyond = candle(0, "1");
    beyond.trades = Some(u64::MAX);
    let mut unknown = candle(60_000, "1");
    unknown.trades = None;
    let batch = Batch::of(&market(), &[beyond, unknown]).expect("a batch");
    assert_eq!(batch.trades, [Some(-1), None]);
}

#[test]
fn a_candle_whose_window_cannot_be_written_refuses_the_whole_batch() {
    let refused =
        Batch::of(&market(), &[candle(0, "1"), candle(i64::MAX - 100, "1")]).expect_err("refused");
    assert!(
        matches!(&refused, StorageError::InvalidArgument(message)
            if message.contains(&(i64::MAX - 100).to_string())),
        "{refused:?}"
    );
}
