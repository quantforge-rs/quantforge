//! The execution store on PostgreSQL, with SQLite's answers
//! (`docs/execution-store.md`). Every unit is one `READ COMMITTED`
//! transaction and one call through the bridge: it locks what the shared
//! plan names, the lease row and the pool's row among them, reads it,
//! decides, writes and commits. What a unit means is decided in
//! `storage::execution`, as on SQLite; this module runs the statements.
//!
//! Two units on one position wait on its lease row, and two that reserve
//! from one pool on the pool's row, so each decides on what the other
//! left. A row a unit finds it locks; a row it does not find it cannot
//! lock, and `READ COMMITTED` reads each of a unit's rows at a moment of
//! its own. So two units that may write a first row under one key, two
//! deliveries of one signal among them, wait for each other before they
//! read: each takes a transaction's advisory lock for every such key
//! ([`Plan::guards`]), in one order, and the second then finds what the
//! first wrote, whole. Without it, a delivery racing another could find
//! the inbox empty and the intent written, and answer that its intent
//! was stale where it was a duplicate. An insert that still meets its
//! key, a row written behind the store's back, writes nothing, and its
//! unit is decided again. A write that finds an unfinished copy is
//! refused, as every other write of the store is.
//!
//! Every unit takes its locks in one order, whatever order it lists its
//! changes in: the lease row, the keys, the pool's row, the rows it finds
//! ([`Plan::lock_order`]), then its tables as it writes them, in the
//! order a copy takes them. So two units never hold a row each that the
//! other waits for, and a unit that waits for a table a beginning copy
//! holds has written to none the copy still waits for.
//!
//! The store's activation is read after the lease row is locked, and a
//! change of activation locks the whole lease table first
//! (`activation.rs`), so a fenced unit or a lease call either ends before
//! the change or sees it. Nothing here locks the control row: a copy
//! that begins locks the transferred tables and then that row, and a
//! unit that held the row while it waited for a table would wait on the
//! copy forever, each for the other.

mod engine;
mod leases;
pub(super) mod schema;

use tokio_postgres::IsolationLevel;
use tokio_postgres::types::Type;

use super::bridge::Effect;
use super::{PostgresStore, copying};
use crate::model::TimestampMs;
use crate::model::execution::{
    Commit, ExecutionUnit, Fence, Lease, LeaseAnswer, LeaseRequest, NewOperation, OperationRecord,
    Stored,
};
use crate::model::futures::PositionKey;
use crate::ports::{ExecutionStore, StorageError};
use crate::storage::execution::lease::LeaseColumns;
use crate::storage::execution::plan::{Decision, Plan, Snapshot};
use crate::storage::execution::read::{Query, RawRow, Rows};
use crate::storage::execution::tables::PoolKey;
use crate::storage::execution::{execution_reads, lease, same_operation};
use engine::Written;
use leases::Lock;
pub(super) use leases::{hold_off_holders, standing};

/// How often a unit is decided again when another writer put a row under
/// a key it was about to write. Each time, that row is there to be read.
const PASSES: usize = 4;

/// Holds one key of [`Plan::guards`] until the transaction ends.
const HOLD_KEY: &str = "SELECT pg_advisory_xact_lock($1)";

/// Turns a refusal reached inside a transaction into the call's answer;
/// the transaction rolls back as it drops.
macro_rules! settled {
    ($outcome:expr) => {
        match $outcome {
            Ok(value) => value,
            Err(refused) => return Ok(Err(refused)),
        }
    };
}

impl Rows for PostgresStore {
    fn rows(&self, query: &Query) -> Result<Vec<RawRow>, StorageError> {
        let query = query.clone();
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move { engine::select(&**client, &query, false).await })
        })
    }

    fn pool_rows(&self, pool: &PoolKey, held: &Query) -> Result<(u64, Vec<RawRow>), StorageError> {
        let (pool, held) = (pool.clone(), held.clone());
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                // Both reads see one snapshot.
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::RepeatableRead)
                    .read_only(true)
                    .start()
                    .await?;
                let revision = engine::pool_revision(&*transaction, &pool, false).await?;
                let rows = engine::select(&*transaction, &held, false).await?;
                Ok((revision, rows))
            })
        })
    }
}

impl ExecutionStore for PostgresStore {
    fn commit(&self, unit: &ExecutionUnit) -> Result<Commit, StorageError> {
        let plan = Plan::of(unit)?;
        let unit = unit.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                for _ in 0..PASSES {
                    let transaction = client
                        .build_transaction()
                        .isolation_level(IsolationLevel::ReadCommitted)
                        .start()
                        .await?;
                    // The lease row first: see the module docs.
                    let lease = match &unit.fence {
                        Some(fence) => {
                            let row = leases::row(&*transaction, &fence.key, Lock::Own).await?;
                            settled!(row)
                        }
                        None => None,
                    };
                    for guard in &plan.guards {
                        transaction
                            .execute_typed(HOLD_KEY, &[(guard, Type::INT8)])
                            .await?;
                    }
                    let mut snapshot = Snapshot::of(settled!(standing(&*transaction).await?));
                    snapshot.lease = lease;
                    if let Some(pool) = &plan.pool {
                        snapshot.pool_revision =
                            engine::pool_revision(&*transaction, pool, true).await?;
                    }
                    snapshot.found = vec![None; plan.lookups.len()];
                    for index in plan.lock_order() {
                        let lookup = &plan.lookups[index];
                        let found = engine::select(&*transaction, lookup, true).await?;
                        snapshot.found[index] = found.into_iter().next();
                    }
                    let (writes, answer) = match settled!(plan.decide(&unit, &snapshot)) {
                        Decision::Answer(answer) => return Ok(Ok(answer)),
                        Decision::Write(writes, answer) => (writes, answer),
                    };
                    let mut clashed = false;
                    for write in &writes {
                        if engine::write(&*transaction, write).await? == Written::Clashed {
                            clashed = true;
                            break;
                        }
                    }
                    if clashed {
                        // Rolled back as it drops; decided again on what
                        // the other writer left.
                        continue;
                    }
                    // Read once the writes hold their tables.
                    if let Some(refused) = copying::refusal(&*transaction).await? {
                        return Ok(Err(refused));
                    }
                    transaction.commit().await?;
                    return Ok(Ok(answer));
                }
                Ok(Err(StorageError::Conflict(
                    "other writers kept recording under the keys this unit writes".to_string(),
                )))
            })
        })?
    }

    fn begin_operation(
        &self,
        operation: &NewOperation,
        at_ms: TimestampMs,
    ) -> Result<Stored<OperationRecord>, StorageError> {
        let asked = operation.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                for _ in 0..PASSES {
                    let transaction = client
                        .build_transaction()
                        .isolation_level(IsolationLevel::ReadCommitted)
                        .start()
                        .await?;
                    let what = format!("the {} on {}", asked.kind, asked.instrument);
                    let bound = settled!(standing(&*transaction).await?);
                    settled!(bound.keeps(asked.scope.environment(), what));
                    let held = engine::operation_under(&*transaction, &asked).await?;
                    if let Some(held) = settled!(held) {
                        return Ok(same_operation(&asked, held));
                    }
                    let numbered = engine::number_operation(&*transaction, &asked, at_ms).await?;
                    let Some(record) = settled!(numbered) else {
                        // Another writer recorded the key meanwhile.
                        continue;
                    };
                    if let Some(refused) = copying::refusal(&*transaction).await? {
                        return Ok(Err(refused));
                    }
                    transaction.commit().await?;
                    return Ok(Ok(Stored {
                        record,
                        revision: 1,
                    }));
                }
                Ok(Err(StorageError::Conflict(
                    "other writers kept recording the operation's key".to_string(),
                )))
            })
        })?
    }

    fn acquire_lease(&self, request: &LeaseRequest) -> Result<LeaseAnswer, StorageError> {
        let request = request.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                // The lease row first: see the module docs.
                leases::ensure(&*transaction, &request.key).await?;
                let row = settled!(leases::row(&*transaction, &request.key, Lock::Own).await?);
                let standing = settled!(standing(&*transaction).await?);
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                let answer = settled!(lease::acquire(&request, row.as_ref(), &standing));
                if let LeaseAnswer::Granted(granted) = &answer {
                    let columns = LeaseColumns::holding(granted);
                    leases::write(&*transaction, &columns, &granted.key).await?;
                    transaction.commit().await?;
                }
                Ok(Ok(answer))
            })
        })?
    }

    fn renew_lease(
        &self,
        fence: &Fence,
        ttl_ms: i64,
        now_ms: TimestampMs,
    ) -> Result<Option<Lease>, StorageError> {
        let fence = fence.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let row = settled!(leases::row(&*transaction, &fence.key, Lock::Own).await?);
                let standing = settled!(standing(&*transaction).await?);
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                let renewed = settled!(lease::renew(
                    &fence,
                    row.as_ref(),
                    &standing,
                    ttl_ms,
                    now_ms
                ));
                if let Some(lease) = &renewed {
                    let columns = LeaseColumns::holding(lease);
                    leases::write(&*transaction, &columns, &lease.key).await?;
                    transaction.commit().await?;
                }
                Ok(Ok(renewed))
            })
        })?
    }

    fn release_lease(&self, fence: &Fence) -> Result<bool, StorageError> {
        let fence = fence.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let row = settled!(leases::row(&*transaction, &fence.key, Lock::Own).await?);
                if !lease::releases(&fence, row.as_ref()) {
                    return Ok(Ok(false));
                }
                let last_fence = row.map_or(0, |row| row.last_fence);
                let columns = LeaseColumns::released(last_fence);
                leases::write(&*transaction, &columns, &fence.key).await?;
                transaction.commit().await?;
                Ok(Ok(true))
            })
        })?
    }

    fn lease(&self, key: &PositionKey) -> Result<Option<Lease>, StorageError> {
        let key = key.clone();
        let row = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move { leases::row(&**client, &key, Lock::None).await })
        })??;
        Ok(row.and_then(|row| row.lease))
    }

    execution_reads!();
}
