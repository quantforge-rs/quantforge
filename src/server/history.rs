//! The history routes: the markets the store holds, and one market's bars
//! in a range, read through the server's store and its turns.

use std::fmt;

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::{IntoResponse, Response};

use super::refusal::Refusal;
use super::selection::{Selection, named};
use super::store::{ServedStore, ServerStore};
use crate::model::{Candle, MarketCoverage, MarketId, SegmentRef, TimestampMs, ms_to_rfc3339};
use crate::ports::CandleQuery;
use crate::protocol::ContentHash;
use crate::protocol::segments::{
    CandleRange, CandleRangeQuery, MarketEntry, MarketIndex, SEGMENT_FORMAT, SegmentEntry,
    SegmentRange,
};
use crate::protocol::stream::Bar;

/// The most bars one `/v1/candles` answer spans when nothing sets it: a
/// leap year of `1m` bars, 366 segments.
pub const DEFAULT_MAX_SPAN_BARS: u64 = 527_040;

/// The most bars one `/v1/candles` answer sends inline, in its tip, when
/// nothing sets it: 31 windows of 1,440 bars, a month of `1m` bars. A
/// healthy store's tip is its newest window; the rest is room for windows
/// waiting for a refresh, and the bound keeps a store that cannot pack them
/// from turning one answer into a hundred megabytes.
pub const DEFAULT_MAX_TIP_BARS: u64 = 44_640;

/// How much one `/v1/candles` answer may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    /// The most bars one `/v1/candles` answer may span.
    pub max_span_bars: u64,
    /// The most bars one `/v1/candles` answer sends in its tip, the bars not
    /// packed into segments yet. A range whose tip holds more is refused,
    /// never cut short.
    pub max_tip_bars: u64,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_span_bars: DEFAULT_MAX_SPAN_BARS,
            max_tip_bars: DEFAULT_MAX_TIP_BARS,
        }
    }
}

/// What the history routes read, and how much one answer may hold.
#[derive(Clone)]
pub struct History {
    store: ServerStore,
    limits: HistoryLimits,
}

impl History {
    /// The routes over the server's store.
    pub fn new(store: ServerStore, limits: HistoryLimits) -> Self {
        Self { store, limits }
    }

    /// The server's store.
    pub(super) fn store(&self) -> &ServerStore {
        &self.store
    }
}

impl fmt::Debug for History {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("History")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// An answer that changes as the store grows: a cache asks again each time.
pub(super) fn fresh<T: IntoResponse>(answer: T) -> Response {
    let mut response = answer.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `GET /v1/markets`.
pub(super) async fn markets(State(history): State<History>) -> Result<Response, Refusal> {
    let covered = history
        .store
        .call(|store| {
            store
                .market_coverage()
                .map_err(|failure| Refusal::storage("listing the markets", failure))
        })
        .await?;
    let mut markets: Vec<MarketEntry> = covered.iter().map(market_entry).collect();
    markets.sort_by(|a, b| {
        (&a.exchange, &a.category, &a.symbol, &a.interval).cmp(&(
            &b.exchange,
            &b.category,
            &b.symbol,
            &b.interval,
        ))
    });
    Ok(fresh(Json(MarketIndex { markets })))
}

fn market_entry(covered: &MarketCoverage) -> MarketEntry {
    let market = &covered.market;
    MarketEntry {
        exchange: market.exchange.venue().as_str().to_string(),
        category: market.category().as_str().to_string(),
        symbol: market.symbol.as_str().to_string(),
        interval: market.interval.as_str().to_string(),
        first_open_ms: covered.first_open_ms,
        last_open_ms: covered.last_open_ms,
    }
}

/// `GET /v1/candles`.
pub(super) async fn candles(
    State(history): State<History>,
    query: Result<Query<CandleRangeQuery>, QueryRejection>,
) -> Result<Response, Refusal> {
    let Query(query) = query.map_err(|rejection| {
        Refusal::invalid(format!(
            "the query string is not a /v1/candles query: {}",
            rejection.body_text()
        ))
    })?;
    let selection = Selection::read(query, history.limits.max_span_bars)?;
    let max_tip_bars = history.limits.max_tip_bars;
    let answer = history
        .store
        .call(move |store| answer(store, &selection, max_tip_bars))
        .await?;
    Ok(fresh(Json(answer)))
}

/// The range's segments and tip, as the store holds them, or a refusal of a
/// tip longer than `max_tip_bars`.
fn answer(
    store: &dyn ServedStore,
    selection: &Selection,
    max_tip_bars: u64,
) -> Result<CandleRange, Refusal> {
    let category = selection.category(store)?;
    let market = selection.market(category);
    let (start, end) = (selection.start_open_ms, selection.end_open_ms);
    let plan = store
        .resolve_segments(&market, start, end)
        .map_err(|failure| Refusal::storage("planning a range", failure))?;
    let tip = match plan.tip {
        Some(tip) => {
            // One bar more than the limit tells a tip that fits from one
            // that does not; both stay within every backend's signed LIMIT.
            let limit = max_tip_bars.min(i64::MAX as u64 - 1);
            let candles = store
                .load_candles(
                    &market,
                    CandleQuery {
                        start_time_ms: Some(tip.first_open_ms),
                        end_time_ms: Some(tip.last_open_ms),
                        limit: Some(usize::try_from(limit + 1).unwrap_or(usize::MAX)),
                    },
                )
                .map_err(|failure| Refusal::storage("reading a range's tip", failure))?;
            if candles.len() as u64 > limit {
                return Err(Refusal::invalid(format!(
                    "the range holds more than {max_tip_bars} bars from {} on that are not packed \
                     into segments yet, more than this server sends in one answer; ask for a \
                     shorter range",
                    ms_to_rfc3339(tip.first_open_ms)
                )));
            }
            candles
        }
        None => Vec::new(),
    };
    let message = if plan.segments.is_empty() && tip.is_empty() {
        Some(nothing_stored(store, &market, start, end)?)
    } else {
        None
    };
    Ok(CandleRange {
        exchange: selection.venue.as_str().to_string(),
        category: category.as_str().to_string(),
        symbol: market.symbol.as_str().to_string(),
        interval: market.interval.as_str().to_string(),
        start_open_ms: start,
        end_open_ms: end,
        segments: plan
            .segments
            .iter()
            .map(|segment| segment_entry(&market, segment))
            .collect(),
        tip: tip.iter().map(bar).collect(),
        message,
    })
}

/// Why a range holds no bars: the store holds none of the market, or none
/// in the range.
fn nothing_stored(
    store: &dyn ServedStore,
    market: &MarketId,
    start: TimestampMs,
    end: TimestampMs,
) -> Result<String, Refusal> {
    let name = named(market);
    let Some((first, last)) = stored_span(store, market)? else {
        return Ok(format!("the server stores no bars of {name}"));
    };
    Ok(format!(
        "the server stores no bars of {name} from {} to {}; it holds them from {} to {}",
        ms_to_rfc3339(start),
        ms_to_rfc3339(end),
        ms_to_rfc3339(first),
        ms_to_rfc3339(last)
    ))
}

/// The open times of the first and the last bar the store holds of
/// `market`, or `None` when it holds none.
pub(super) fn stored_span(
    store: &dyn ServedStore,
    market: &MarketId,
) -> Result<Option<(TimestampMs, TimestampMs)>, Refusal> {
    let reading = |failure| Refusal::storage("reading a market's span", failure);
    let Some(last) = store.max_open_time_ms(market).map_err(reading)? else {
        return Ok(None);
    };
    let first = store
        .load_candles(
            market,
            CandleQuery {
                limit: Some(1),
                ..CandleQuery::default()
            },
        )
        .map_err(reading)?
        .first()
        .map_or(last, |candle| candle.open_time_ms);
    Ok(Some((first, last)))
}

fn segment_entry(market: &MarketId, segment: &SegmentRef) -> SegmentEntry {
    SegmentEntry {
        hash: ContentHash::sha256(*segment.hash.digest()),
        format: SEGMENT_FORMAT,
        exchange: market.exchange.venue().as_str().to_string(),
        category: market.category().as_str().to_string(),
        symbol: market.symbol.as_str().to_string(),
        interval: market.interval.as_str().to_string(),
        range: SegmentRange {
            first_open_ms: segment.first_open_ms,
            last_open_ms: segment.last_open_ms,
            bar_count: segment.bar_count,
        },
        size_bytes: segment.stored_len,
    }
}

pub(super) fn bar(candle: &Candle) -> Bar {
    Bar {
        open_time_ms: candle.open_time_ms,
        close_time_ms: candle.close_time_ms,
        open: candle.open.into(),
        high: candle.high.into(),
        low: candle.low.into(),
        close: candle.close.into(),
        volume: candle.volume.into(),
        trades: candle.trades,
    }
}
