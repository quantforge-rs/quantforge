//! Reading the execution tables: the queries a backend runs, and every
//! check a stored row passes before it is a record again. A row whose
//! columns do not say what its document says is refused, never repaired:
//! it was written behind the store's back.

use super::rows;
use super::tables::{self, DELIVERIES, Named, PoolKey, Table};
use crate::model::TransferValue;
use crate::model::execution::{ExecutionScope, Record, RecordKind, SignalDelivery, Stored};
use crate::model::futures::{Delivery, SignalId};
use crate::ports::StorageError;

/// One read of one table: rows whose columns equal `equals` and, where
/// given, whose `one_of` column is one of a few fixed words, in the order
/// of `order`.
#[derive(Clone, Debug)]
pub(crate) struct Query {
    pub(crate) table: &'static Table,
    pub(crate) equals: Vec<Named>,
    pub(crate) one_of: Option<(&'static str, Vec<&'static str>)>,
    pub(crate) order: &'static str,
    pub(crate) limit: Option<usize>,
}

impl Query {
    /// The rows whose columns equal `equals`, in the order the store
    /// numbered them. A table's natural key names at most one.
    pub(crate) fn of(table: &'static Table, equals: Vec<Named>) -> Self {
        Self {
            table,
            equals,
            one_of: None,
            order: table.spec.key,
            limit: None,
        }
    }

    /// The same, of the rows whose `column` is one of `words`.
    pub(crate) fn in_states(mut self, column: &'static str, words: Vec<&'static str>) -> Self {
        self.one_of = Some((column, words));
        self
    }

    /// The same, of the first `limit` rows.
    pub(crate) fn first(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// A row as a backend read it: one value for each column of its table.
pub(crate) type RawRow = Vec<TransferValue>;

/// What reads a backend's tables for the shared reads.
pub(crate) trait Rows {
    /// Every row `query` names, in its order.
    fn rows(&self, query: &Query) -> Result<Vec<RawRow>, StorageError>;

    /// A pool's revision and the rows of `held`, read in one snapshot.
    fn pool_rows(&self, pool: &PoolKey, held: &Query) -> Result<(u64, Vec<RawRow>), StorageError>;
}

pub(crate) fn unreadable(table: &Table, why: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidArgument(format!(
        "a stored row of {} cannot be read: {why}",
        table.name()
    ))
}

fn integer(table: &Table, row: &RawRow, column: &str) -> Result<i64, StorageError> {
    match row.get(table.index(column)?) {
        Some(TransferValue::Integer(value)) => Ok(*value),
        other => Err(unreadable(
            table,
            format!("its {column} is {other:?}, not a number"),
        )),
    }
}

fn optional_text<'r>(
    table: &Table,
    row: &'r RawRow,
    column: &str,
) -> Result<Option<&'r str>, StorageError> {
    match row.get(table.index(column)?) {
        Some(TransferValue::Text(text)) => Ok(Some(text)),
        Some(TransferValue::Null) => Ok(None),
        other => Err(unreadable(
            table,
            format!("its {column} is {other:?}, not text"),
        )),
    }
}

fn required_text<'r>(
    table: &Table,
    row: &'r RawRow,
    column: &str,
) -> Result<&'r str, StorageError> {
    optional_text(table, row, column)?
        .ok_or_else(|| unreadable(table, format!("it has no {column}")))
}

/// The revision a row stands at: its own column's, or 1 for a table whose
/// rows are never replaced.
fn revision_of(table: &Table, row: &RawRow) -> Result<u64, StorageError> {
    if !table.revised {
        return Ok(1);
    }
    let revision = integer(table, row, "revision")?;
    u64::try_from(revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| unreadable(table, format!("its revision is {revision}")))
}

/// The record a row holds, with its revision, held to its own columns:
/// every column its document decides must say what the document says.
pub(crate) fn decode(table: &'static Table, row: &RawRow) -> Result<Stored<Record>, StorageError> {
    let kind = table
        .kind
        .ok_or_else(|| unreadable(table, "its rows are notes, not records"))?;
    let document = required_text(table, row, table.document)?;
    let record = Record::from_stored(kind, document).map_err(|error| unreadable(table, error))?;
    let derived = rows::derived(&record)?;
    for (index, column) in table.spec.columns.iter().enumerate() {
        if table.kept_by_store(column.name) {
            continue;
        }
        let says = derived
            .iter()
            .find(|(name, _)| *name == column.name)
            .map(|(_, value)| value);
        if says != row.get(index) {
            return Err(unreadable(
                table,
                format!(
                    "the {column} column of {kind} {name} says {held}, and its document says \
                     {says}",
                    column = column.name,
                    name = record.name(),
                    held = row.get(index).unwrap_or(&TransferValue::Null),
                    says = says.unwrap_or(&TransferValue::Null),
                ),
            ));
        }
    }
    Ok(Stored {
        revision: revision_of(table, row)?,
        record,
    })
}

/// The scope a row's first six columns hold.
fn scope_of(table: &Table, row: &RawRow) -> Result<ExecutionScope, StorageError> {
    let text = |column| required_text(table, row, column);
    let kind = match text("account_kind")? {
        tables::KIND_VENUE => "Venue",
        tables::KIND_KEY_FINGERPRINT => "KeyFingerprint",
        other => return Err(unreadable(table, format!("account kind {other:?}"))),
    };
    // The scope's own stored form reads the columns back with every check
    // a scope passes; a column spells in lowercase the word the stored
    // form capitalizes.
    let capitalized = |word: &str| {
        let mut letters = word.chars();
        letters.next().map_or_else(String::new, |first| {
            first.to_ascii_uppercase().to_string() + letters.as_str()
        })
    };
    let stored = serde_json::json!({
        "venue": capitalized(text("venue")?),
        "category": capitalized(text("category")?),
        "environment": capitalized(text("environment")?),
        "account": { kind: text("account_id")? },
        "profile": optional_text(table, row, "profile_id")?,
    });
    serde_json::from_value(stored).map_err(|error| unreadable(table, error))
}

/// A delivery's note from its row.
pub(crate) fn delivery(row: &RawRow) -> Result<SignalDelivery, StorageError> {
    let table = &DELIVERIES;
    let text = |column| required_text(table, row, column);
    let invalid = |error: crate::ModelError| unreadable(table, error);
    let recorded = text("disposition")?;
    let disposition = [Delivery::New, Delivery::Duplicate, Delivery::Conflict]
        .into_iter()
        .find(|delivery| delivery.as_str() == recorded)
        .ok_or_else(|| unreadable(table, "its disposition is none a delivery has"))?;
    Ok(SignalDelivery {
        scope: scope_of(table, row)?,
        signal: SignalId::new(text("signal_id")?).map_err(invalid)?,
        disposition,
        payload_hash: text("payload_hash")?.parse().map_err(invalid)?,
        payload: optional_text(table, row, "payload")?.map(str::to_string),
        fire_time_ms: integer(table, row, "fire_time_ms")?,
        received_at_ms: integer(table, row, "received_at_ms")?,
    })
}

/// The record a change of `kind` finds, by the key `columns` give.
pub(crate) fn lookup(kind: RecordKind, columns: &[Named]) -> Result<Query, StorageError> {
    let table = tables::table_of(kind);
    Ok(Query::of(table, table.key_of(columns)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::futures::{OwnerTag, ProfileId, ProfileIdentity};
    use crate::model::{AccountRef, Category, Environment, ExchangeId, Venue, VenueAccountId};

    /// Every venue, family, environment and kind of account reads back
    /// from its columns as the scope that wrote them.
    #[test]
    fn a_scope_reads_back_from_its_columns_whatever_it_names() {
        let table = &tables::SIGNALS;
        let mut scopes = Vec::new();
        for venue in Venue::ALL {
            for category in [Category::Linear, Category::Inverse] {
                for environment in [Environment::Testnet, Environment::Demo] {
                    for account in [
                        VenueAccountId::issued("71002233").expect("an account id"),
                        VenueAccountId::fingerprint("a key"),
                    ] {
                        let identity = ProfileIdentity::new(
                            ProfileId::new("main").expect("a profile id"),
                            AccountRef::new(venue, environment, account).expect("an account"),
                            category,
                            OwnerTag::new("k3x9p2").expect("an owner tag"),
                        )
                        .expect("an identity");
                        scopes.push(ExecutionScope::of_profile(&identity));
                    }
                }
            }
        }
        let own = VenueAccountId::fingerprint("another key");
        scopes.push(ExecutionScope::of_account(
            ExchangeId::BybitSpot,
            Environment::Mainnet,
            own,
        ));
        for scope in scopes {
            let columns = tables::scope_columns(&scope);
            let row: RawRow = table
                .spec
                .columns
                .iter()
                .map(|column| {
                    columns
                        .iter()
                        .find(|(name, _)| *name == column.name)
                        .map_or(TransferValue::Null, |(_, value)| value.clone())
                })
                .collect();
            assert_eq!(scope_of(table, &row).expect("a scope"), scope);
        }
    }
}
