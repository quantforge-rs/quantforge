//! A 100,000-candle batch through `upsert_candles`, measured on SQLite and,
//! when `QUANTFORGE_TEST_POSTGRES_URL` is set, on PostgreSQL: into an empty
//! market, again unchanged, and again with every close changed, each on a
//! fresh store, three runs. A measurement, not a check of speed: it asserts
//! what each batch wrote and nothing about how long it took, and it runs
//! only when asked, in release:
//!
//! ```text
//! cargo test --release --test storage measure -- --ignored --nocapture
//! ```
//!
//! `docs/postgres-query-plans.md` ("Writing candles") records a run.

use quantforge::{Candle, CandleQuery, CandleStore, ExchangeId, Interval, MarketId, Symbol};
use rust_decimal::Decimal;
use std::time::{Duration, Instant};

use crate::conformance::Backend;
use crate::sqlite::Sqlite;

const CANDLES: i64 = 100_000;
const RUNS: usize = 3;
/// 2026-10-20T00:00:00Z.
const FROM: i64 = 1_792_454_400_000;
const MINUTE: i64 = 60_000;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitLinear,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

/// 100,000 consecutive `1m` candles, about 69 days, with prices of two
/// places and volumes of eight, as an exchange reports them; `changed`
/// moves every close by a cent.
fn batch(changed: bool) -> Vec<Candle> {
    let cents = |value: i64| Decimal::new(value, 2);
    (0..CANDLES)
        .map(|i| {
            let base = 6_700_000 + (i * 37) % 50_000;
            let close = base + (i * 13) % 900 - 450 + i64::from(changed);
            Candle {
                open_time_ms: FROM + i * MINUTE,
                close_time_ms: FROM + i * MINUTE + MINUTE - 1,
                open: cents(base),
                high: cents(base.max(close) + 125),
                low: cents(base.min(close) - 125),
                close: cents(close),
                volume: Decimal::new(1_000_000_000 + (i * 7_919) % 900_000_000, 8),
                trades: Some((100 + (i * 31) % 4_000) as u64),
            }
        })
        .collect()
}

/// One `upsert_candles` of `candles`, timed.
fn timed<S: CandleStore>(store: &S, candles: &[Candle]) -> Duration {
    let started = Instant::now();
    let written = store.upsert_candles(&market(), candles).expect("the batch");
    let took = started.elapsed();
    assert_eq!(written, candles.len(), "every candle counts");
    took
}

fn measure<B: Backend>()
where
    B::Store: CandleStore,
{
    let (first, changed) = (batch(false), batch(true));
    for run in 1..=RUNS {
        let fixture = B::fresh();
        let store = &fixture.store;
        let fresh = timed(store, &first);
        let identical = timed(store, &first);
        let corrected = timed(store, &changed);
        let stored = store
            .load_candles(&market(), CandleQuery::default())
            .expect("load");
        assert_eq!(stored.len() as i64, CANDLES);
        assert_eq!(stored[1].close.to_string(), changed[1].close.to_string());
        eprintln!(
            "{} run {run}: into an empty market {:.3} s, unchanged {:.3} s, every close changed {:.3} s",
            B::NAME,
            fresh.as_secs_f64(),
            identical.as_secs_f64(),
            corrected.as_secs_f64()
        );
    }
}

#[test]
#[ignore = "a measurement, run in release with --ignored"]
fn a_100000_candle_batch_on_each_backend() {
    measure::<Sqlite>();
    #[cfg(feature = "postgres")]
    {
        if std::env::var("QUANTFORGE_TEST_POSTGRES_URL").is_ok_and(|url| !url.is_empty()) {
            measure::<crate::postgres::Postgres>();
        } else {
            eprintln!("postgres skipped: QUANTFORGE_TEST_POSTGRES_URL is unset");
        }
    }
}
