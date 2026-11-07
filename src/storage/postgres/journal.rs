//! The run journal on PostgreSQL: `RunJournalStore` with SQLite's rules
//! and messages, shared through `storage::runs`.
//!
//! Every call is one statement, so each write is atomic on its own: a run
//! is created once, saved only after it is created, and keeps the identity
//! it was created with; an order event or a closed trade is appended as a
//! row of its own, duplicates kept, numbered by an identity column that
//! never reuses a value; no write is taken while a copy is unfinished
//! (`copying`). Nothing is retried: a lost answer is `OutcomeUnknown`, for
//! the caller to look at before writing again ("Writing the journal" in
//! `docs/storage-schema.md`). Run IDs sort by their bytes (`COLLATE "C"`),
//! as SQLite sorts them, whatever the database's collation.

use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;

use super::bridge::Effect;
use super::{PostgresStore, copying, limit};
use crate::model::RunIdentity;
use crate::storage::common::read_decimal;
use crate::storage::runs::{
    already_exists, decode_state, identity_columns, identity_from_columns, never_created,
    refuse_legacy_creation,
};
use crate::{
    BotRunState, ClosedTrade, ExchangeOrder, MarketId, RunJournalStore, StorageError, Symbol,
    VenueOrderId, now_utc_ms,
};

const CREATE: &str = concat!(
    r#"
    INSERT INTO bot_runs(
      run_id, exchange, symbol, interval, strategy_name, status,
      state_json, started_at_ms, updated_at_ms, stopped_at_ms, last_error,
      environment, account_kind, account_id, identity_source, identity_at_ms
    )
    SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16
    WHERE "#,
    copying::not_copying!()
);

// `state_json` is the single source of truth for run state; `status`,
// `stopped_at_ms` and `last_error` are copies for SQL and display, never
// read back. The identity columns never change after `create_run`.
const SAVE: &str = concat!(
    r#"
    UPDATE bot_runs
    SET status = $2, state_json = $3, updated_at_ms = $4, stopped_at_ms = $5, last_error = $6
    WHERE run_id = $1 AND "#,
    copying::not_copying!()
);

const LOAD: &str = "SELECT state_json FROM bot_runs WHERE run_id = $1";

const IDENTITY: &str =
    "SELECT environment, account_kind, account_id, identity_source FROM bot_runs WHERE run_id = $1";

const LATEST: &str = r#"
    SELECT run_id, state_json
    FROM bot_runs
    WHERE exchange = $1 AND symbol = $2 AND interval = $3 AND strategy_name = $4
    ORDER BY updated_at_ms DESC, started_at_ms DESC, run_id COLLATE "C" DESC
    LIMIT 1
"#;

const APPEND_EVENT: &str = concat!(
    r#"
    INSERT INTO order_events(
      run_id, symbol, side, order_type, status, order_id, client_order_id,
      transact_time_ms, raw_json
    )
    SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9
    WHERE "#,
    copying::not_copying!()
);

const APPEND_TRADE: &str = concat!(
    r#"
    INSERT INTO closed_trades(
      run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
      qty, gross_quote_pnl, entry_order_id, exit_order_id
    )
    SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
    WHERE "#,
    copying::not_copying!()
);

const EVENTS: &str =
    "SELECT raw_json FROM order_events WHERE run_id = $1 ORDER BY seq DESC LIMIT $2";

const TRADES: &str = r#"
    SELECT symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
           qty, gross_quote_pnl, entry_order_id, exit_order_id
    FROM closed_trades WHERE run_id = $1 ORDER BY seq DESC LIMIT $2
"#;

/// A closed trade's row before its symbol, decimals and IDs are read.
struct StoredTrade {
    symbol: String,
    entry_time_ms: i64,
    exit_time_ms: i64,
    decimals: [String; 4],
    entry_order_id: Option<String>,
    exit_order_id: Option<String>,
}

impl StoredTrade {
    fn trade(self) -> Result<ClosedTrade, StorageError> {
        let [entry_price, exit_price, qty, gross_quote_pnl] = self.decimals;
        let order_id = |id: Option<String>| {
            id.map(VenueOrderId::new)
                .transpose()
                .map_err(StorageError::other)
        };
        Ok(ClosedTrade {
            symbol: self.symbol.parse::<Symbol>().map_err(StorageError::other)?,
            entry_time_ms: self.entry_time_ms,
            exit_time_ms: self.exit_time_ms,
            entry_price: read_decimal(&entry_price)?,
            exit_price: read_decimal(&exit_price)?,
            qty: read_decimal(&qty)?,
            gross_quote_pnl: read_decimal(&gross_quote_pnl)?,
            entry_order_id: order_id(self.entry_order_id)?,
            exit_order_id: order_id(self.exit_order_id)?,
        })
    }
}

impl RunJournalStore for PostgresStore {
    fn init(&self) -> Result<(), StorageError> {
        PostgresStore::init(self)
    }

    fn create_run(&self, state: &BotRunState, identity: &RunIdentity) -> Result<(), StorageError> {
        refuse_legacy_creation(state, identity)?;
        let state_json = serde_json::to_string(state).map_err(StorageError::other)?;
        let (environment, account_kind, account_id) = identity_columns(identity);
        let account_id = account_id.map(str::to_string);
        let state = state.clone();
        let source = identity.source.as_str();
        let identity_at_ms = now_utc_ms();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let inserted = client
                    .execute_typed(
                        CREATE,
                        &[
                            (&state.run_id, Type::TEXT),
                            (&state.market.exchange.as_str(), Type::TEXT),
                            (&state.market.symbol.as_str(), Type::TEXT),
                            (&state.market.interval.as_str(), Type::TEXT),
                            (&state.strategy_name, Type::TEXT),
                            (&state.status.as_str(), Type::TEXT),
                            (&state_json, Type::TEXT),
                            (&state.started_at_ms, Type::INT8),
                            (&state.updated_at_ms, Type::INT8),
                            (&state.stopped_at_ms, Type::INT8),
                            (&state.last_error, Type::TEXT),
                            (&environment, Type::TEXT),
                            (&account_kind, Type::TEXT),
                            (&account_id, Type::TEXT),
                            (&source, Type::TEXT),
                            (&identity_at_ms, Type::INT8),
                        ],
                    )
                    .await;
                match inserted {
                    Ok(rows) => copying::written(&**client, rows, copying::finished_since).await,
                    Err(error) if error.code() == Some(&SqlState::UNIQUE_VIOLATION) => {
                        Ok(Err(already_exists(&state.run_id)))
                    }
                    Err(error) => Err(error),
                }
            })
        })?
    }

    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError> {
        let state_json = serde_json::to_string(state).map_err(StorageError::other)?;
        let state = state.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let updated = client
                    .execute_typed(
                        SAVE,
                        &[
                            (&state.run_id, Type::TEXT),
                            (&state.status.as_str(), Type::TEXT),
                            (&state_json, Type::TEXT),
                            (&state.updated_at_ms, Type::INT8),
                            (&state.stopped_at_ms, Type::INT8),
                            (&state.last_error, Type::TEXT),
                        ],
                    )
                    .await?;
                copying::written(&**client, updated, || never_created(&state.run_id)).await
            })
        })?
    }

    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError> {
        let id = run_id.to_string();
        let json: Option<String> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed_opt(LOAD, &[(&id, Type::TEXT)])
                    .await?
                    .map(|row| row.try_get(0))
                    .transpose()
            })
        })?;
        json.map(|json| decode_state(run_id, &json)).transpose()
    }

    fn run_identity(&self, run_id: &str) -> Result<Option<RunIdentity>, StorageError> {
        let id = run_id.to_string();
        type Columns = (Option<String>, Option<String>, Option<String>, String);
        let columns: Option<Columns> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let Some(row) = client
                    .query_typed_opt(IDENTITY, &[(&id, Type::TEXT)])
                    .await?
                else {
                    return Ok(None);
                };
                Ok(Some((
                    row.try_get(0)?,
                    row.try_get(1)?,
                    row.try_get(2)?,
                    row.try_get(3)?,
                )))
            })
        })?;
        columns
            .map(|(environment, kind, id, source)| {
                identity_from_columns(
                    environment.as_deref(),
                    kind.as_deref(),
                    id.as_deref(),
                    &source,
                )
                // Columns no build writes, as SQLite reports them.
                .map_err(StorageError::other)
            })
            .transpose()
    }

    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError> {
        let key = [
            market.exchange.as_str().to_string(),
            market.symbol.as_str().to_string(),
            market.interval.as_str().to_string(),
            strategy_name.to_string(),
        ];
        let found: Option<(String, String)> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let [exchange, symbol, interval, strategy] = &key;
                client
                    .query_typed_opt(
                        LATEST,
                        &[
                            (exchange, Type::TEXT),
                            (symbol, Type::TEXT),
                            (interval, Type::TEXT),
                            (strategy, Type::TEXT),
                        ],
                    )
                    .await?
                    .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
                    .transpose()
            })
        })?;
        found
            .map(|(run_id, json)| decode_state(&run_id, &json))
            .transpose()
    }

    fn append_order_event(&self, run_id: &str, order: &ExchangeOrder) -> Result<(), StorageError> {
        let raw_json = serde_json::to_string(order).map_err(StorageError::other)?;
        let run_id = run_id.to_string();
        let order = order.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let appended = client
                    .execute_typed(
                        APPEND_EVENT,
                        &[
                            (&run_id, Type::TEXT),
                            (&order.symbol.as_str(), Type::TEXT),
                            (&order.side.as_str(), Type::TEXT),
                            (&order.order_type, Type::TEXT),
                            (&order.status.as_str(), Type::TEXT),
                            (
                                &order.order_id.as_ref().map(VenueOrderId::as_str),
                                Type::TEXT,
                            ),
                            (&order.client_order_id, Type::TEXT),
                            (&order.transact_time_ms, Type::INT8),
                            (&raw_json, Type::TEXT),
                        ],
                    )
                    .await?;
                copying::written(&**client, appended, copying::finished_since).await
            })
        })?
    }

    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError> {
        let run_id = run_id.to_string();
        let trade = trade.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let appended = client
                    .execute_typed(
                        APPEND_TRADE,
                        &[
                            (&run_id, Type::TEXT),
                            (&trade.symbol.as_str(), Type::TEXT),
                            (&trade.entry_time_ms, Type::INT8),
                            (&trade.exit_time_ms, Type::INT8),
                            (&trade.entry_price.to_string(), Type::TEXT),
                            (&trade.exit_price.to_string(), Type::TEXT),
                            (&trade.qty.to_string(), Type::TEXT),
                            (&trade.gross_quote_pnl.to_string(), Type::TEXT),
                            (
                                &trade.entry_order_id.as_ref().map(VenueOrderId::as_str),
                                Type::TEXT,
                            ),
                            (
                                &trade.exit_order_id.as_ref().map(VenueOrderId::as_str),
                                Type::TEXT,
                            ),
                        ],
                    )
                    .await?;
                copying::written(&**client, appended, copying::finished_since).await
            })
        })?
    }

    fn list_order_events(
        &self,
        run_id: &str,
        count: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError> {
        let id = run_id.to_string();
        let count = limit(count);
        let rows: Vec<String> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed(EVENTS, &[(&id, Type::TEXT), (&count, Type::INT8)])
                    .await?
                    .iter()
                    .map(|row| row.try_get(0))
                    .collect()
            })
        })?;
        rows.iter()
            .map(|json| serde_json::from_str::<ExchangeOrder>(json).map_err(StorageError::other))
            .collect()
    }

    fn list_closed_trades(
        &self,
        run_id: &str,
        count: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError> {
        let id = run_id.to_string();
        let count = limit(count);
        let rows: Vec<StoredTrade> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed(TRADES, &[(&id, Type::TEXT), (&count, Type::INT8)])
                    .await?
                    .iter()
                    .map(|row| {
                        Ok(StoredTrade {
                            symbol: row.try_get(0)?,
                            entry_time_ms: row.try_get(1)?,
                            exit_time_ms: row.try_get(2)?,
                            decimals: [
                                row.try_get(3)?,
                                row.try_get(4)?,
                                row.try_get(5)?,
                                row.try_get(6)?,
                            ],
                            entry_order_id: row.try_get(7)?,
                            exit_order_id: row.try_get(8)?,
                        })
                    })
                    .collect()
            })
        })?;
        rows.into_iter().map(StoredTrade::trade).collect()
    }
}
