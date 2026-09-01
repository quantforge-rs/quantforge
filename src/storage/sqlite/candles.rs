//! Candle table: its schema, the `CandleStore` implementation, and the row
//! decoder shared by the two read paths.

use super::{SqliteStore, parse_decimal_str};
use crate::{Candle, CandleQuery, CandleStore, MarketId, StorageError, TimestampMs};
use rusqlite::{ToSql, params, params_from_iter};

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS candles (
                  exchange TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  interval TEXT NOT NULL,
                  open_time_ms INTEGER NOT NULL,
                  close_time_ms INTEGER NOT NULL,
                  open TEXT NOT NULL,
                  high TEXT NOT NULL,
                  low TEXT NOT NULL,
                  close TEXT NOT NULL,
                  volume TEXT NOT NULL,
                  trades INTEGER,
                  PRIMARY KEY (exchange, symbol, interval, open_time_ms)
                );

                CREATE INDEX IF NOT EXISTS idx_candles_market_time
                  ON candles(exchange, symbol, interval, open_time_ms);
"#;

impl CandleStore for SqliteStore {
    fn init(&self) -> Result<(), StorageError> {
        self.initialize_schema()
    }

    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError> {
        if candles.is_empty() {
            return Ok(0);
        }

        let mut connection = self.open()?;
        let transaction = connection.transaction().map_err(StorageError::other)?;
        let mut written = 0usize;

        {
            let mut stmt = transaction
                .prepare_cached(
                    r#"
                    INSERT INTO candles(
                      exchange, symbol, interval, open_time_ms, close_time_ms,
                      open, high, low, close, volume, trades
                    )
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                    ON CONFLICT(exchange, symbol, interval, open_time_ms) DO UPDATE SET
                      close_time_ms = excluded.close_time_ms,
                      open = excluded.open,
                      high = excluded.high,
                      low = excluded.low,
                      close = excluded.close,
                      volume = excluded.volume,
                      trades = excluded.trades
                    "#,
                )
                .map_err(StorageError::other)?;

            for candle in candles {
                stmt.execute(params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str(),
                    candle.open_time_ms,
                    candle.close_time_ms,
                    candle.open.to_string(),
                    candle.high.to_string(),
                    candle.low.to_string(),
                    candle.close.to_string(),
                    candle.volume.to_string(),
                    candle.trades.map(|value| value as i64),
                ])
                .map_err(StorageError::other)?;
                written += 1;
            }
        }

        transaction.commit().map_err(StorageError::other)?;
        Ok(written)
    }

    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError> {
        let connection = self.open()?;

        let mut sql = String::from(
            r#"
            SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades
            FROM candles
            WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
            "#,
        );

        let exchange = market.exchange.as_str().to_string();
        let symbol = market.symbol.as_str().to_string();
        let interval = market.interval.as_str().to_string();
        let start_time_ms = query.start_time_ms;
        let end_time_ms = query.end_time_ms;

        let mut sql_params: Vec<&dyn ToSql> = vec![&exchange, &symbol, &interval];

        if let Some(ref start_value) = start_time_ms {
            sql.push_str(&format!(" AND open_time_ms >= ?{}", sql_params.len() + 1));
            sql_params.push(start_value);
        }
        if let Some(ref end_value) = end_time_ms {
            sql.push_str(&format!(" AND open_time_ms <= ?{}", sql_params.len() + 1));
            sql_params.push(end_value);
        }

        sql.push_str(" ORDER BY open_time_ms ASC");
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = connection.prepare(&sql).map_err(StorageError::other)?;
        let rows = stmt
            .query_map(params_from_iter(sql_params), |row| {
                parse_candle_row(
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                )
            })
            .map_err(StorageError::other)?;

        let mut candles = Vec::new();
        for row in rows {
            candles.push(row.map_err(StorageError::other)?);
        }
        Ok(candles)
    }

    fn load_recent_candles(
        &self,
        market: &MarketId,
        limit: usize,
    ) -> Result<Vec<Candle>, StorageError> {
        let connection = self.open()?;
        let mut stmt = connection
            .prepare(
                r#"
                SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades
                FROM candles
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
                ORDER BY open_time_ms DESC
                LIMIT ?4
                "#,
            )
            .map_err(StorageError::other)?;

        let rows = stmt
            .query_map(
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str(),
                    limit as i64
                ],
                |row| {
                    parse_candle_row(
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                    )
                },
            )
            .map_err(StorageError::other)?;

        let mut candles = Vec::new();
        for row in rows {
            candles.push(row.map_err(StorageError::other)?);
        }
        candles.reverse();
        Ok(candles)
    }

    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        let connection = self.open()?;
        let value = connection
            .query_row(
                r#"
                SELECT MAX(open_time_ms)
                FROM candles
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
                "#,
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str()
                ],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(StorageError::other)?;
        Ok(value)
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_candle_row(
    open_time_ms: i64,
    close_time_ms: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
    trades: Option<i64>,
) -> Result<Candle, rusqlite::Error> {
    Ok(Candle {
        open_time_ms,
        close_time_ms,
        open: parse_decimal_str(&open)?,
        high: parse_decimal_str(&high)?,
        low: parse_decimal_str(&low)?,
        close: parse_decimal_str(&close)?,
        volume: parse_decimal_str(&volume)?,
        trades: trades.map(|value| value as u64),
    })
}
