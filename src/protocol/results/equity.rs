//! The `equity` table: one row per bar.

use super::super::ExactDecimal;
use super::arrow::{self, Column, Layout, Text};
use super::{ColumnType, MAX_ROWS, ResultError, exactly, reader, refused};

const RESULT: &str = "equity";

/// The columns of `equity.arrow`, in order: each one's name and Arrow type.
/// `equity` holds decimals as text.
pub const EQUITY_COLUMNS: [(&str, ColumnType); 2] = [
    ("open_time_ms", ColumnType::Int64),
    ("equity", ColumnType::Utf8),
];

const LAYOUT: Layout = Layout {
    name: RESULT,
    columns: &EQUITY_COLUMNS,
};

/// Equity at one bar's close: a row of `equity.arrow`, its fields the
/// table's columns in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EquityRow {
    /// The bar's open time, which names the bar.
    pub open_time_ms: i64,
    /// Cash plus the position valued at the bar's close, after that bar's
    /// fill, in the quote asset.
    pub equity: ExactDecimal,
}

/// The bytes of `equity.arrow` for `curve`, in the order given.
///
/// ```
/// use quantforge::protocol::results::{EquityRow, decode_equity, encode_equity};
///
/// let curve = [
///     EquityRow { open_time_ms: 1792454400000, equity: "10000".parse()? },
///     EquityRow { open_time_ms: 1792454460000, equity: "10012.50".parse()? },
/// ];
/// let file = encode_equity(&curve)?;
/// let read = decode_equity(&file)?;
/// assert_eq!(read, curve);
/// assert_eq!(read[1].equity.to_string(), "10012.50");
/// assert_eq!(encode_equity(&read)?, file);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn encode_equity(curve: &[EquityRow]) -> Result<Vec<u8>, ResultError> {
    encode_within(curve, MAX_ROWS)
}

/// [`encode_equity`] for a table that holds at most `most` rows.
fn encode_within(curve: &[EquityRow], most: usize) -> Result<Vec<u8>, ResultError> {
    if curve.len() > most {
        return Err(ResultError::TooManyRows {
            result: RESULT,
            rows: curve.len(),
            most,
        });
    }
    let mut equity = Text::default();
    for row in curve {
        if !equity.push(&row.equity.to_string()) {
            return Err(ResultError::ColumnTooLarge {
                result: RESULT,
                column: "equity",
            });
        }
    }
    let columns = [
        Column::Int64(curve.iter().map(|row| row.open_time_ms).collect()),
        Column::Utf8(equity),
    ];
    Ok(arrow::write(&LAYOUT, curve.len(), &columns))
}

/// The curve of an `equity.arrow`, in its order. Refuses bytes that are not
/// exactly what [`encode_equity`] writes for it.
pub fn decode_equity(file: &[u8]) -> Result<Vec<EquityRow>, ResultError> {
    let (rows, columns) = reader::read(&LAYOUT, file).map_err(|reason| refused(RESULT, reason))?;
    let [Column::Int64(open_time_ms), Column::Utf8(equity)] = columns.as_slice() else {
        return Err(refused(RESULT, "its columns are not the table's"));
    };
    let missing = || refused(RESULT, "a column is shorter than the table");
    let mut curve = Vec::with_capacity(rows);
    for row in 0..rows {
        let text = equity.get(row).ok_or_else(missing)?;
        curve.push(EquityRow {
            open_time_ms: *open_time_ms.get(row).ok_or_else(missing)?,
            equity: text
                .parse()
                .map_err(|failure| refused(RESULT, format!("equity in row {row}: {failure}")))?,
        });
    }
    exactly(RESULT, file, curve, |curve| encode_equity(curve))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(equities: &[&str]) -> Vec<EquityRow> {
        equities
            .iter()
            .zip(0..)
            .map(|(equity, bar)| EquityRow {
                open_time_ms: 1_792_454_400_000 + bar * 60_000,
                equity: equity.parse().expect("an amount"),
            })
            .collect()
    }

    fn texts(curve: &[EquityRow]) -> Vec<String> {
        curve
            .iter()
            .map(|row| format!("{} {}", row.open_time_ms, row.equity))
            .collect()
    }

    #[test]
    fn a_curve_reads_back_digit_for_digit_and_writes_the_same_file_again() {
        let written = curve(&[
            "10000",
            "10000.00",
            "9999.9900000000000000000000",
            "0",
            "0.000",
            "-12.5",
            "79228162514264337593543950335",
        ]);
        let file = encode_equity(&written).expect("a file");
        let read = decode_equity(&file).expect("the curve");
        assert_eq!(texts(&read), texts(&written));
        assert_eq!(encode_equity(&read).expect("a file"), file);
    }

    // The rows are counted before any is read, so a table that holds two
    // shows the refusal without the memory `MAX_ROWS + 1` rows would take.
    #[test]
    fn more_rows_than_a_table_holds_are_refused() {
        let written = curve(&["1", "2", "3"]);
        assert_eq!(
            encode_within(&written, 2).expect_err("too many"),
            ResultError::TooManyRows {
                result: "equity",
                rows: 3,
                most: 2
            }
        );
        assert_eq!(
            encode_within(&written[..2], 2),
            encode_equity(&written[..2])
        );
        assert_eq!(encode_within(&written, MAX_ROWS), encode_equity(&written));
    }

    #[test]
    fn the_same_curve_is_the_same_file_and_another_curve_another() {
        let first = encode_equity(&curve(&["1", "2"])).expect("a file");
        assert_eq!(first, encode_equity(&curve(&["1", "2"])).expect("a file"));
        // `2.0` is the number `2`, and another text.
        assert_ne!(first, encode_equity(&curve(&["1", "2.0"])).expect("a file"));
    }

    #[test]
    fn a_long_curve_reads_back() {
        let written: Vec<EquityRow> = (0..20_000_i64)
            .map(|bar| EquityRow {
                open_time_ms: bar * 60_000,
                equity: format!("{}.{:02}", 10_000 + bar, bar % 100)
                    .parse()
                    .expect("an amount"),
            })
            .collect();
        let file = encode_equity(&written).expect("a file");
        assert_eq!(
            texts(&decode_equity(&file).expect("the curve")),
            texts(&written)
        );
    }

    #[test]
    fn a_file_that_is_not_exactly_the_one_written_is_refused() {
        let file = encode_equity(&curve(&["10000.5", "10001"])).expect("a file");
        let trades = super::super::encode_trades(&[]).expect("a file");
        assert_eq!(
            decode_equity(&trades)
                .expect_err("another table")
                .to_string(),
            "not a format 1 equity file: it does not start with the equity schema"
        );
        assert!(decode_equity(&[]).is_err());
        assert!(decode_equity(b"ARROW1\0\0").is_err());
        assert!(
            decode_equity(&file[..file.len() - 8]).is_err(),
            "no footer end"
        );

        // `10000.5` with a sign it may not carry: `+0000.5`.
        let at = file
            .windows(7)
            .position(|found| found == b"10000.5")
            .expect("the amount");
        let mut signed = file.clone();
        signed[at] = b'+';
        let refusal = decode_equity(&signed).expect_err("a sign");
        assert!(
            refusal
                .to_string()
                .starts_with("not a format 1 equity file: equity in row 0: invalid decimal"),
            "{refusal}"
        );
    }

    #[test]
    fn any_one_byte_changed_is_refused_or_reads_as_another_curve() {
        let written = curve(&["10000.5", "-3"]);
        let file = encode_equity(&written).expect("a file");
        for at in 0..file.len() {
            for flip in [0x01, 0x80] {
                let mut altered = file.clone();
                altered[at] ^= flip;
                match decode_equity(&altered) {
                    Ok(read) => {
                        assert_ne!(texts(&read), texts(&written), "byte {at}");
                        assert_eq!(encode_equity(&read).expect("a file"), altered, "byte {at}");
                    }
                    Err(ResultError::NotCanonical { result, .. }) => assert_eq!(result, "equity"),
                    Err(other) => panic!("byte {at}: {other}"),
                }
            }
        }
    }
}
