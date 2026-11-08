//! The history routes: the markets the store holds, one market's bars in a
//! range, and the store they read through. Every store call runs on a
//! blocking thread once a turn at the store is free, so that a slow store
//! never stalls the requests that do not need it (ADR 0001).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

use super::refusal::Refusal;
use super::selection::Selection;
use crate::model::{
    Candle, Category, ExchangeId, MarketCoverage, MarketId, SegmentRef, ms_to_rfc3339,
};
use crate::ports::{CandleQuery, CandleStore, SegmentStore};
use crate::protocol::ContentHash;
use crate::protocol::segments::{
    CandleRange, CandleRangeQuery, MarketEntry, MarketIndex, SEGMENT_FORMAT, SegmentEntry,
    SegmentRange,
};
use crate::protocol::stream::Bar;

/// The most bars one `/v1/candles` answer spans when nothing sets it: a
/// leap year of `1m` bars, 366 segments.
pub const DEFAULT_MAX_SPAN_BARS: u64 = 527_040;

/// The storage ports the history routes read, as one trait object: MSRV
/// 1.85 cannot turn one trait object into another, so this trait stands for
/// both. Every store implements it.
pub trait HistoryStore: CandleStore + SegmentStore {}

impl<S: CandleStore + SegmentStore> HistoryStore for S {}

/// How much the history routes take on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    /// The most bars one `/v1/candles` answer may span.
    pub max_span_bars: u64,
    /// How many requests use the store at once: as many as its pool has
    /// connections.
    pub concurrency: usize,
    /// How long a request waits for its turn before it is answered 503.
    pub wait: Duration,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_span_bars: DEFAULT_MAX_SPAN_BARS,
            concurrency: 8,
            wait: Duration::from_secs(2),
        }
    }
}

/// What the history routes read, and how much they take on at once.
#[derive(Clone)]
pub struct History {
    store: Arc<dyn HistoryStore>,
    turns: Arc<Semaphore>,
    limits: HistoryLimits,
}

impl History {
    /// The routes over `store`. Each of its calls should be bounded, as a
    /// store from `AnyStore::within` is: a request keeps its turn for as long
    /// as its calls take, even after its client gives up.
    pub fn new<S: HistoryStore + 'static>(store: S, limits: HistoryLimits) -> Self {
        Self {
            store: Arc::new(store),
            turns: Arc::new(Semaphore::new(limits.concurrency.max(1))),
            limits,
        }
    }

    /// Runs `work` on a blocking thread once a turn at the store is free, or
    /// refuses as unavailable when none frees up in time. The turn goes
    /// with the work, so a request whose client hangs up keeps it until its
    /// store calls end.
    pub(super) async fn with_store<T, F>(&self, work: F) -> Result<T, Refusal>
    where
        T: Send + 'static,
        F: FnOnce(&dyn HistoryStore) -> Result<T, Refusal> + Send + 'static,
    {
        let turn = tokio::time::timeout(self.limits.wait, Arc::clone(&self.turns).acquire_owned())
            .await
            .map_err(|_| {
                Refusal::unavailable(
                    "the server is answering as many history requests as its store can take; \
                     retry later",
                )
            })?
            .map_err(|_| Refusal::internal("the server is shutting down"))?;
        let store = Arc::clone(&self.store);
        let done = tokio::task::spawn_blocking(move || {
            let _turn = turn;
            work(store.as_ref())
        })
        .await;
        done.unwrap_or_else(|_| Err(Refusal::internal("the server failed while answering")))
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
fn fresh<T: IntoResponse>(answer: T) -> Response {
    let mut response = answer.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `GET /v1/markets`.
pub(super) async fn markets(State(history): State<History>) -> Result<Response, Refusal> {
    let covered = history
        .with_store(|store| {
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
    let answer = history
        .with_store(move |store| answer(store, &selection))
        .await?;
    Ok(fresh(Json(answer)))
}

/// The range's segments and tip, as the store holds them.
fn answer(store: &dyn HistoryStore, selection: &Selection) -> Result<CandleRange, Refusal> {
    let category = match selection.category {
        Some(category) => category,
        None => legacy_category(store, selection)?,
    };
    let market = MarketId::new(
        ExchangeId::from_parts(selection.venue, category),
        selection.symbol.clone(),
        selection.interval,
    );
    let (start, end) = (selection.start_open_ms, selection.end_open_ms);
    let plan = store
        .resolve_segments(&market, start, end)
        .map_err(|failure| Refusal::storage("planning a range", failure))?;
    let tip = match plan.tip {
        Some(tip) => store
            .load_candles(
                &market,
                CandleQuery {
                    start_time_ms: Some(tip.first_open_ms),
                    end_time_ms: Some(tip.last_open_ms),
                    limit: None,
                },
            )
            .map_err(|failure| Refusal::storage("reading a range's tip", failure))?,
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

/// The category a query without one reads: Spot, the only family before
/// markets carried one, unless another family of the venue holds the
/// symbol and interval, which makes the query ambiguous.
fn legacy_category(store: &dyn HistoryStore, selection: &Selection) -> Result<Category, Refusal> {
    let mut held = Vec::new();
    for category in Category::ALL {
        let market = MarketId::new(
            ExchangeId::from_parts(selection.venue, category),
            selection.symbol.clone(),
            selection.interval,
        );
        let newest = store
            .max_open_time_ms(&market)
            .map_err(|failure| Refusal::storage("finding a query's category", failure))?;
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
        "{} stores {} {} as {}; a query without category reads spot only when no other \
         family holds the symbol and interval, so name the category",
        selection.venue.as_str(),
        selection.symbol.as_str(),
        selection.interval.as_str(),
        listed(&held)
    )))
}

/// `a`, `a and b`, or `a, b and c`.
fn listed(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.join(""),
    }
}

/// Why a range holds no bars: the store holds none of the market, or none
/// in the range.
fn nothing_stored(
    store: &dyn HistoryStore,
    market: &MarketId,
    start: i64,
    end: i64,
) -> Result<String, Refusal> {
    let name = format!(
        "{} {} {} {}",
        market.exchange.venue().as_str(),
        market.category().as_str(),
        market.symbol.as_str(),
        market.interval.as_str()
    );
    let reading = |failure| Refusal::storage("reading a market's span", failure);
    let Some(last) = store.max_open_time_ms(market).map_err(reading)? else {
        return Ok(format!("the server stores no bars of {name}"));
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
    Ok(format!(
        "the server stores no bars of {name} from {} to {}; it holds them from {} to {}",
        ms_to_rfc3339(start),
        ms_to_rfc3339(end),
        ms_to_rfc3339(first),
        ms_to_rfc3339(last)
    ))
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

fn bar(candle: &Candle) -> Bar {
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
