//! Bot-run journal: the run-state, order-event and closed-trade tables and
//! the `RunJournalStore` implementation over them.

use super::{SqliteStore, parse_decimal_str, to_from_sql_error};
use crate::{BotRunState, ClosedTrade, ExchangeOrder, MarketId, RunJournalStore, StorageError};
use rusqlite::{OptionalExtension, params};

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS bot_runs (
                  run_id TEXT PRIMARY KEY,
                  exchange TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  interval TEXT NOT NULL,
                  strategy_name TEXT NOT NULL,
                  status TEXT NOT NULL,
                  state_json TEXT NOT NULL,
                  started_at_ms INTEGER NOT NULL,
                  updated_at_ms INTEGER NOT NULL,
                  stopped_at_ms INTEGER,
                  last_error TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_bot_runs_market
                  ON bot_runs(exchange, symbol, interval, strategy_name, updated_at_ms DESC);

                CREATE TABLE IF NOT EXISTS order_events (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  run_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  side TEXT NOT NULL,
                  order_type TEXT NOT NULL,
                  status TEXT NOT NULL,
                  order_id INTEGER,
                  client_order_id TEXT,
                  transact_time_ms INTEGER,
                  raw_json TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_order_events_run
                  ON order_events(run_id, seq DESC);

                CREATE TABLE IF NOT EXISTS closed_trades (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  run_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  entry_time_ms INTEGER NOT NULL,
                  exit_time_ms INTEGER NOT NULL,
                  entry_price TEXT NOT NULL,
                  exit_price TEXT NOT NULL,
                  qty TEXT NOT NULL,
                  gross_quote_pnl TEXT NOT NULL,
                  entry_order_id INTEGER,
                  exit_order_id INTEGER
                );

                CREATE INDEX IF NOT EXISTS idx_closed_trades_run
                  ON closed_trades(run_id, seq DESC);
"#;

impl RunJournalStore for SqliteStore {
    fn init(&self) -> Result<(), StorageError> {
        self.initialize_schema()
    }

    // `state_json` is the single source of truth for run state; the
    // `status`, `stopped_at_ms`, and `last_error` columns are denormalized
    // copies for ad-hoc SQL/display only and are never read back into a
    // `BotRunState`.
    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError> {
        let connection = self.open()?;
        let state_json = serde_json::to_string(state).map_err(StorageError::other)?;
        connection
            .execute(
                r#"
                INSERT INTO bot_runs(
                  run_id, exchange, symbol, interval, strategy_name, status,
                  state_json, started_at_ms, updated_at_ms, stopped_at_ms, last_error
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(run_id) DO UPDATE SET
                  status = excluded.status,
                  state_json = excluded.state_json,
                  updated_at_ms = excluded.updated_at_ms,
                  stopped_at_ms = excluded.stopped_at_ms,
                  last_error = excluded.last_error
                "#,
                params![
                    &state.run_id,
                    state.market.exchange.as_str(),
                    state.market.symbol.as_str(),
                    state.market.interval.as_str(),
                    &state.strategy_name,
                    state.status.as_str(),
                    state_json,
                    state.started_at_ms,
                    state.updated_at_ms,
                    state.stopped_at_ms,
                    state.last_error.as_deref(),
                ],
            )
            .map_err(StorageError::other)?;
        Ok(())
    }

    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError> {
        let connection = self.open()?;
        let state_json = connection
            .query_row(
                "SELECT state_json FROM bot_runs WHERE run_id = ?1",
                [run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::other)?;

        match state_json {
            Some(json) => {
                let state =
                    serde_json::from_str::<BotRunState>(&json).map_err(StorageError::other)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError> {
        let connection = self.open()?;
        let state_json = connection
            .query_row(
                r#"
                SELECT state_json
                FROM bot_runs
                WHERE exchange = ?1
                  AND symbol = ?2
                  AND interval = ?3
                  AND strategy_name = ?4
                ORDER BY updated_at_ms DESC
                LIMIT 1
                "#,
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str(),
                    strategy_name
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::other)?;

        match state_json {
            Some(json) => {
                let state =
                    serde_json::from_str::<BotRunState>(&json).map_err(StorageError::other)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    fn append_order_event(&self, run_id: &str, order: &ExchangeOrder) -> Result<(), StorageError> {
        let connection = self.open()?;
        let raw_json = serde_json::to_string(order).map_err(StorageError::other)?;
        connection
            .execute(
                r#"
                INSERT INTO order_events(
                  run_id, symbol, side, order_type, status, order_id, client_order_id,
                  transact_time_ms, raw_json
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
                params![
                    run_id,
                    order.symbol.as_str(),
                    order.side.as_str(),
                    &order.order_type,
                    order.status.as_str(),
                    order.order_id,
                    order.client_order_id.as_deref(),
                    order.transact_time_ms,
                    raw_json
                ],
            )
            .map_err(StorageError::other)?;
        Ok(())
    }

    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError> {
        let connection = self.open()?;
        connection
            .execute(
                r#"
                INSERT INTO closed_trades(
                  run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
                  qty, gross_quote_pnl, entry_order_id, exit_order_id
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                "#,
                params![
                    run_id,
                    trade.symbol.as_str(),
                    trade.entry_time_ms,
                    trade.exit_time_ms,
                    trade.entry_price.to_string(),
                    trade.exit_price.to_string(),
                    trade.qty.to_string(),
                    trade.gross_quote_pnl.to_string(),
                    trade.entry_order_id,
                    trade.exit_order_id
                ],
            )
            .map_err(StorageError::other)?;
        Ok(())
    }

    fn list_order_events(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError> {
        let connection = self.open()?;
        let mut stmt = connection
            .prepare(
                r#"
                SELECT raw_json
                FROM order_events
                WHERE run_id = ?1
                ORDER BY seq DESC
                LIMIT ?2
                "#,
            )
            .map_err(StorageError::other)?;

        let rows = stmt
            .query_map(params![run_id, limit as i64], |row| row.get::<_, String>(0))
            .map_err(StorageError::other)?;

        let mut items = Vec::new();
        for row in rows {
            let raw_json = row.map_err(StorageError::other)?;
            let order =
                serde_json::from_str::<ExchangeOrder>(&raw_json).map_err(StorageError::other)?;
            items.push(order);
        }
        Ok(items)
    }

    fn list_closed_trades(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError> {
        let connection = self.open()?;
        let mut stmt = connection
            .prepare(
                r#"
                SELECT symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
                       qty, gross_quote_pnl, entry_order_id, exit_order_id
                FROM closed_trades
                WHERE run_id = ?1
                ORDER BY seq DESC
                LIMIT ?2
                "#,
            )
            .map_err(StorageError::other)?;

        let rows = stmt
            .query_map(params![run_id, limit as i64], |row| {
                Ok(ClosedTrade {
                    symbol: row
                        .get::<_, String>(0)?
                        .parse()
                        .map_err(to_from_sql_error)?,
                    entry_time_ms: row.get(1)?,
                    exit_time_ms: row.get(2)?,
                    entry_price: parse_decimal_str(&row.get::<_, String>(3)?)?,
                    exit_price: parse_decimal_str(&row.get::<_, String>(4)?)?,
                    qty: parse_decimal_str(&row.get::<_, String>(5)?)?,
                    gross_quote_pnl: parse_decimal_str(&row.get::<_, String>(6)?)?,
                    entry_order_id: row.get(7)?,
                    exit_order_id: row.get(8)?,
                })
            })
            .map_err(StorageError::other)?;

        let mut trades = Vec::new();
        for row in rows {
            trades.push(row.map_err(StorageError::other)?);
        }
        Ok(trades)
    }
}
