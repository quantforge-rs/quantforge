//! The guard an unfinished copy keeps on its store
//! (`docs/storage-migration.md`): while the `meta` row `transfer` records a
//! copy `copying`, every write but the copy's own is refused, from any
//! process, so one that had the store open before the copy began cannot
//! slip rows into it.
//!
//! A copy begins only after [`hold_off_writers`] finds every transferred
//! table empty with those tables locked against writers, and a write reads
//! the guard once it holds the tables it writes: so a write either commits
//! before the copy looks, and the copy refuses the store, or waits for the
//! copy's record and finds it. A single statement carries the guard in its
//! own condition ([`not_copying!`]), since PostgreSQL locks a statement's
//! tables before it takes the statement's snapshot; a transaction asks
//! [`refusal`] after its first write.

use tokio_postgres::{Error, GenericClient};

use crate::model::{TRANSFER_TABLES, TransferRecord, TransferState};
use crate::ports::StorageError;

/// The condition a guarded statement carries: no copy is unfinished. The
/// state is the record's serde token.
macro_rules! not_copying {
    () => {
        "NOT EXISTS (SELECT 1 FROM meta WHERE key = 'transfer' \
         AND value::jsonb ->> 'state' = 'Copying')"
    };
}
pub(super) use not_copying;

const RECORD: &str = "SELECT value FROM meta WHERE key = 'transfer'";

/// Held while a copy begins, so that `store bind` waits for its record.
const LOCK_CONTROL: &str = "SELECT id FROM store_control WHERE id = 1 FOR UPDATE";

/// What a copy runs in the transaction that records it, before the record:
/// every transferred table locked `SHARE`, which waits for each writer
/// holding one and holds off the rest until the record is in, the store's
/// record row locked, and the tables that hold rows found. Answers those
/// tables, in copy order; the copy is recorded only when there are none.
pub(super) async fn hold_off_writers<C: GenericClient>(
    transaction: &C,
) -> Result<Vec<&'static str>, Error> {
    let names: Vec<&'static str> = TRANSFER_TABLES.iter().map(|table| table.name).collect();
    let lock = format!("LOCK TABLE {} IN SHARE MODE", names.join(", "));
    transaction.batch_execute(&lock).await?;
    transaction.query_typed(LOCK_CONTROL, &[]).await?;
    let holding = names
        .iter()
        .map(|name| format!("SELECT '{name}' WHERE EXISTS (SELECT 1 FROM {name})"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let mut found = Vec::new();
    for row in transaction.query_typed(&holding, &[]).await? {
        found.push(row.try_get::<_, String>(0)?);
    }
    Ok(names
        .into_iter()
        .filter(|name| found.iter().any(|holds| holds == name))
        .collect())
}

/// The refusal of a write to a store whose copy is unfinished, or `None`
/// when the store takes the write.
pub(super) async fn refusal<C: GenericClient>(client: &C) -> Result<Option<StorageError>, Error> {
    let Some(row) = client.query_typed_opt(RECORD, &[]).await? else {
        return Ok(None);
    };
    let json: String = row.try_get(0)?;
    Ok(match TransferRecord::from_json(&json) {
        Ok(record) if record.state == TransferState::Copying => {
            Some(StorageError::InvalidArgument(format!(
                "the store holds an unfinished copy of store {}, and takes no other write until \
                 it finishes; stop this process, finish the copy with `quantforge data migrate \
                 --from <the source>`, then start it again",
                record.source_store_id
            )))
        }
        Ok(_) => None,
        Err(error) => Some(StorageError::InvalidArgument(error.to_string())),
    })
}

/// A guarded statement's answer for the `rows` it wrote: none means the
/// store's copy was unfinished, or, when none is now, `missing`.
pub(super) async fn written<C: GenericClient>(
    client: &C,
    rows: u64,
    missing: impl FnOnce() -> StorageError,
) -> Result<Result<(), StorageError>, Error> {
    if rows > 0 {
        return Ok(Ok(()));
    }
    Ok(Err(refusal(client).await?.unwrap_or_else(missing)))
}

/// What a guarded insert answers when it wrote nothing although no copy is
/// unfinished now: the copy finished in between.
pub(super) fn finished_since() -> StorageError {
    StorageError::InvalidArgument(
        "the store held an unfinished copy when this write came, and took nothing of it; the \
         copy has finished since, so the write can be made again"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use crate::model::{StoreId, TransferRecord};

    #[test]
    fn the_guard_reads_the_state_a_copy_records_while_unfinished() {
        let store_id = StoreId::new("0123456789abcdef0123456789abcdef").expect("id");
        let begun = TransferRecord::begin(store_id, 1);
        assert!(begun.to_json().contains(r#""state":"Copying""#));
        assert!(not_copying!().contains("'state' = 'Copying'"));
        assert!(!begun.copied(2).to_json().contains("Copying"));
    }
}
