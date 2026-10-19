//! `CandleBatch` ↔ `Vec<Candle>` round trips. `Decimal`'s `==` compares
//! values, so `81940.01000000 == 81940.01` would hide a lost scale; these
//! tests compare `Decimal::serialize()`, which covers scale and sign as well
//! as digits.

use quantforge::{Candle, CandleBatch, CandleColumns};
use rust_decimal::Decimal;

fn exact(text: &str) -> Decimal {
    Decimal::from_str_exact(text).expect("decimal")
}

/// A zero that keeps its minus sign: parsing "-0" gives a positive zero.
fn negative_zero() -> Decimal {
    let mut zero = Decimal::ZERO;
    zero.set_sign_negative(true);
    assert!(zero.is_sign_negative(), "negative zero");
    zero
}

// Exact strings as Binance sends them (8 decimal places, trailing zeros
// kept), plus the corners a lossless container has to carry: the decimal
// extremes, 28 decimal places, a negative zero, the i64 timestamp extremes,
// and every shape of trade count.
fn candles() -> Vec<Candle> {
    vec![
        Candle {
            open_time_ms: 1_760_000_000_000,
            close_time_ms: 1_760_000_059_999,
            open: exact("81940.01000000"),
            high: exact("81950.00000000"),
            low: exact("81900.10000000"),
            close: exact("81945.55000000"),
            volume: exact("0.17235000"),
            trades: Some(39),
        },
        Candle {
            open_time_ms: i64::MIN,
            close_time_ms: i64::MAX,
            open: Decimal::MAX,
            high: Decimal::MIN,
            low: exact("0.0000000000000000000000000001"),
            close: negative_zero(),
            volume: Decimal::ZERO,
            trades: None,
        },
        Candle {
            open_time_ms: 0,
            close_time_ms: -1,
            open: exact("1.10"),
            high: exact("1.1"),
            low: exact("1.100000000000000000000000000"),
            close: exact("-2.5"),
            volume: exact("14118.14755140"),
            trades: Some(0),
        },
        Candle {
            open_time_ms: -60_000,
            close_time_ms: 59_999,
            open: exact("998431.2042875853960324310791"),
            high: exact("0.12147738891864899987371366"),
            low: exact("66633140290.84541427"),
            close: exact("1"),
            volume: exact("79228162514264337593543950335"),
            trades: Some(u64::MAX),
        },
    ]
}

fn bytes(column: &[Decimal]) -> Vec<[u8; 16]> {
    column.iter().map(|value| value.serialize()).collect()
}

/// Every field identical, decimals by their exact representation.
fn assert_identical(actual: &[Candle], expected: &[Candle]) {
    assert_eq!(actual.len(), expected.len(), "row count");
    for (row, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            (a.open_time_ms, a.close_time_ms, a.trades),
            (e.open_time_ms, e.close_time_ms, e.trades),
            "row {row}"
        );
        for (field, got, want) in [
            ("open", a.open, e.open),
            ("high", a.high, e.high),
            ("low", a.low, e.low),
            ("close", a.close, e.close),
            ("volume", a.volume, e.volume),
        ] {
            assert_eq!(
                got.serialize(),
                want.serialize(),
                "row {row} {field}: {got} is not exactly {want}"
            );
        }
    }
}

/// Every column identical, decimals by their exact representation.
fn assert_same_columns(actual: &CandleColumns, expected: &CandleColumns) {
    assert_eq!(actual.open_time_ms, expected.open_time_ms, "open_time_ms");
    assert_eq!(
        actual.close_time_ms, expected.close_time_ms,
        "close_time_ms"
    );
    assert_eq!(bytes(&actual.open), bytes(&expected.open), "open");
    assert_eq!(bytes(&actual.high), bytes(&expected.high), "high");
    assert_eq!(bytes(&actual.low), bytes(&expected.low), "low");
    assert_eq!(bytes(&actual.close), bytes(&expected.close), "close");
    assert_eq!(bytes(&actual.volume), bytes(&expected.volume), "volume");
    assert_eq!(actual.trades, expected.trades, "trades");
}

#[test]
fn candles_survive_a_round_trip_through_a_batch_unchanged() {
    let candles = candles();
    let batch = CandleBatch::from_candles(&candles);
    assert_eq!(batch.len(), candles.len());
    assert!(!batch.is_empty());
    assert_identical(&batch.to_candles(), &candles);
}

#[test]
fn a_batch_survives_a_round_trip_through_candles_unchanged() {
    let columns = CandleBatch::from_candles(&candles()).into_columns();
    let batch = CandleBatch::from_columns(columns.clone()).expect("equal lengths");
    let again = CandleBatch::from_candles(&batch.to_candles()).into_columns();
    assert_same_columns(&again, &columns);
}

#[test]
fn batch_columns_hold_the_candles_in_row_order() {
    let candles = candles();
    let batch = CandleBatch::from_candles(&candles);
    let columns = batch.columns();
    for (row, candle) in candles.iter().enumerate() {
        assert_eq!(columns.open_time_ms[row], candle.open_time_ms, "row {row}");
        assert_eq!(
            columns.close_time_ms[row], candle.close_time_ms,
            "row {row}"
        );
        assert_eq!(columns.open[row].serialize(), candle.open.serialize());
        assert_eq!(columns.high[row].serialize(), candle.high.serialize());
        assert_eq!(columns.low[row].serialize(), candle.low.serialize());
        assert_eq!(columns.close[row].serialize(), candle.close.serialize());
        assert_eq!(columns.volume[row].serialize(), candle.volume.serialize());
        assert_eq!(columns.trades[row], candle.trades, "row {row}");
    }
}

#[test]
fn an_empty_batch_round_trips() {
    let batch = CandleBatch::from_candles(&[]);
    assert!(batch.is_empty());
    assert_eq!(batch.len(), 0);
    assert!(batch.to_candles().is_empty());
    assert_eq!(batch, CandleBatch::default());
    let from_columns = CandleBatch::from_columns(CandleColumns::default()).expect("empty columns");
    assert!(from_columns.is_empty());
}

/// Adds one row to a single column.
type Lengthen = fn(&mut CandleColumns);

#[test]
fn from_columns_rejects_a_column_of_a_different_length() {
    let full = CandleBatch::from_candles(&candles()).into_columns();
    let rows = full.open_time_ms.len();
    let lengthen: [(&str, Lengthen); 7] = [
        ("close_time_ms", |c| c.close_time_ms.push(0)),
        ("open", |c| c.open.push(Decimal::ONE)),
        ("high", |c| c.high.push(Decimal::ONE)),
        ("low", |c| c.low.push(Decimal::ONE)),
        ("close", |c| c.close.push(Decimal::ONE)),
        ("volume", |c| c.volume.push(Decimal::ONE)),
        ("trades", |c| c.trades.push(None)),
    ];
    for (column, change) in lengthen {
        let mut columns = full.clone();
        change(&mut columns);
        let err = CandleBatch::from_columns(columns).expect_err("mismatched lengths");
        assert_eq!(
            err.to_string(),
            format!(
                "candle column `{column}` has {} rows, expected {rows}",
                rows + 1
            )
        );
    }

    // A short reference column is reported against the first column after it.
    let mut columns = full;
    columns.open_time_ms.pop();
    let err = CandleBatch::from_columns(columns).expect_err("mismatched lengths");
    assert_eq!(
        err.to_string(),
        format!(
            "candle column `close_time_ms` has {rows} rows, expected {}",
            rows - 1
        )
    );
}
