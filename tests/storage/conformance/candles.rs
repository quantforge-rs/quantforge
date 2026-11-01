//! The candle store's steps, and the checks that read the shared cases:
//! every product line stores its market under the storage spelling the
//! identity cases give, every decimal text of the cases comes back
//! unchanged, and the candles of every segment spec case, read back, pack
//! to the case's name on every backend.

use quantforge::storage::segments;
use quantforge::{
    Candle, CandleQuery, CandleStore, Category, ExchangeId, Interval, MarketId, Symbol, Venue,
};
use std::str::FromStr;

use super::cases::{CandleText, Cases, decimal, identity_cases, segment_spec_cases};
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "candles.identity_product_lines",
        "every product line of the identity cases stores its market under the storage spelling the cases give, and reads back only its own candle",
    ),
    (
        "candles.identity_decimals",
        "every decimal text of the identity cases is stored and read back unchanged",
    ),
    (
        "candles.segment_cases",
        "the candles of every segment spec case, stored and read back, pack to the case's name",
    ),
];

pub fn check<S: CandleStore>(id: &str) -> Coded<S> {
    match id {
        "candles.identity_product_lines" => product_lines,
        "candles.identity_decimals" => identity_decimals,
        "candles.segment_cases" => segment_cases,
        other => panic!("no candle check {other}"),
    }
}

fn texts(candles: &[Candle]) -> Vec<CandleText> {
    candles.iter().map(CandleText::of).collect()
}

/// Runs a candle step; `false` for a step of another port.
pub fn step<S: CandleStore + ?Sized>(cases: &Cases, store: &S, step: &Step) -> bool {
    match step {
        Step::Upsert {
            market,
            candles,
            series,
            written,
            refused,
        } => {
            let market = cases.market(market);
            let rows = match (candles, series) {
                (Some(candles), None) => candles.clone(),
                (None, Some(series)) => series.candles(market.interval),
                _ => panic!("an upsert names candles or a series"),
            };
            let rows: Vec<Candle> = rows.iter().map(CandleText::candle).collect();
            let outcome = store.upsert_candles(&market, &rows);
            match (written, refused) {
                (Some(written), None) => {
                    assert_eq!(&outcome.expect("upsert"), written, "written")
                }
                (None, Some(refused)) => expect_refused(outcome, refused, "upsert"),
                _ => panic!("an upsert expects a count or a refusal"),
            }
        }
        Step::Load {
            market,
            start,
            end,
            limit,
            expect,
            refused,
        } => {
            let query = CandleQuery {
                start_time_ms: *start,
                end_time_ms: *end,
                limit: *limit,
            };
            let outcome = store.load_candles(&cases.market(market), query);
            let what = format!("load {start:?}..={end:?} limit {limit:?}");
            match (expect, refused) {
                (Some(expect), None) => {
                    assert_eq!(&texts(&outcome.expect("load")), expect, "{what}")
                }
                (None, Some(refused)) => expect_refused(outcome, refused, &what),
                _ => panic!("a load expects candles or a refusal"),
            }
        }
        Step::Recent {
            market,
            limit,
            expect,
            refused,
        } => {
            let outcome = store.load_recent_candles(&cases.market(market), *limit);
            let what = format!("recent {limit}");
            match (expect, refused) {
                (Some(expect), None) => {
                    assert_eq!(&texts(&outcome.expect("recent")), expect, "{what}")
                }
                (None, Some(refused)) => expect_refused(outcome, refused, &what),
                _ => panic!("a recent read expects candles or a refusal"),
            }
        }
        Step::MaxOpenTime { market, expect } => {
            let newest = store
                .max_open_time_ms(&cases.market(market))
                .expect("max open time");
            assert_eq!(&newest, expect, "max open time of {market}");
        }
        _ => return false,
    }
    true
}

fn venue(name: &str) -> Venue {
    Venue::ALL
        .into_iter()
        .find(|venue| venue.as_str() == name)
        .unwrap_or_else(|| panic!("unknown venue {name}"))
}

fn category(name: &str) -> Category {
    Category::ALL
        .into_iter()
        .find(|category| category.as_str() == name)
        .unwrap_or_else(|| panic!("unknown category {name}"))
}

fn candle(open_time_ms: i64, close: &str) -> Candle {
    let close = decimal(close);
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: decimal("1.000"),
        trades: Some(1),
    }
}

/// The text of `close` for every candle of `market`, in open-time order:
/// the backend's own view of what it stored.
fn stored_closes<S>(fixture: &Fixture<S>, market: &MarketId) -> Vec<String> {
    let sql = format!(
        "SELECT close FROM candles WHERE exchange = '{}' AND symbol = '{}' AND interval = '{}' \
         ORDER BY open_time_ms",
        market.exchange.as_str(),
        market.symbol.as_str(),
        market.interval.as_str()
    );
    fixture
        .raw
        .select(&sql)
        .into_iter()
        .map(|row| row.into_iter().next().flatten().expect("a close"))
        .collect()
}

/// Every product line of the identity cases stores and returns its market
/// under the case's storage spelling, keeps the same symbol on another line
/// apart, and spells itself on the wire and in `state_json` as the case
/// says.
fn product_lines<S: CandleStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    let cases = identity_cases();
    assert_eq!(cases.markets.len(), 6, "one case per product line");
    for (index, case) in cases.markets.iter().enumerate() {
        let line = ExchangeId::from_parts(venue(&case.venue), category(&case.category));
        assert_eq!(line.as_str(), case.storage.exchange, "{case:?}");
        assert_eq!(line.venue().as_str(), case.wire.exchange);
        assert_eq!(line.category().as_str(), case.wire.category);
        assert_eq!(
            case.wire.symbol, case.storage.symbol,
            "one spelling of a symbol"
        );
        assert_eq!(
            case.wire.interval, case.storage.interval,
            "one spelling of an interval"
        );
        assert_eq!(
            serde_json::to_value(line).expect("token"),
            serde_json::Value::String(case.state_json_exchange.clone())
        );
        let market = MarketId::new(
            line,
            Symbol::new(&case.symbol).expect("symbol"),
            Interval::from_str(&case.interval).expect("interval"),
        );
        assert_eq!(market.symbol.as_str(), case.storage.symbol);
        assert_eq!(market.interval.as_str(), case.storage.interval);
        // The same open time on every line, with a close only this line has.
        let close = format!("{}.{index}0", 100 + index);
        store
            .upsert_candles(&market, &[candle(1_792_454_400_000, &close)])
            .expect("upsert");
        assert_eq!(stored_closes(fixture, &market), vec![close]);
    }
    // Each line reads back only its own candle.
    for (index, case) in cases.markets.iter().enumerate() {
        let market = MarketId::new(
            ExchangeId::from_storage_name(&case.storage.exchange).expect("storage name"),
            Symbol::new(&case.storage.symbol).expect("symbol"),
            Interval::from_str(&case.storage.interval).expect("interval"),
        );
        let loaded = store
            .load_candles(&market, CandleQuery::default())
            .expect("load");
        assert_eq!(loaded.len(), 1, "{case:?}");
        assert_eq!(
            loaded[0].close.to_string(),
            format!("{}.{index}0", 100 + index)
        );
    }
}

/// Every decimal text of the identity cases is stored and returned with its
/// scale.
fn identity_decimals<S: CandleStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    let cases = identity_cases();
    let market = MarketId::new(
        ExchangeId::BybitInverse,
        Symbol::new("BTCUSD").expect("symbol"),
        Interval::M1,
    );
    let candles: Vec<Candle> = cases
        .decimals
        .iter()
        .enumerate()
        .map(|(index, text)| candle(1_792_454_400_000 + index as i64 * 60_000, text))
        .collect();
    store.upsert_candles(&market, &candles).expect("upsert");
    assert_eq!(
        stored_closes(fixture, &market),
        cases.decimals,
        "the stored text"
    );
    let loaded = store
        .load_candles(&market, CandleQuery::default())
        .expect("load");
    let texts: Vec<String> = loaded
        .iter()
        .map(|candle| candle.close.to_string())
        .collect();
    assert_eq!(texts, cases.decimals, "the text read back");
}

/// The candles of every segment spec case, stored under a market of their
/// interval and read back, are the case's texts and pack to the case's
/// name: the same rows make the same canonical bytes on every backend.
fn segment_cases<S: CandleStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    for (index, case) in segment_spec_cases().iter().enumerate() {
        let market = MarketId::new(
            ExchangeId::BybitLinear,
            Symbol::new(format!("CASE{index}USDT")).expect("symbol"),
            case.interval(),
        );
        let candles: Vec<Candle> = case.candles.iter().map(CandleText::candle).collect();
        store
            .upsert_candles(&market, &candles)
            .expect("the case's candles");
        let loaded = store
            .load_candles(&market, CandleQuery::default())
            .expect("load");
        assert_eq!(texts(&loaded), case.candles, "{}", case.name);
        let payload = segments::encode(&loaded).expect("a payload");
        assert_eq!(
            segments::hash(&payload).to_string(),
            case.hash,
            "{}",
            case.name
        );
    }
}
