//! The `trades` table: one row per closed trade.

use super::super::ExactDecimal;
use super::arrow::{self, Column, Layout, Text};
use super::{ColumnType, MAX_ROWS, ResultError, exactly, reader, refused};

const RESULT: &str = "trades";

/// The columns of `trades.arrow`, in order: each one's name and Arrow type.
/// The four `Utf8` columns after the times hold decimals as text.
pub const TRADES_COLUMNS: [(&str, ColumnType); 7] = [
    ("symbol", ColumnType::Utf8),
    ("entry_time_ms", ColumnType::Int64),
    ("exit_time_ms", ColumnType::Int64),
    ("entry_price", ColumnType::Utf8),
    ("exit_price", ColumnType::Utf8),
    ("qty", ColumnType::Utf8),
    ("gross_quote_pnl", ColumnType::Utf8),
];

const LAYOUT: Layout = Layout {
    name: RESULT,
    columns: &TRADES_COLUMNS,
};

/// One closed trade: a row of `trades.arrow`, its fields the table's
/// columns in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeRow {
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// When the entry filled: the open time of the bar it filled at.
    pub entry_time_ms: i64,
    /// When the exit filled: the open time of the bar it filled at, or the
    /// last bar's close time for a position closed out at the end.
    pub exit_time_ms: i64,
    /// The entry's fill price.
    pub entry_price: ExactDecimal,
    /// The exit's fill price.
    pub exit_price: ExactDecimal,
    /// The quantity bought and sold, in the base asset.
    pub qty: ExactDecimal,
    /// The change in cash from before the entry to after the exit, in the
    /// quote asset: both fees are already deducted.
    pub gross_quote_pnl: ExactDecimal,
}

/// The bytes of `trades.arrow` for `trades`, in the order given.
///
/// ```
/// use quantforge::protocol::results::{TradeRow, decode_trades, encode_trades};
///
/// let trade = TradeRow {
///     symbol: "BTCUSDT".to_string(),
///     entry_time_ms: 1792454460000,
///     exit_time_ms: 1792454580000,
///     entry_price: "81940.01000000".parse()?,
///     exit_price: "82010.50".parse()?,
///     qty: "0.12203".parse()?,
///     gross_quote_pnl: "-11.4".parse()?,
/// };
/// let file = encode_trades(std::slice::from_ref(&trade))?;
/// assert_eq!(&file[..6], b"ARROW1");
///
/// let read = decode_trades(&file)?;
/// assert_eq!(read, [trade]);
/// // Every decimal keeps its digits, trailing zeros included.
/// assert_eq!(read[0].entry_price.to_string(), "81940.01000000");
/// assert_eq!(encode_trades(&read)?, file);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn encode_trades(trades: &[TradeRow]) -> Result<Vec<u8>, ResultError> {
    encode_within(trades, MAX_ROWS)
}

/// [`encode_trades`] for a table that holds at most `most` rows.
fn encode_within(trades: &[TradeRow], most: usize) -> Result<Vec<u8>, ResultError> {
    if trades.len() > most {
        return Err(ResultError::TooManyRows {
            result: RESULT,
            rows: trades.len(),
            most,
        });
    }
    let text = |column: &'static str, value: &dyn Fn(&TradeRow) -> String| {
        let mut values = Text::default();
        for trade in trades {
            if !values.push(&value(trade)) {
                return Err(ResultError::ColumnTooLarge {
                    result: RESULT,
                    column,
                });
            }
        }
        Ok(Column::Utf8(values))
    };
    let columns = [
        text("symbol", &|trade| trade.symbol.clone())?,
        Column::Int64(trades.iter().map(|trade| trade.entry_time_ms).collect()),
        Column::Int64(trades.iter().map(|trade| trade.exit_time_ms).collect()),
        text("entry_price", &|trade| trade.entry_price.to_string())?,
        text("exit_price", &|trade| trade.exit_price.to_string())?,
        text("qty", &|trade| trade.qty.to_string())?,
        text("gross_quote_pnl", &|trade| {
            trade.gross_quote_pnl.to_string()
        })?,
    ];
    Ok(arrow::write(&LAYOUT, trades.len(), &columns))
}

/// The trades of a `trades.arrow`, in its order. Refuses bytes that are not
/// exactly what [`encode_trades`] writes for them.
pub fn decode_trades(file: &[u8]) -> Result<Vec<TradeRow>, ResultError> {
    let (rows, columns) = reader::read(&LAYOUT, file).map_err(|reason| refused(RESULT, reason))?;
    let [
        Column::Utf8(symbol),
        Column::Int64(entry_time_ms),
        Column::Int64(exit_time_ms),
        Column::Utf8(entry_price),
        Column::Utf8(exit_price),
        Column::Utf8(qty),
        Column::Utf8(gross_quote_pnl),
    ] = columns.as_slice()
    else {
        return Err(refused(RESULT, "its columns are not the table's"));
    };
    let missing = || refused(RESULT, "a column is shorter than the table");
    let decimal = |column: &'static str, values: &Text, row: usize| {
        let text = values.get(row).ok_or_else(missing)?;
        text.parse::<ExactDecimal>()
            .map_err(|failure| refused(RESULT, format!("{column} in row {row}: {failure}")))
    };
    let mut trades = Vec::with_capacity(rows);
    for row in 0..rows {
        trades.push(TradeRow {
            symbol: symbol.get(row).ok_or_else(missing)?.to_string(),
            entry_time_ms: *entry_time_ms.get(row).ok_or_else(missing)?,
            exit_time_ms: *exit_time_ms.get(row).ok_or_else(missing)?,
            entry_price: decimal("entry_price", entry_price, row)?,
            exit_price: decimal("exit_price", exit_price, row)?,
            qty: decimal("qty", qty, row)?,
            gross_quote_pnl: decimal("gross_quote_pnl", gross_quote_pnl, row)?,
        });
    }
    exactly(RESULT, file, trades, |trades| encode_trades(trades))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(index: i64, price: &str, pnl: &str) -> TradeRow {
        TradeRow {
            symbol: "BTCUSDT".to_string(),
            entry_time_ms: index * 60_000,
            exit_time_ms: index * 60_000 + 59_999,
            entry_price: price.parse().expect("a price"),
            exit_price: "0.0000000000000000000000000001".parse().expect("a price"),
            qty: "79228162514264337593543950335".parse().expect("a quantity"),
            gross_quote_pnl: pnl.parse().expect("an amount"),
        }
    }

    /// Every field of every trade as text: equal texts mean equal digits,
    /// where `==` on decimals would take `1.10` for `1.1`.
    fn texts(trades: &[TradeRow]) -> Vec<String> {
        trades
            .iter()
            .map(|trade| {
                format!(
                    "{} {} {} {} {} {} {}",
                    trade.symbol,
                    trade.entry_time_ms,
                    trade.exit_time_ms,
                    trade.entry_price,
                    trade.exit_price,
                    trade.qty,
                    trade.gross_quote_pnl
                )
            })
            .collect()
    }

    #[test]
    fn trades_read_back_digit_for_digit_and_write_the_same_file_again() {
        let trades = vec![
            trade(1, "81940.01000000", "-0.50"),
            trade(2, "1.10", "0"),
            trade(-3, "1.1", "-79228162514264337593543950335"),
        ];
        let file = encode_trades(&trades).expect("a file");
        let read = decode_trades(&file).expect("the trades");
        assert_eq!(texts(&read), texts(&trades));
        assert_eq!(read[1].entry_price.to_string(), "1.10");
        assert_eq!(read[2].entry_price.to_string(), "1.1");
        assert_eq!(encode_trades(&read).expect("a file"), file);
    }

    #[test]
    fn no_trades_are_a_file_with_one_empty_record_batch() {
        let file = encode_trades(&[]).expect("a file");
        assert!(decode_trades(&file).expect("no trades").is_empty());
        assert_ne!(
            file,
            super::super::encode_equity(&[]).expect("a file"),
            "each table has a schema of its own"
        );
    }

    #[test]
    fn a_symbol_is_any_text() {
        let mut odd = trade(1, "1", "1");
        odd.symbol = "币安人生USDT".to_string();
        let mut empty = trade(2, "1", "1");
        empty.symbol = String::new();
        let trades = vec![odd, empty];
        let file = encode_trades(&trades).expect("a file");
        assert_eq!(decode_trades(&file).expect("the trades"), trades);
    }

    #[test]
    fn a_file_that_is_not_exactly_the_one_written_is_refused() {
        let trades = vec![trade(1, "81940.01", "5"), trade(2, "81950", "-5")];
        let file = encode_trades(&trades).expect("a file");

        let equity = super::super::encode_equity(&[]).expect("a file");
        assert_eq!(
            decode_trades(&equity)
                .expect_err("another table")
                .to_string(),
            "not a format 1 trades file: it does not start with the trades schema"
        );
        assert!(decode_trades(&file[..file.len() - 1]).is_err(), "cut short");
        let mut longer = file.clone();
        longer.push(0);
        assert!(decode_trades(&longer).is_err(), "a byte too many");

        // A price whose text is not an exact decimal: `81940.01` becomes
        // `81940.0e`, in the body and nowhere else.
        let at = file
            .windows(8)
            .position(|found| found == b"81940.01")
            .expect("the price");
        let mut inexact = file.clone();
        inexact[at + 7] = b'e';
        let refusal = decode_trades(&inexact).expect_err("an inexact price");
        assert!(
            refusal
                .to_string()
                .starts_with("not a format 1 trades file: entry_price in row 0: invalid decimal"),
            "{refusal}"
        );

        // Padding that is not zero reads as the same rows, and is refused.
        let padding = at + "81940.0181950".len();
        assert_eq!(file[padding], 0);
        let mut padded = file.clone();
        padded[padding] = 1;
        assert_eq!(
            decode_trades(&padded).expect_err("padding").to_string(),
            "not a format 1 trades file: it is not the file its contents are written as"
        );
    }

    #[test]
    fn any_one_byte_changed_is_refused_or_reads_as_other_trades() {
        let trades = vec![trade(1, "81940.01", "5")];
        let file = encode_trades(&trades).expect("a file");
        for at in 0..file.len() {
            let mut altered = file.clone();
            altered[at] ^= 0x01;
            match decode_trades(&altered) {
                Ok(read) => {
                    assert_ne!(texts(&read), texts(&trades), "byte {at}");
                    assert_eq!(encode_trades(&read).expect("a file"), altered, "byte {at}");
                }
                Err(ResultError::NotCanonical { result, .. }) => assert_eq!(result, "trades"),
                Err(other) => panic!("byte {at}: {other}"),
            }
        }
    }

    // The rows are counted before any is read, so a table that holds two
    // shows the refusal: `MAX_ROWS + 1` trades would take gigabytes to show
    // the same comparison.
    #[test]
    fn more_rows_than_a_table_holds_are_refused() {
        let trades: Vec<TradeRow> = (1..=3).map(|index| trade(index, "1", "1")).collect();
        assert_eq!(
            encode_within(&trades, 2).expect_err("too many"),
            ResultError::TooManyRows {
                result: "trades",
                rows: 3,
                most: 2
            }
        );
        // As many rows as a table holds are written, and as the file the
        // table always is: the limit is `MAX_ROWS` and changes no byte.
        assert_eq!(encode_within(&trades[..2], 2), encode_trades(&trades[..2]));
        assert_eq!(encode_within(&trades, MAX_ROWS), encode_trades(&trades));
    }
}
