//! `CandleStore` over the bridge: what ALG-404's adapter does for every port,
//! shown for one. Decimals are TEXT, written with `to_string` and read with
//! `from_str` exactly as the SQLite store does, so both return the same text.
//! Statements are unnamed (`query_typed`), which PgBouncer's transaction mode
//! needs, and `init` runs its DDL under an advisory lock, so that two
//! processes starting together cannot race on it.

use std::str::FromStr;

use quantforge::{Candle, CandleQuery, CandleStore, MarketId, StorageError, TimestampMs};
use rust_decimal::Decimal;
use tokio_postgres::types::{ToSql, Type};

use crate::bridge::{BridgeConfig, Effect, PgBridge, PgError};

/// Every statement's schema-qualified table, so that tests can share a
/// database without sharing data.
#[derive(Debug)]
pub struct PgCandleStore {
    bridge: PgBridge,
    schema: String,
    table: String,
}

impl PgCandleStore {
    /// Connects, and keeps the candles in `schema`.
    pub fn connect(config: &BridgeConfig, schema: &str) -> Result<Self, PgError> {
        Ok(Self {
            bridge: PgBridge::connect(config)?,
            schema: format!("\"{schema}\""),
            table: format!("\"{schema}\".candles"),
        })
    }

    /// The bridge, for tests that run their own statements on it.
    pub fn bridge(&self) -> &PgBridge {
        &self.bridge
    }

    fn rows(&self, sql: String, params: Vec<Param>) -> Result<Vec<Candle>, StorageError> {
        let rows = self
            .bridge
            .execute(Effect::Read, move |client| {
                Box::pin(async move {
                    let typed: Vec<(&(dyn ToSql + Sync), Type)> =
                        params.iter().map(Param::typed).collect();
                    client.query_typed(&sql, &typed).await
                })
            })
            .map_err(StorageError::other)?;
        rows.iter().map(candle).collect()
    }
}

/// A statement parameter with its declared type, as `query_typed` takes it.
enum Param {
    Text(String),
    Int(i64),
    /// An integer or SQL `NULL`.
    OptInt(Option<i64>),
}

impl Param {
    fn typed(&self) -> (&(dyn ToSql + Sync), Type) {
        match self {
            Self::Text(value) => (value, Type::TEXT),
            Self::Int(value) => (value, Type::INT8),
            Self::OptInt(value) => (value, Type::INT8),
        }
    }
}

fn key(market: &MarketId) -> [Param; 3] {
    [
        Param::Text(market.exchange.as_str().to_string()),
        Param::Text(market.symbol.as_str().to_string()),
        Param::Text(market.interval.as_str().to_string()),
    ]
}

fn candle(row: &tokio_postgres::Row) -> Result<Candle, StorageError> {
    let decimal = |index: usize| -> Result<Decimal, StorageError> {
        let text: String = row.try_get(index).map_err(StorageError::other)?;
        Decimal::from_str(&text).map_err(StorageError::other)
    };
    Ok(Candle {
        open_time_ms: row.try_get(0).map_err(StorageError::other)?,
        close_time_ms: row.try_get(1).map_err(StorageError::other)?,
        open: decimal(2)?,
        high: decimal(3)?,
        low: decimal(4)?,
        close: decimal(5)?,
        volume: decimal(6)?,
        trades: row
            .try_get::<_, Option<i64>>(7)
            .map_err(StorageError::other)?
            .map(|trades| trades as u64),
    })
}

const COLUMNS: &str = "open_time_ms, close_time_ms, open, high, low, close, volume, trades";

impl CandleStore for PgCandleStore {
    fn init(&self) -> Result<(), StorageError> {
        let (schema, table) = (self.schema.clone(), self.table.clone());
        self.bridge
            .execute(Effect::Write, move |client| {
                Box::pin(async move {
                    client
                        .batch_execute(&format!(
                            "SELECT pg_advisory_lock(4003);
                             CREATE SCHEMA IF NOT EXISTS {schema};
                             CREATE TABLE IF NOT EXISTS {table} (
                               exchange TEXT NOT NULL,
                               symbol TEXT NOT NULL,
                               interval TEXT NOT NULL,
                               open_time_ms BIGINT NOT NULL,
                               close_time_ms BIGINT NOT NULL,
                               open TEXT NOT NULL,
                               high TEXT NOT NULL,
                               low TEXT NOT NULL,
                               close TEXT NOT NULL,
                               volume TEXT NOT NULL,
                               trades BIGINT,
                               PRIMARY KEY (exchange, symbol, interval, open_time_ms)
                             );
                             SELECT pg_advisory_unlock(4003);"
                        ))
                        .await
                })
            })
            .map_err(StorageError::other)
    }

    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError> {
        if candles.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "INSERT INTO {table} (exchange, symbol, interval, {COLUMNS})
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (exchange, symbol, interval, open_time_ms) DO UPDATE SET
               close_time_ms = excluded.close_time_ms, open = excluded.open,
               high = excluded.high, low = excluded.low, close = excluded.close,
               volume = excluded.volume, trades = excluded.trades
             WHERE {table}.close_time_ms IS DISTINCT FROM excluded.close_time_ms
               OR {table}.open IS DISTINCT FROM excluded.open
               OR {table}.high IS DISTINCT FROM excluded.high
               OR {table}.low IS DISTINCT FROM excluded.low
               OR {table}.close IS DISTINCT FROM excluded.close
               OR {table}.volume IS DISTINCT FROM excluded.volume
               OR {table}.trades IS DISTINCT FROM excluded.trades",
            table = self.table
        );
        let rows: Vec<[Param; 11]> = candles
            .iter()
            .map(|candle| {
                let [exchange, symbol, interval] = key(market);
                [
                    exchange,
                    symbol,
                    interval,
                    Param::Int(candle.open_time_ms),
                    Param::Int(candle.close_time_ms),
                    Param::Text(candle.open.to_string()),
                    Param::Text(candle.high.to_string()),
                    Param::Text(candle.low.to_string()),
                    Param::Text(candle.close.to_string()),
                    Param::Text(candle.volume.to_string()),
                    Param::OptInt(candle.trades.map(|trades| trades as i64)),
                ]
            })
            .collect();
        let written = rows.len();
        self.bridge
            .execute(Effect::Write, move |client| {
                Box::pin(async move {
                    client.batch_execute("BEGIN").await?;
                    for row in &rows {
                        let typed: Vec<(&(dyn ToSql + Sync), Type)> =
                            row.iter().map(Param::typed).collect();
                        if let Err(error) = client.query_typed(&sql, &typed).await {
                            let _ = client.batch_execute("ROLLBACK").await;
                            return Err(error);
                        }
                    }
                    client.batch_execute("COMMIT").await
                })
            })
            .map_err(StorageError::other)?;
        Ok(written)
    }

    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError> {
        let mut sql = format!(
            "SELECT {COLUMNS} FROM {table} WHERE exchange = $1 AND symbol = $2 AND interval = $3",
            table = self.table
        );
        let mut params: Vec<Param> = key(market).into();
        if let Some(start) = query.start_time_ms {
            params.push(Param::Int(start));
            sql.push_str(&format!(" AND open_time_ms >= ${}", params.len()));
        }
        if let Some(end) = query.end_time_ms {
            params.push(Param::Int(end));
            sql.push_str(&format!(" AND open_time_ms <= ${}", params.len()));
        }
        sql.push_str(" ORDER BY open_time_ms ASC");
        if let Some(limit) = query.limit {
            params.push(Param::Int(limit as i64));
            sql.push_str(&format!(" LIMIT ${}", params.len()));
        }
        self.rows(sql, params)
    }

    fn load_recent_candles(
        &self,
        market: &MarketId,
        limit: usize,
    ) -> Result<Vec<Candle>, StorageError> {
        let mut params: Vec<Param> = key(market).into();
        params.push(Param::Int(limit as i64));
        let sql = format!(
            "SELECT {COLUMNS} FROM {table} WHERE exchange = $1 AND symbol = $2 AND interval = $3
             ORDER BY open_time_ms DESC LIMIT $4",
            table = self.table
        );
        let mut candles = self.rows(sql, params)?;
        candles.reverse();
        Ok(candles)
    }

    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        let params: Vec<Param> = key(market).into();
        let sql = format!(
            "SELECT MAX(open_time_ms) FROM {table}
             WHERE exchange = $1 AND symbol = $2 AND interval = $3",
            table = self.table
        );
        self.bridge
            .execute(Effect::Read, move |client| {
                Box::pin(async move {
                    let typed: Vec<(&(dyn ToSql + Sync), Type)> =
                        params.iter().map(Param::typed).collect();
                    let row = client.query_typed_one(&sql, &typed).await?;
                    row.try_get::<_, Option<i64>>(0)
                })
            })
            .map_err(StorageError::other)
    }
}
