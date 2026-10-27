//! Journal tables: their schema as QuantForge 0.2.0 created them and as
//! schema 5 rebuilt them, the identity every run carries beside its state,
//! and the `RunJournalStore` implementation. The order-event and
//! closed-trade journals are in [`events`].

pub(super) mod events;
mod identity;

use identity::decode_identity;
pub(super) use identity::{identity_columns, parse_environment};

use super::SqliteStore;
use crate::model::{IdentitySource, RunIdentity};
use crate::{
    BotRunState, ClosedTrade, ExchangeOrder, MarketId, RunJournalStore, StorageError, now_utc_ms,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases. This is the text 0.2.0 wrote; the
// chain's step 2 still creates the tables with it before step 5 rebuilds
// two of them.
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

// Schema 5 (QUA-37): every run carries an identity, the venue IDs of the
// two journals become text (SQLite cannot retype a column, so the tables
// are rebuilt with their rows and sequence numbers), and the original JSON
// of every row an earlier build wrote is kept beside its rewritten form.
pub(super) const IDENTITY_DDL: &str = r#"
                ALTER TABLE bot_runs ADD COLUMN environment TEXT;
                ALTER TABLE bot_runs ADD COLUMN account_kind TEXT;
                ALTER TABLE bot_runs ADD COLUMN account_id TEXT;
                ALTER TABLE bot_runs ADD COLUMN identity_source TEXT NOT NULL DEFAULT 'legacy';
                ALTER TABLE bot_runs ADD COLUMN identity_at_ms INTEGER;
                ALTER TABLE bot_runs ADD COLUMN legacy_state_json TEXT;
                UPDATE bot_runs SET legacy_state_json = state_json;

                CREATE TABLE order_events_v5 (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  run_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  side TEXT NOT NULL,
                  order_type TEXT NOT NULL,
                  status TEXT NOT NULL,
                  order_id TEXT,
                  client_order_id TEXT,
                  transact_time_ms INTEGER,
                  raw_json TEXT NOT NULL,
                  legacy_raw_json TEXT
                );
                INSERT INTO order_events_v5(
                  seq, run_id, symbol, side, order_type, status, order_id, client_order_id,
                  transact_time_ms, raw_json, legacy_raw_json
                )
                SELECT seq, run_id, symbol, side, order_type, status, CAST(order_id AS TEXT),
                       client_order_id, transact_time_ms, raw_json, raw_json
                FROM order_events;
                DROP TABLE order_events;
                ALTER TABLE order_events_v5 RENAME TO order_events;

                CREATE INDEX IF NOT EXISTS idx_order_events_run
                  ON order_events(run_id, seq DESC);

                CREATE TABLE closed_trades_v5 (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  run_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  entry_time_ms INTEGER NOT NULL,
                  exit_time_ms INTEGER NOT NULL,
                  entry_price TEXT NOT NULL,
                  exit_price TEXT NOT NULL,
                  qty TEXT NOT NULL,
                  gross_quote_pnl TEXT NOT NULL,
                  entry_order_id TEXT,
                  exit_order_id TEXT
                );
                INSERT INTO closed_trades_v5(
                  seq, run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
                  qty, gross_quote_pnl, entry_order_id, exit_order_id
                )
                SELECT seq, run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
                       qty, gross_quote_pnl, CAST(entry_order_id AS TEXT), CAST(exit_order_id AS TEXT)
                FROM closed_trades;
                DROP TABLE closed_trades;
                ALTER TABLE closed_trades_v5 RENAME TO closed_trades;

                CREATE INDEX IF NOT EXISTS idx_closed_trades_run
                  ON closed_trades(run_id, seq DESC);
"#;

/// A run's state from its JSON. A run QuantForge 0.2.0 wrote recorded no
/// execution mode, and loads only once an operator has stated it.
fn decode_state(run_id: &str, json: &str) -> Result<BotRunState, StorageError> {
    match serde_json::from_str::<BotRunState>(json) {
        Ok(state) => Ok(state),
        Err(err) => {
            let value: serde_json::Value =
                serde_json::from_str(json).map_err(StorageError::other)?;
            if value.get("execution_mode").is_none() {
                return Err(StorageError::InvalidArgument(format!(
                    "run {run_id} was recorded by QuantForge 0.2.0 without its execution mode; \
                     adopt it with `quantforge store adopt-run --run-id {run_id} --execution-mode \
                     <dry-run|live> --yes`, or start a new run"
                )));
            }
            Err(StorageError::other(err))
        }
    }
}

fn load_state(connection: &Connection, run_id: &str) -> Result<Option<BotRunState>, StorageError> {
    let state_json = connection
        .query_row(
            "SELECT state_json FROM bot_runs WHERE run_id = ?1",
            [run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StorageError::other)?;
    state_json
        .map(|json| decode_state(run_id, &json))
        .transpose()
}

impl RunJournalStore for SqliteStore {
    fn init(&self) -> Result<(), StorageError> {
        self.initialize_schema()
    }

    // `state_json` is the single source of truth for run state; the
    // `status`, `stopped_at_ms`, and `last_error` columns are denormalized
    // copies for ad-hoc SQL/display only and are never read back into a
    // `BotRunState`. The identity columns are read back, through
    // `run_identity`, and never change after `create_run`.
    fn create_run(&self, state: &BotRunState, identity: &RunIdentity) -> Result<(), StorageError> {
        if identity.source == IdentitySource::Legacy {
            return Err(StorageError::InvalidArgument(format!(
                "run {} cannot be created as a legacy run; only a migration marks runs legacy",
                state.run_id
            )));
        }
        let connection = self.open()?;
        let state_json = serde_json::to_string(state).map_err(StorageError::other)?;
        let (environment, account_kind, account_id) = identity_columns(identity);
        let inserted = connection.execute(
            r#"
            INSERT INTO bot_runs(
              run_id, exchange, symbol, interval, strategy_name, status,
              state_json, started_at_ms, updated_at_ms, stopped_at_ms, last_error,
              environment, account_kind, account_id, identity_source, identity_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                environment,
                account_kind,
                account_id,
                identity.source.as_str(),
                now_utc_ms(),
            ],
        );
        match inserted {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == ErrorCode::ConstraintViolation =>
            {
                Err(StorageError::InvalidArgument(format!(
                    "run {} already exists; resume it with --run-id, or start a new run",
                    state.run_id
                )))
            }
            Err(err) => Err(StorageError::other(err)),
        }
    }

    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError> {
        let connection = self.open()?;
        let state_json = serde_json::to_string(state).map_err(StorageError::other)?;
        let updated = connection
            .execute(
                r#"
                UPDATE bot_runs
                SET status = ?2, state_json = ?3, updated_at_ms = ?4, stopped_at_ms = ?5,
                    last_error = ?6
                WHERE run_id = ?1
                "#,
                params![
                    &state.run_id,
                    state.status.as_str(),
                    state_json,
                    state.updated_at_ms,
                    state.stopped_at_ms,
                    state.last_error.as_deref(),
                ],
            )
            .map_err(StorageError::other)?;
        if updated == 0 {
            return Err(StorageError::InvalidArgument(format!(
                "run {} was never created; a run is created with its identity before it is saved",
                state.run_id
            )));
        }
        Ok(())
    }

    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError> {
        load_state(&self.open()?, run_id)
    }

    fn run_identity(&self, run_id: &str) -> Result<Option<RunIdentity>, StorageError> {
        let connection = self.open()?;
        connection
            .query_row(
                r#"
                SELECT environment, account_kind, account_id, identity_source
                FROM bot_runs
                WHERE run_id = ?1
                "#,
                [run_id],
                |row| decode_identity(row, 0),
            )
            .optional()
            .map_err(StorageError::other)
    }

    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError> {
        let connection = self.open()?;
        let found: Option<(String, String)> = connection
            .query_row(
                r#"
                SELECT run_id, state_json
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
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StorageError::other)?;
        found
            .map(|(run_id, json)| decode_state(&run_id, &json))
            .transpose()
    }

    fn append_order_event(&self, run_id: &str, order: &ExchangeOrder) -> Result<(), StorageError> {
        events::append_order_event(self, run_id, order)
    }

    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError> {
        events::append_closed_trade(self, run_id, trade)
    }

    fn list_order_events(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError> {
        events::list_order_events(self, run_id, limit)
    }

    fn list_closed_trades(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError> {
        events::list_closed_trades(self, run_id, limit)
    }
}
