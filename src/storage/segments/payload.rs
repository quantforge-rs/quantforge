//! The payload of segment format 1: a window's candles as columns, every
//! integer little-endian, every decimal its exact text.
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 8 | `QFSEGMNT` |
//! | 4 | the format, `1` |
//! | 4 | the candle count `n`, 1 to 1,440 |
//! | 8 × n | open times, rising |
//! | 8 × n | close times |
//! | 8 × n | trade counts, `0` where unknown |
//! | n | `1` where the trade count is known, else `0` |
//! | n, then their sum | for each of open, high, low, close and volume: the text lengths, then the texts |

use rust_decimal::Decimal;

use super::{FORMAT, MAX_DECIMAL_LEN, SegmentError, WINDOW_BARS};
use crate::{Candle, CandleBatch, CandleColumns, TimestampMs};

const MAGIC: &[u8; 8] = b"QFSEGMNT";
const DECIMALS: [&str; 5] = ["open", "high", "low", "close", "volume"];

/// A candle with its decimals as written text: open, high, low, close and
/// volume, in that order. The SQLite store keeps candles this way and packs
/// the text as it is, never reading it as a number in between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextCandle {
    pub(crate) open_time_ms: TimestampMs,
    pub(crate) close_time_ms: TimestampMs,
    pub(crate) decimals: [String; 5],
    pub(crate) trades: Option<u64>,
}

/// The payload of 1 to [`WINDOW_BARS`] candles with rising open times. Each
/// decimal is written at its own scale, and a negative zero as zero.
pub fn encode(candles: &[Candle]) -> Result<Vec<u8>, SegmentError> {
    let candles: Vec<TextCandle> = candles
        .iter()
        .map(|candle| TextCandle {
            open_time_ms: candle.open_time_ms,
            close_time_ms: candle.close_time_ms,
            decimals: [
                candle.open,
                candle.high,
                candle.low,
                candle.close,
                candle.volume,
            ]
            .map(written),
            trades: candle.trades,
        })
        .collect();
    encode_text(&candles)
}

/// A decimal's text: its digits at its scale, with no sign on zero.
fn written(mut value: Decimal) -> String {
    if value.is_zero() {
        value.set_sign_positive(true);
    }
    value.to_string()
}

/// [`encode`] over decimals that are already text. Text that is not a
/// decimal in its exact form is refused, not corrected.
pub(crate) fn encode_text(candles: &[TextCandle]) -> Result<Vec<u8>, SegmentError> {
    let count = u32::try_from(candles.len())
        .ok()
        .filter(|count| (1..=WINDOW_BARS).contains(count))
        .ok_or(SegmentError::BarCount {
            count: candles.len(),
        })?;
    for pair in candles.windows(2) {
        if let [previous, next] = pair {
            if next.open_time_ms <= previous.open_time_ms {
                return Err(SegmentError::OutOfOrder {
                    previous_ms: previous.open_time_ms,
                    open_time_ms: next.open_time_ms,
                });
            }
        }
    }

    let mut payload = Vec::with_capacity(16 + candles.len() * 64);
    payload.extend_from_slice(MAGIC);
    payload.extend_from_slice(&FORMAT.to_le_bytes());
    payload.extend_from_slice(&count.to_le_bytes());
    for candle in candles {
        payload.extend_from_slice(&candle.open_time_ms.to_le_bytes());
    }
    for candle in candles {
        payload.extend_from_slice(&candle.close_time_ms.to_le_bytes());
    }
    for candle in candles {
        payload.extend_from_slice(&candle.trades.unwrap_or(0).to_le_bytes());
    }
    payload.extend(
        candles
            .iter()
            .map(|candle| u8::from(candle.trades.is_some())),
    );
    for (index, column) in DECIMALS.into_iter().enumerate() {
        let mut texts = Vec::new();
        for candle in candles {
            let text = candle.decimals.get(index).map_or("", String::as_str);
            let length = u8::try_from(text.len())
                .ok()
                .filter(|_| is_exact(text))
                .ok_or_else(|| SegmentError::InvalidDecimal {
                    column,
                    open_time_ms: candle.open_time_ms,
                    text: text.to_string(),
                })?;
            payload.push(length);
            texts.extend_from_slice(text.as_bytes());
        }
        payload.extend_from_slice(&texts);
    }
    Ok(payload)
}

/// The candles of a payload. Bytes that [`encode`] would not have written
/// are refused, so that every set of candles has exactly one payload.
pub fn decode(payload: &[u8]) -> Result<Vec<Candle>, SegmentError> {
    let mut bytes = Reader { rest: payload };
    if bytes.take(MAGIC.len())? != MAGIC {
        return Err(SegmentError::Malformed {
            reason: "it does not start with QFSEGMNT",
        });
    }
    let format = bytes.u32()?;
    if format != FORMAT {
        return Err(SegmentError::UnsupportedFormat(format));
    }
    let count = bytes.u32()?;
    if !(1..=WINDOW_BARS).contains(&count) {
        return Err(SegmentError::BarCount {
            count: count as usize,
        });
    }
    let count = count as usize;

    let open_time_ms = bytes
        .words(count)?
        .map(i64::from_le_bytes)
        .collect::<Vec<_>>();
    for pair in open_time_ms.windows(2) {
        if let [previous, next] = *pair {
            if next <= previous {
                return Err(SegmentError::OutOfOrder {
                    previous_ms: previous,
                    open_time_ms: next,
                });
            }
        }
    }
    let close_time_ms = bytes.words(count)?.map(i64::from_le_bytes).collect();
    let counts = bytes.words(count)?.map(u64::from_le_bytes);
    let trades = bytes
        .take(count)?
        .iter()
        .zip(counts)
        .map(|(known, trades)| match (known, trades) {
            (1, trades) => Ok(Some(trades)),
            (0, 0) => Ok(None),
            (0, _) => Err(SegmentError::Malformed {
                reason: "a trade count is set where it is marked unknown",
            }),
            _ => Err(SegmentError::Malformed {
                reason: "a trade count's flag is neither 0 nor 1",
            }),
        })
        .collect::<Result<_, _>>()?;

    let mut columns: [Vec<Decimal>; 5] = Default::default();
    for (column, values) in DECIMALS.into_iter().zip(columns.iter_mut()) {
        let lengths = bytes.take(count)?;
        for (length, open_time_ms) in lengths.iter().zip(&open_time_ms) {
            let text = bytes.take(usize::from(*length))?;
            let refused = || SegmentError::InvalidDecimal {
                column,
                open_time_ms: *open_time_ms,
                text: String::from_utf8_lossy(text).into_owned(),
            };
            let text = std::str::from_utf8(text)
                .ok()
                .filter(|text| is_exact(text))
                .ok_or_else(refused)?;
            values.push(Decimal::from_str_exact(text).map_err(|_| refused())?);
        }
    }
    if !bytes.rest.is_empty() {
        return Err(SegmentError::Malformed {
            reason: "bytes follow the last column",
        });
    }

    let [open, high, low, close, volume] = columns;
    let batch = CandleBatch::from_columns(CandleColumns {
        open_time_ms,
        close_time_ms,
        open,
        high,
        low,
        close,
        volume,
        trades,
    })
    .map_err(|_| SegmentError::Malformed {
        reason: "its columns differ in length",
    })?;
    Ok(batch.to_candles())
}

/// Whether `text` is a decimal in its one written form: an optional minus,
/// the integer digits without leading zeros, an optional fraction, no
/// minus on zero, and a value a decimal holds without rounding — at most 28
/// fraction digits and a magnitude below 2^96.
pub(crate) fn is_exact(text: &str) -> bool {
    text.len() <= MAX_DECIMAL_LEN
        && is_exact_form(text)
        // The form excludes the `+`, `_`, and bare point the parser takes;
        // the round trip excludes what it would round.
        && Decimal::from_str_exact(text).is_ok_and(|value| value.to_string() == text)
}

/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, with no minus sign on zero.
fn is_exact_form(text: &str) -> bool {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let well_formed = digits(integer)
        && (integer == "0" || !integer.starts_with('0'))
        && fraction.is_none_or(digits);
    let zero = unsigned.bytes().all(|byte| byte == b'0' || byte == b'.');
    well_formed && !(negative && zero)
}

/// The unread rest of a payload.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], SegmentError> {
        let (head, rest) = self
            .rest
            .split_at_checked(len)
            .ok_or(SegmentError::Malformed {
                reason: "it ends early",
            })?;
        self.rest = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, SegmentError> {
        let word = self.take(4)?;
        <[u8; 4]>::try_from(word)
            .map(u32::from_le_bytes)
            .map_err(|_| SegmentError::Malformed {
                reason: "it ends early",
            })
    }

    /// `count` little-endian words of `N` bytes each.
    fn words<const N: usize>(
        &mut self,
        count: usize,
    ) -> Result<impl Iterator<Item = [u8; N]> + 'a, SegmentError> {
        let bytes = self.take(count * N)?;
        Ok(bytes
            .chunks_exact(N)
            .filter_map(|chunk| <[u8; N]>::try_from(chunk).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_decimal_in_its_exact_form_is_exact() {
        for text in [
            "0",
            "0.00000000",
            "42000.01000000",
            "-1.5",
            "79228162514264337593543950335",
            "-7.9228162514264337593543950335",
            "0.0000000000000000000000000001",
        ] {
            assert!(is_exact(text), "{text}");
        }
        for text in [
            "",
            "-",
            "01",
            "+1",
            "1.",
            ".5",
            "-0",
            "-0.00",
            "1e5",
            "1_000",
            " 1",
            "1 ",
            "0.00000000000000000000000000001",
            "79228162514264337593543950336",
            "١",
        ] {
            assert!(!is_exact(text), "{text:?}");
        }
    }

    #[test]
    fn text_and_decimals_encode_to_the_same_payload() {
        let candle = Candle {
            open_time_ms: 60_000,
            close_time_ms: 119_999,
            open: "1.10".parse().expect("open"),
            high: "2".parse().expect("high"),
            low: "-0.5".parse().expect("low"),
            close: "0.000".parse().expect("close"),
            volume: "12.04510000".parse().expect("volume"),
            trades: Some(0),
        };
        let text = TextCandle {
            open_time_ms: 60_000,
            close_time_ms: 119_999,
            decimals: ["1.10", "2", "-0.5", "0.000", "12.04510000"].map(String::from),
            trades: Some(0),
        };
        let payload = encode(std::slice::from_ref(&candle)).expect("payload");
        assert_eq!(encode_text(&[text]).expect("payload of text"), payload);
        assert_eq!(decode(&payload).expect("candles"), vec![candle]);
    }

    #[test]
    fn a_negative_zero_is_written_as_zero() {
        let mut zero = Decimal::new(0, 2);
        zero.set_sign_negative(true);
        assert_eq!(zero.to_string(), "-0.00");
        assert_eq!(written(zero), "0.00");
    }
}
