//! What the database spikes share: the URL, names no other test uses, an
//! observer counting connections on the server, and candles to store.
#![allow(dead_code)] // each test binary uses its own part of this module

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use postgres_bridge_spike::bridge::{BridgeConfig, Effect, PgBridge};
use postgres_bridge_spike::store::PgCandleStore;
use quantforge::{Candle, CandleStore, ExchangeId, Interval, MarketId, Symbol};
use rust_decimal::Decimal;
use tokio_postgres::types::Type;

/// The database the spikes run against, or `None` to skip them.
pub fn database_url() -> Option<String> {
    std::env::var("SPIKE_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

/// The URL, or returns from the test, saying why.
#[macro_export]
macro_rules! database_or_skip {
    () => {
        match common::database_url() {
            Some(url) => url,
            None => {
                eprintln!("skipping: SPIKE_DATABASE_URL is unset");
                return;
            }
        }
    };
}

/// A name no other test in this run uses, for a schema or an
/// `application_name`.
pub fn unique(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A store in a schema of its own, initialized.
pub fn store(config: &BridgeConfig) -> PgCandleStore {
    let store = PgCandleStore::connect(config, &unique("spike")).expect("a store");
    CandleStore::init(&store).expect("init");
    store
}

/// Counts what the server shows in `pg_stat_activity`, one statement at a
/// time, so that each count is fresh rather than one transaction's snapshot.
pub struct Observer {
    bridge: PgBridge,
}

impl Observer {
    pub fn connect(url: &str) -> Self {
        let mut config = BridgeConfig::new(url, unique("observer"));
        config.max_size = 1;
        Self {
            bridge: PgBridge::connect(&config).expect("an observer"),
        }
    }

    fn count(&self, sql: &'static str, application_name: &str) -> i64 {
        let name = application_name.to_string();
        self.bridge
            .execute(Effect::Read, move |client| {
                Box::pin(async move {
                    let row = client.query_typed_one(sql, &[(&name, Type::TEXT)]).await?;
                    row.try_get::<_, i64>(0)
                })
            })
            .expect("a count")
    }

    /// The connections open under `application_name`.
    pub fn connections(&self, application_name: &str) -> i64 {
        self.count(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
            application_name,
        )
    }

    /// Those of them waiting in `pg_sleep`.
    pub fn sleeping(&self, application_name: &str) -> i64 {
        self.count(
            "SELECT count(*) FROM pg_stat_activity
             WHERE application_name = $1 AND wait_event = 'PgSleep'",
            application_name,
        )
    }

    /// Waits up to five seconds for `count` to say `expected`.
    pub fn wait_for(&self, expected: i64, count: impl Fn(&Self) -> i64) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let found = count(self);
            if found == expected || Instant::now() > deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

/// 2024-01-01T00:00:00Z: long closed, so data sync stores these.
pub const T: i64 = 1_704_067_200_000;

/// A candle whose prices carry their text as written.
pub fn candle(open_time_ms: i64, close: &str, trades: Option<u64>) -> Candle {
    let close: Decimal = close.parse().expect("close");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: "0.50000000".parse().expect("volume"),
        trades,
    }
}

/// Scales and trade counts a careless store would lose: trailing zeros,
/// zero with a scale, an absent count, and a count of zero.
pub fn tricky_candles() -> Vec<Candle> {
    vec![
        candle(T, "1.10", Some(5)),
        candle(T + 60_000, "1.1", None),
        candle(T + 120_000, "0.00000000", Some(0)),
        candle(T + 180_000, "67015.50000000", Some(1234)),
        candle(T + 240_000, "79228162514264337593543950335", None),
    ]
}

/// Candles as text, so that `1.10` and `1.1` differ, as they must.
pub fn texts(candles: &[Candle]) -> Vec<String> {
    candles
        .iter()
        .map(|candle| {
            format!(
                "{} {} {} {} {} {} {} {:?}",
                candle.open_time_ms,
                candle.close_time_ms,
                candle.open,
                candle.high,
                candle.low,
                candle.close,
                candle.volume,
                candle.trades
            )
        })
        .collect()
}
