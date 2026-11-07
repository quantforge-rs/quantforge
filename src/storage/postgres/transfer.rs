//! A PostgreSQL store's rows for a transfer (`docs/storage-migration.md`).
//!
//! Rows are read in key order whatever the database's collation: a text key
//! sorts `COLLATE "C"`, byte by byte, as SQLite compares it, and an integer
//! column is read as `BIGINT`, however narrow. A batch is written as arrays
//! unnested into one statement that skips every key it holds already, so a
//! batch written twice is written once; a numbered column takes the
//! source's numbers (`OVERRIDING SYSTEM VALUE`), and finishing the copy
//! moves each identity past the largest. The record of the copy is the
//! `meta` row `transfer`, written with the binding in one transaction.

use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{IsolationLevel, Row};

use super::PostgresStore;
use super::bridge::Effect;
use crate::model::{
    ColumnKind, Environment, StoreIdentity, TransferMarket, TransferRecord, TransferRow,
    TransferTable, TransferValue, now_utc_ms,
};
use crate::ports::{StorageError, StoreControl, TransferSource, TransferTarget};
use crate::storage::postgres::limit;

const READ_RECORD: &str = "SELECT value FROM meta WHERE key = 'transfer'";

const BEGIN_RECORD: &str = "INSERT INTO meta (key, value) VALUES ('transfer', $1)";

const FINISH_RECORD: &str = "UPDATE meta SET value = $1 WHERE key = 'transfer'";

const BIND: &str = r#"
    UPDATE store_control SET environment = $1, changed_at_ms = $2
    WHERE id = 1 AND environment IS NULL
"#;

/// A statement's parameter, owned, so that it can travel to the bridge.
enum Param {
    Text(String),
    Integer(i64),
    Blob(Vec<u8>),
    Texts(Vec<Option<String>>),
    Integers(Vec<Option<i64>>),
    Blobs(Vec<Option<Vec<u8>>>),
}

impl Param {
    fn typed(&self) -> (&(dyn ToSql + Sync), Type) {
        match self {
            Self::Text(value) => (value, Type::TEXT),
            Self::Integer(value) => (value, Type::INT8),
            Self::Blob(value) => (value, Type::BYTEA),
            Self::Texts(values) => (values, Type::TEXT_ARRAY),
            Self::Integers(values) => (values, Type::INT8_ARRAY),
            Self::Blobs(values) => (values, Type::BYTEA_ARRAY),
        }
    }

    fn of(value: &TransferValue) -> Result<Self, StorageError> {
        match value {
            TransferValue::Integer(value) => Ok(Self::Integer(*value)),
            TransferValue::Text(text) => Ok(Self::Text(text.clone())),
            TransferValue::Blob(bytes) => Ok(Self::Blob(bytes.clone())),
            TransferValue::Null => Err(StorageError::InvalidArgument(
                "a key is never NULL".to_string(),
            )),
        }
    }
}

/// A key column as the order compares it.
fn ordered(table: &TransferTable) -> String {
    match table.key_kind() {
        ColumnKind::Text => format!("t.{} COLLATE \"C\"", table.key),
        ColumnKind::Integer | ColumnKind::Blob => format!("t.{}", table.key),
    }
}

/// The statement reading a batch of `table`.
fn select(table: &TransferTable, by_market: bool, after: bool) -> String {
    let columns: Vec<String> = table
        .columns
        .iter()
        .map(|column| match column.kind {
            ColumnKind::Integer => format!("CAST(t.{} AS BIGINT)", column.name),
            ColumnKind::Text | ColumnKind::Blob => format!("t.{}", column.name),
        })
        .collect();
    let order = ordered(table);
    let mut conditions = Vec::new();
    let mut count = 0;
    if by_market {
        conditions.push("t.exchange = $1 AND t.symbol = $2 AND t.interval = $3".to_string());
        count = 3;
    }
    if after {
        count += 1;
        conditions.push(format!("{order} > ${count}"));
    }
    let filter = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    format!(
        "SELECT {} FROM {} t{filter} ORDER BY {order} LIMIT ${}",
        columns.join(", "),
        table.name,
        count + 1
    )
}

/// The statement writing a batch of `table`: one array per column.
fn insert(table: &TransferTable) -> String {
    let names: Vec<&str> = table.columns.iter().map(|column| column.name).collect();
    let arrays: Vec<String> = table
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let kind = match column.kind {
                ColumnKind::Integer => "bigint",
                ColumnKind::Text => "text",
                ColumnKind::Blob => "bytea",
            };
            format!("${}::{kind}[]", index + 1)
        })
        .collect();
    let overriding = if table.numbered {
        " OVERRIDING SYSTEM VALUE"
    } else {
        ""
    };
    format!(
        "INSERT INTO {} ({}){overriding} SELECT * FROM unnest({}) ON CONFLICT DO NOTHING",
        table.name,
        names.join(", "),
        arrays.join(", ")
    )
}

/// The rows of a batch as one array per column, or the refusal of a value
/// its column cannot hold.
fn columns(table: &TransferTable, rows: &[TransferRow]) -> Result<Vec<Param>, StorageError> {
    let mut params = Vec::with_capacity(table.columns.len());
    for (index, column) in table.columns.iter().enumerate() {
        let values = rows
            .iter()
            .map(|row| row.get(index).unwrap_or(&TransferValue::Null));
        let mismatch = |value: &TransferValue| {
            StorageError::InvalidArgument(format!(
                "{}.{} cannot hold {}",
                table.name,
                column.name,
                value.kind_name()
            ))
        };
        params.push(match column.kind {
            ColumnKind::Integer => Param::Integers(
                values
                    .map(|value| match value {
                        TransferValue::Null => Ok(None),
                        TransferValue::Integer(value) => Ok(Some(*value)),
                        other => Err(mismatch(other)),
                    })
                    .collect::<Result<_, _>>()?,
            ),
            ColumnKind::Text => Param::Texts(
                values
                    .map(|value| match value {
                        TransferValue::Null => Ok(None),
                        TransferValue::Text(text) => Ok(Some(text.clone())),
                        other => Err(mismatch(other)),
                    })
                    .collect::<Result<_, _>>()?,
            ),
            ColumnKind::Blob => Param::Blobs(
                values
                    .map(|value| match value {
                        TransferValue::Null => Ok(None),
                        TransferValue::Blob(bytes) => Ok(Some(bytes.clone())),
                        other => Err(mismatch(other)),
                    })
                    .collect::<Result<_, _>>()?,
            ),
        });
    }
    Ok(params)
}

/// A row as the table's columns hold it.
fn decode(table: &TransferTable, row: &Row) -> Result<TransferRow, tokio_postgres::Error> {
    let mut values = Vec::with_capacity(table.columns.len());
    for (index, column) in table.columns.iter().enumerate() {
        values.push(match column.kind {
            ColumnKind::Integer => row
                .try_get::<_, Option<i64>>(index)?
                .map_or(TransferValue::Null, TransferValue::Integer),
            ColumnKind::Text => row
                .try_get::<_, Option<String>>(index)?
                .map_or(TransferValue::Null, TransferValue::Text),
            ColumnKind::Blob => row
                .try_get::<_, Option<Vec<u8>>>(index)?
                .map_or(TransferValue::Null, TransferValue::Blob),
        });
    }
    Ok(values)
}

impl TransferSource for PostgresStore {
    fn transfer_identity(&self) -> Result<StoreIdentity, StorageError> {
        self.store_identity()
    }

    fn markets(&self, table: &TransferTable) -> Result<Vec<TransferMarket>, StorageError> {
        // One indexed step per market, rather than reading every row.
        let first = format!(
            "SELECT exchange, symbol, interval FROM {} ORDER BY exchange, symbol, interval LIMIT 1",
            table.name
        );
        let next = format!(
            "SELECT exchange, symbol, interval FROM {} WHERE (exchange, symbol, interval) > \
             ($1, $2, $3) ORDER BY exchange, symbol, interval LIMIT 1",
            table.name
        );
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let mut markets: Vec<TransferMarket> = Vec::new();
                loop {
                    let rows = match markets.last() {
                        None => client.query_typed(&first, &[]).await?,
                        Some(last) => {
                            client
                                .query_typed(
                                    &next,
                                    &[
                                        (&last.exchange, Type::TEXT),
                                        (&last.symbol, Type::TEXT),
                                        (&last.interval, Type::TEXT),
                                    ],
                                )
                                .await?
                        }
                    };
                    let Some(row) = rows.first() else {
                        return Ok(markets);
                    };
                    markets.push(TransferMarket {
                        exchange: row.try_get(0)?,
                        symbol: row.try_get(1)?,
                        interval: row.try_get(2)?,
                    });
                }
            })
        })
    }

    fn read_rows(
        &self,
        table: &TransferTable,
        market: Option<&TransferMarket>,
        after: Option<&TransferValue>,
        limit_rows: usize,
    ) -> Result<Vec<TransferRow>, StorageError> {
        let sql = select(table, market.is_some(), after.is_some());
        let mut params = Vec::new();
        if let Some(market) = market {
            params.push(Param::Text(market.exchange.clone()));
            params.push(Param::Text(market.symbol.clone()));
            params.push(Param::Text(market.interval.clone()));
        }
        if let Some(after) = after {
            params.push(Param::of(after)?);
        }
        params.push(Param::Integer(limit(limit_rows)));
        let table = *table;
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let typed: Vec<_> = params.iter().map(Param::typed).collect();
                client
                    .query_typed(&sql, &typed)
                    .await?
                    .iter()
                    .map(|row| decode(&table, row))
                    .collect()
            })
        })
    }
}

impl TransferTarget for PostgresStore {
    fn transfer_record(&self) -> Result<Option<TransferRecord>, StorageError> {
        let value: Option<String> = self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move {
                match client.query_typed(READ_RECORD, &[]).await?.first() {
                    Some(row) => row.try_get(0),
                    None => Ok(None),
                }
            })
        })?;
        value
            .map(|json| {
                TransferRecord::from_json(&json)
                    .map_err(|error| StorageError::InvalidArgument(error.to_string()))
            })
            .transpose()
    }

    fn begin_transfer(
        &self,
        record: &TransferRecord,
        environment: Option<Environment>,
    ) -> Result<(), StorageError> {
        let json = record.to_json();
        let binding = environment.map(|environment| environment.as_str().to_string());
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                transaction
                    .execute_typed(BEGIN_RECORD, &[(&json, Type::TEXT)])
                    .await?;
                if let Some(binding) = &binding {
                    transaction
                        .execute_typed(BIND, &[(binding, Type::TEXT), (&now_utc_ms(), Type::INT8)])
                        .await?;
                }
                transaction.commit().await
            })
        })
    }

    fn write_rows(&self, table: &TransferTable, rows: &[TransferRow]) -> Result<u64, StorageError> {
        if rows.is_empty() {
            return Ok(0);
        }
        let sql = insert(table);
        let params = columns(table, rows)?;
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let typed: Vec<_> = params.iter().map(Param::typed).collect();
                client.execute_typed(&sql, &typed).await
            })
        })
    }

    fn finish_transfer(&self, record: &TransferRecord) -> Result<(), StorageError> {
        let json = record.to_json();
        // Each identity past the largest number its table holds; a table
        // with none keeps its own.
        let numbering: String = crate::model::TRANSFER_TABLES
            .iter()
            .filter(|table| table.numbered)
            .map(|table| {
                format!(
                    "SELECT setval(pg_get_serial_sequence('{name}', '{key}'), top) \
                     FROM (SELECT max({key}) AS top FROM {name}) AS largest \
                     WHERE top IS NOT NULL;",
                    name = table.name,
                    key = table.key
                )
            })
            .collect();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                transaction.batch_execute(&numbering).await?;
                transaction
                    .execute_typed(FINISH_RECORD, &[(&json, Type::TEXT)])
                    .await?;
                transaction.commit().await
            })
        })
    }
}
