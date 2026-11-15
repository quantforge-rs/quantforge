//! The execution store on SQLite (`docs/execution-store.md`): the signal
//! inbox, the operations, the ledger, the outbox and the leases. Every
//! unit is one `BEGIN IMMEDIATE` transaction, in which the rows the shared
//! plan names are read, the unit decided, and its writes made: SQLite's
//! one writer makes what was read and what is written one step. What a
//! unit means is decided in `storage::execution`, as on PostgreSQL.

mod engine;
mod leases;
mod schema;

pub(super) use schema::SCHEMA_DDL;

use rusqlite::TransactionBehavior;

use super::SqliteStore;
use super::errors::{commit_failed, failed};
use crate::model::TimestampMs;
use crate::model::execution::{
    Commit, ExecutionUnit, Fence, Lease, LeaseAnswer, LeaseRequest, NewOperation, OperationRecord,
    Stored,
};
use crate::model::futures::PositionKey;
use crate::ports::{ExecutionStore, StorageError};
use crate::storage::execution::plan::{Decision, Plan, Snapshot};
use crate::storage::execution::read::{Query, RawRow, Rows};
use crate::storage::execution::tables::PoolKey;
use crate::storage::execution::{execution_reads, lease, same_operation};

impl Rows for SqliteStore {
    fn rows(&self, query: &Query) -> Result<Vec<RawRow>, StorageError> {
        engine::select(&self.open()?, query)
    }

    fn pool_rows(&self, pool: &PoolKey, held: &Query) -> Result<(u64, Vec<RawRow>), StorageError> {
        // A deferred transaction: both reads see one snapshot.
        let mut connection = self.open()?;
        let transaction = connection.transaction().map_err(failed)?;
        let revision = engine::pool_revision(&transaction, pool)?;
        let rows = engine::select(&transaction, held)?;
        Ok((revision, rows))
    }
}

impl ExecutionStore for SqliteStore {
    fn commit(&self, unit: &ExecutionUnit) -> Result<Commit, StorageError> {
        let plan = Plan::of(unit)?;
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let mut snapshot = Snapshot::of(leases::standing(&transaction)?);
        if let Some(fence) = &unit.fence {
            snapshot.lease = leases::row(&transaction, &fence.key)?;
        }
        if let Some(pool) = &plan.pool {
            snapshot.pool_revision = engine::pool_revision(&transaction, pool)?;
        }
        snapshot.found = vec![None; plan.lookups.len()];
        for index in plan.lock_order() {
            let found = engine::select(&transaction, &plan.lookups[index])?;
            snapshot.found[index] = found.into_iter().next();
        }
        match plan.decide(unit, &snapshot)? {
            // Nothing written: the transaction ends as it drops.
            Decision::Answer(answer) => Ok(answer),
            Decision::Write(writes, answer) => {
                for write in &writes {
                    engine::write(&transaction, write)?;
                }
                transaction.commit().map_err(commit_failed)?;
                Ok(answer)
            }
        }
    }

    fn begin_operation(
        &self,
        operation: &NewOperation,
        at_ms: TimestampMs,
    ) -> Result<Stored<OperationRecord>, StorageError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let what = format!("the {} on {}", operation.kind, operation.instrument);
        leases::standing(&transaction)?.keeps(operation.scope.environment(), what)?;
        if let Some(held) = engine::operation_under(&transaction, operation)? {
            return same_operation(operation, held);
        }
        let record = engine::number_operation(&transaction, operation, at_ms)?;
        transaction.commit().map_err(commit_failed)?;
        Ok(Stored {
            record,
            revision: 1,
        })
    }

    fn acquire_lease(&self, request: &LeaseRequest) -> Result<LeaseAnswer, StorageError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let standing = leases::standing(&transaction)?;
        let row = leases::row(&transaction, &request.key)?;
        let answer = lease::acquire(request, row.as_ref(), &standing)?;
        if let LeaseAnswer::Granted(granted) = &answer {
            leases::write(
                &transaction,
                &lease::LeaseColumns::holding(granted),
                &granted.key,
            )?;
            transaction.commit().map_err(commit_failed)?;
        }
        Ok(answer)
    }

    fn renew_lease(
        &self,
        fence: &Fence,
        ttl_ms: i64,
        now_ms: TimestampMs,
    ) -> Result<Option<Lease>, StorageError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let standing = leases::standing(&transaction)?;
        let row = leases::row(&transaction, &fence.key)?;
        let renewed = lease::renew(fence, row.as_ref(), &standing, ttl_ms, now_ms)?;
        if let Some(lease) = &renewed {
            leases::write(
                &transaction,
                &lease::LeaseColumns::holding(lease),
                &lease.key,
            )?;
            transaction.commit().map_err(commit_failed)?;
        }
        Ok(renewed)
    }

    fn release_lease(&self, fence: &Fence) -> Result<bool, StorageError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let row = leases::row(&transaction, &fence.key)?;
        if !lease::releases(fence, row.as_ref()) {
            return Ok(false);
        }
        let last_fence = row.map_or(0, |row| row.last_fence);
        leases::write(
            &transaction,
            &lease::LeaseColumns::released(last_fence),
            &fence.key,
        )?;
        transaction.commit().map_err(commit_failed)?;
        Ok(true)
    }

    fn lease(&self, key: &PositionKey) -> Result<Option<Lease>, StorageError> {
        Ok(leases::row(&self.open()?, key)?.and_then(|row| row.lease))
    }

    execution_reads!();
}
