//! A `/v1/candles` query read into a market and a range, every value in its
//! one canonical form, before the store is asked anything. A spelling the
//! protocol does not write is refused, never corrected.

use std::str::FromStr;

use super::refusal::Refusal;
use crate::model::{Category, Interval, Symbol, TimestampMs, Venue};
use crate::protocol::segments::CandleRangeQuery;

/// The protocol's bound on every integer: JavaScript reads anything below
/// it exactly.
const INTEGER_BOUND: i64 = 1 << 53;

/// A checked query: the market, with its category unless the query left it
/// out, and the range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub(super) venue: Venue,
    pub(super) category: Option<Category>,
    pub(super) symbol: Symbol,
    pub(super) interval: Interval,
    pub(super) start_open_ms: TimestampMs,
    pub(super) end_open_ms: TimestampMs,
}

impl Selection {
    /// Reads `query`, refusing a value not in its canonical form, a range
    /// that starts after it ends, and one spanning more than `max_span_bars`
    /// bars.
    pub(super) fn read(query: CandleRangeQuery, max_span_bars: u64) -> Result<Self, Refusal> {
        let venue = Venue::ALL
            .into_iter()
            .find(|venue| venue.as_str() == query.exchange)
            .ok_or_else(|| {
                let names: Vec<&str> = Venue::ALL.iter().map(|venue| venue.as_str()).collect();
                Refusal::invalid(format!(
                    "exchange {:?} is not one QuantForge names; write one of {}",
                    query.exchange,
                    names.join(", ")
                ))
            })?;
        let category = match &query.category {
            None => None,
            Some(text) => Some(
                Category::ALL
                    .into_iter()
                    .find(|category| category.as_str() == text)
                    .ok_or_else(|| {
                        Refusal::invalid(format!(
                            "category {text:?} is not spot, linear or inverse"
                        ))
                    })?,
            ),
        };
        let symbol = match Symbol::new(query.symbol.as_str()) {
            Ok(symbol) if symbol.as_str() == query.symbol => symbol,
            Ok(symbol) => {
                return Err(Refusal::invalid(format!(
                    "symbol {:?} is not in its canonical form; write {:?}",
                    query.symbol,
                    symbol.as_str()
                )));
            }
            Err(_) => return Err(Refusal::invalid("symbol is empty")),
        };
        let interval = Interval::from_str(&query.interval).map_err(|_| {
            Refusal::invalid(format!(
                "interval {:?} is not one of QuantForge's intervals, such as 1m or 1h",
                query.interval
            ))
        })?;
        for (name, value) in [
            ("start_open_ms", query.start_open_ms),
            ("end_open_ms", query.end_open_ms),
        ] {
            if value.unsigned_abs() >= INTEGER_BOUND.unsigned_abs() {
                return Err(Refusal::invalid(format!(
                    "{name} {value} is not below 2^53 in magnitude"
                )));
            }
        }
        let (start_open_ms, end_open_ms) = (query.start_open_ms, query.end_open_ms);
        if start_open_ms > end_open_ms {
            return Err(Refusal::invalid(format!(
                "start_open_ms {start_open_ms} is after end_open_ms {end_open_ms}"
            )));
        }
        // Both bounds lie within 2^53 of zero, so neither step overflows.
        let bars = (end_open_ms - start_open_ms) / interval.step_ms() + 1;
        if bars.unsigned_abs() > max_span_bars {
            return Err(Refusal::invalid(format!(
                "the range spans {bars} bars of {}, more than the {max_span_bars} this server \
                 serves in one answer; ask for a shorter range",
                interval.as_str()
            )));
        }
        Ok(Self {
            venue,
            category,
            symbol,
            interval,
            start_open_ms,
            end_open_ms,
        })
    }
}
