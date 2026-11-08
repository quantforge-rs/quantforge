//! The markets the candle table holds, and the open times each spans: one
//! snapshot, read one indexed step at a time, as SQLite reads it.

use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{IsolationLevel, Row};

use super::PostgresStore;
use super::bridge::Effect;
use super::candles::MAX_OPEN_TIME;
use crate::storage::common;
use crate::{MarketCoverage, StorageError, TimestampMs};

/// The first candle of the first market in key order: one indexed step.
const FIRST_MARKET: &str = r#"
    SELECT exchange, symbol, interval, open_time_ms FROM candles
    ORDER BY exchange, symbol, interval, open_time_ms
    LIMIT 1
"#;

/// The first candle of the market after a key: one indexed step. The
/// comparison and the key share the database's collation, so the steps
/// follow the index whatever it is.
const NEXT_MARKET: &str = r#"
    SELECT exchange, symbol, interval, open_time_ms FROM candles
    WHERE (exchange, symbol, interval) > ($1, $2, $3)
    ORDER BY exchange, symbol, interval, open_time_ms
    LIMIT 1
"#;

/// A market's key, as stored, and the open time of its first candle.
type FirstCandle = (String, String, String, TimestampMs);

fn first_candle(row: &Row) -> Result<FirstCandle, tokio_postgres::Error> {
    Ok((
        row.try_get(0)?,
        row.try_get(1)?,
        row.try_get(2)?,
        row.try_get(3)?,
    ))
}

impl PostgresStore {
    /// `CandleStore::market_coverage`: every market's key and span, read in
    /// one read-only snapshot.
    pub(super) fn coverage(&self) -> Result<Vec<MarketCoverage>, StorageError> {
        let spans = self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::RepeatableRead)
                    .read_only(true)
                    .start()
                    .await?;
                let mut spans = Vec::new();
                let mut found = transaction.query_typed_opt(FIRST_MARKET, &[]).await?;
                while let Some(row) = found {
                    let first = first_candle(&row)?;
                    let (exchange, symbol, interval, _) = &first;
                    let key: [(&(dyn ToSql + Sync), Type); 3] = [
                        (exchange, Type::TEXT),
                        (symbol, Type::TEXT),
                        (interval, Type::TEXT),
                    ];
                    let last_open_ms: TimestampMs = transaction
                        .query_typed_one(MAX_OPEN_TIME, &key)
                        .await?
                        .try_get(0)?;
                    found = transaction.query_typed_opt(NEXT_MARKET, &key).await?;
                    spans.push((first, last_open_ms));
                }
                Ok(spans)
            })
        })?;
        Ok(spans
            .into_iter()
            .filter_map(
                |((exchange, symbol, interval, first_open_ms), last_open_ms)| {
                    Some(MarketCoverage {
                        market: common::stored_market(&exchange, &symbol, &interval)?,
                        first_open_ms,
                        last_open_ms,
                    })
                },
            )
            .collect())
    }
}
