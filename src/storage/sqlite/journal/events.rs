//! The journals beside a run's state: order events and closed trades, and
//! the schema-5 rewrite that turned their venue IDs into text.

use super::super::{SqliteStore, parse_decimal_str, to_from_sql_error};
use crate::{ClosedTrade, ExchangeOrder, StorageError, VenueOrderId};
use rusqlite::{Transaction, params};
use serde_json::Value;

pub(super) fn append_order_event(
    store: &SqliteStore,
    run_id: &str,
    order: &ExchangeOrder,
) -> Result<(), StorageError> {
    let connection = store.open()?;
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
                order.order_id.as_ref().map(VenueOrderId::as_str),
                order.client_order_id.as_deref(),
                order.transact_time_ms,
                raw_json
            ],
        )
        .map_err(StorageError::other)?;
    Ok(())
}

pub(super) fn append_closed_trade(
    store: &SqliteStore,
    run_id: &str,
    trade: &ClosedTrade,
) -> Result<(), StorageError> {
    let connection = store.open()?;
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
                trade.entry_order_id.as_ref().map(VenueOrderId::as_str),
                trade.exit_order_id.as_ref().map(VenueOrderId::as_str)
            ],
        )
        .map_err(StorageError::other)?;
    Ok(())
}

pub(super) fn list_order_events(
    store: &SqliteStore,
    run_id: &str,
    limit: usize,
) -> Result<Vec<ExchangeOrder>, StorageError> {
    let connection = store.open()?;
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

pub(super) fn list_closed_trades(
    store: &SqliteStore,
    run_id: &str,
    limit: usize,
) -> Result<Vec<ClosedTrade>, StorageError> {
    let connection = store.open()?;
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
            let order_id = |index: usize| -> Result<Option<VenueOrderId>, rusqlite::Error> {
                row.get::<_, Option<String>>(index)?
                    .map(VenueOrderId::new)
                    .transpose()
                    .map_err(to_from_sql_error)
            };
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
                entry_order_id: order_id(7)?,
                exit_order_id: order_id(8)?,
            })
        })
        .map_err(StorageError::other)?;

    let mut trades = Vec::new();
    for row in rows {
        trades.push(row.map_err(StorageError::other)?);
    }
    Ok(trades)
}

/// Schema 5: the venue IDs inside `state_json` (`position.entry_order_id`)
/// and `raw_json` (`order_id`, `fills[].trade_id`) become strings, as the
/// types now read them. Every row's original JSON was copied to its
/// `legacy_*` column by the DDL before this runs; a row whose IDs are
/// already text, or absent, is left alone.
pub(in crate::storage::sqlite) fn migrate_ids(
    transaction: &Transaction<'_>,
) -> Result<(), StorageError> {
    let runs: Vec<(String, String)> = {
        let mut stmt = transaction
            .prepare("SELECT run_id, state_json FROM bot_runs")
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(StorageError::other)?;
        rows.collect::<Result<_, _>>()
            .map_err(StorageError::other)?
    };
    for (run_id, json) in runs {
        let mut state: Value = serde_json::from_str(&json).map_err(StorageError::other)?;
        let changed = state
            .get_mut("position")
            .and_then(Value::as_object_mut)
            .is_some_and(|position| number_to_text(position, "entry_order_id"));
        if changed {
            transaction
                .execute(
                    "UPDATE bot_runs SET state_json = ?2 WHERE run_id = ?1",
                    params![
                        run_id,
                        serde_json::to_string(&state).map_err(StorageError::other)?
                    ],
                )
                .map_err(StorageError::other)?;
        }
    }

    let events: Vec<(i64, String)> = {
        let mut stmt = transaction
            .prepare("SELECT seq, raw_json FROM order_events")
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(StorageError::other)?;
        rows.collect::<Result<_, _>>()
            .map_err(StorageError::other)?
    };
    for (seq, json) in events {
        let mut order: Value = serde_json::from_str(&json).map_err(StorageError::other)?;
        let Some(object) = order.as_object_mut() else {
            continue;
        };
        let mut changed = number_to_text(object, "order_id");
        if let Some(fills) = object.get_mut("fills").and_then(Value::as_array_mut) {
            for fill in fills.iter_mut().filter_map(Value::as_object_mut) {
                changed |= number_to_text(fill, "trade_id");
            }
        }
        if changed {
            transaction
                .execute(
                    "UPDATE order_events SET raw_json = ?2 WHERE seq = ?1",
                    params![
                        seq,
                        serde_json::to_string(&order).map_err(StorageError::other)?
                    ],
                )
                .map_err(StorageError::other)?;
        }
    }
    Ok(())
}

/// Turns a numeric `key` into its decimal text; `true` when it did.
fn number_to_text(object: &mut serde_json::Map<String, Value>, key: &str) -> bool {
    match object.get(key) {
        Some(Value::Number(number)) => {
            let text = number.to_string();
            object.insert(key.to_string(), Value::String(text));
            true
        }
        _ => false,
    }
}
