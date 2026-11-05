//! PostgreSQL, reached through the bridge of ADR 0001: tokio-postgres and
//! deadpool-postgres on a two-thread runtime the store owns, so that the
//! synchronous storage ports can call it from any context: an async
//! handler, a current-thread test, a `spawn_blocking` thread, a plain one.
//!
//! - `runner`: the runtime, and the reply channel callers block on, with
//!   the refusals for a call from the bridge's own thread or from another
//!   process.
//! - `bridge`: the bounded pool on it, each call's deadline, the query
//!   cancelled past it, and what a lost answer means for a read and for a
//!   write.
//! - `durability`: what every connection is given, and what it found.
//! - `tls`: the rustls connector.
//! - `schema`: the DDL of every schema version, and the lock `init` takes.
//! - `migrate`: the chain that creates the schema and brings an earlier one
//!   forward, one transaction per step (`docs/storage-schema.md`).
//! - `candles`: `CandleStore`, with SQLite's semantics and batches that
//!   mark the windows they change in the same transaction.
//! - `journal`: `RunJournalStore`, one statement per call.
//! - `control`: `StoreControl`, the store's record and legacy runs.
//! - `segments`: `SegmentStore`, each window and plan read in one snapshot
//!   and written back only over the revision it read.
//!
//! [`PostgresStore`] connects, answers, creates its schema, and serves
//! every storage port. A host selects it through `storage::factory`.

mod bridge;
mod candles;
mod control;
mod durability;
mod journal;
mod migrate;
mod runner;
mod schema;
mod segments;
#[cfg(test)]
mod tests;
mod tls;

use std::fmt;
use std::time::Duration;

use tracing::info;

use crate::ports::StorageError;
use crate::storage::settings::PostgresSettings;
use bridge::{Bridge, Effect};

/// A limit as PostgreSQL takes it; one beyond `i64` is no limit at all.
fn limit(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// A PostgreSQL database behind the bridge; see the module docs. Dropping
/// it closes its pool, then stops its runtime in the background, so it may
/// be dropped inside async code.
pub struct PostgresStore {
    bridge: Bridge,
    /// The connection without user or password, for `Debug`.
    shown: String,
}

impl PostgresStore {
    /// Builds the pool on the bridge and checks, from the bridge, that the
    /// database answers, so that a wrong URL, password or CA fails here.
    pub fn connect(settings: &PostgresSettings) -> Result<Self, StorageError> {
        let store = Self {
            bridge: Bridge::new(settings)?,
            shown: settings.connection.to_string(),
        };
        store.ping()?;
        Ok(store)
    }

    /// Creates the schema in a database that holds none and brings an
    /// earlier one to the version this build writes, one transaction per
    /// step under an advisory lock, so processes starting together create
    /// it once; run again, it changes nothing. A database from a newer
    /// build is refused as [`StorageError::SchemaUnsupported`], and one
    /// holding tables of ours without a recorded version is refused too,
    /// both untouched. An answer lost on the way is
    /// [`StorageError::OutcomeUnknown`], and running `init` again is safe.
    pub fn init(&self) -> Result<(), StorageError> {
        migrate::ensure_current(&self.bridge, &self.shown)?;
        info!(connection = %self.shown, "postgres store initialized");
        Ok(())
    }

    /// The same store and pool, each call through it bounded by `budget`:
    /// waiting for a connection, for locks and for statements together. It
    /// shares the pool's connections, so it costs no connection of its own.
    pub fn within(&self, budget: Duration) -> Self {
        Self {
            bridge: self.bridge.within(budget),
            shown: self.shown.clone(),
        }
    }

    /// Runs `SELECT 1` on a pooled connection.
    pub fn ping(&self) -> Result<(), StorageError> {
        self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move { client.batch_execute("SELECT 1").await })
        })
    }
}

impl fmt::Debug for PostgresStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresStore")
            .field("connection", &self.shown)
            .field("bridge", &self.bridge)
            .finish()
    }
}
