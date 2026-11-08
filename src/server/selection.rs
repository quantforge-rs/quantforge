//! A `/v1/candles` query or a job's data selection read into a market and a
//! range, every value in its one canonical form, before the store is asked
//! anything; and the category a selection without one reads. A spelling the
//! protocol does not write is refused, never corrected.

use std::str::FromStr;

use super::refusal::Refusal;
use super::store::ServedStore;
use crate::model::{Category, ExchangeId, Interval, MarketId, Symbol, TimestampMs, Venue};
use crate::protocol::jobs::DataSelection;
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
        let selection = Self::read_parts(Parts {
            exchange: &query.exchange,
            category: query.category.as_deref(),
            symbol: &query.symbol,
            interval: &query.interval,
            start_open_ms: query.start_open_ms,
            end_open_ms: query.end_open_ms,
        })?;
        let bars = selection.bars();
        if bars > max_span_bars {
            return Err(Refusal::invalid(format!(
                "the range spans {bars} bars of {}, more than the {max_span_bars} this server \
                 serves in one answer; ask for a shorter range",
                selection.interval.as_str()
            )));
        }
        Ok(selection)
    }

    /// Reads a job's data selection, as a `/v1/candles` query is read but
    /// with no limit on its span.
    pub(super) fn read_data(data: &DataSelection) -> Result<Self, Refusal> {
        Self::read_parts(Parts {
            exchange: &data.exchange,
            category: data.category.as_deref(),
            symbol: &data.symbol,
            interval: &data.interval,
            start_open_ms: data.start_open_ms,
            end_open_ms: data.end_open_ms,
        })
    }

    fn read_parts(query: Parts<'_>) -> Result<Self, Refusal> {
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
        let category = match query.category {
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
        let symbol = match Symbol::new(query.symbol) {
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
        let interval = Interval::from_str(query.interval).map_err(|_| {
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
        Ok(Self {
            venue,
            category,
            symbol,
            interval,
            start_open_ms,
            end_open_ms,
        })
    }

    /// How many bars of its interval the range spans.
    pub(super) fn bars(&self) -> u64 {
        // Both bounds lie within 2^53 of zero, and the start is not after
        // the end, so no step overflows.
        let span = (self.end_open_ms - self.start_open_ms) / self.interval.step_ms();
        span.unsigned_abs() + 1
    }

    /// The market of `category`.
    pub(super) fn market(&self, category: Category) -> MarketId {
        MarketId::new(
            ExchangeId::from_parts(self.venue, category),
            self.symbol.clone(),
            self.interval,
        )
    }

    /// The category the selection names, or the one a selection without one
    /// reads: Spot, the only family before markets carried one, unless
    /// another family of the venue holds the symbol and interval, which
    /// makes the selection ambiguous.
    pub(super) fn category(&self, store: &dyn ServedStore) -> Result<Category, Refusal> {
        if let Some(category) = self.category {
            return Ok(category);
        }
        let mut held = Vec::new();
        for category in Category::ALL {
            let newest = store
                .max_open_time_ms(&self.market(category))
                .map_err(|failure| Refusal::storage("finding a selection's category", failure))?;
            if newest.is_some() {
                held.push(category.as_str());
            }
        }
        if held
            .iter()
            .all(|&category| category == Category::Spot.as_str())
        {
            return Ok(Category::Spot);
        }
        Err(Refusal::invalid(format!(
            "{} stores {} {} as {}; a market without category reads spot only when no other \
             family holds the symbol and interval, so name the category",
            self.venue.as_str(),
            self.symbol.as_str(),
            self.interval.as_str(),
            listed(&held)
        )))
    }
}

/// A selection's values, from whichever body carried them.
struct Parts<'a> {
    exchange: &'a str,
    category: Option<&'a str>,
    symbol: &'a str,
    interval: &'a str,
    start_open_ms: TimestampMs,
    end_open_ms: TimestampMs,
}

/// A market as people read it: venue, category, symbol and interval.
pub(super) fn named(market: &MarketId) -> String {
    format!(
        "{} {} {} {}",
        market.exchange.venue().as_str(),
        market.category().as_str(),
        market.symbol.as_str(),
        market.interval.as_str()
    )
}

/// `a`, `a and b`, or `a, b and c`.
fn listed(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.join(""),
    }
}
