//! The ports a transfer between two stores goes through
//! (`docs/storage-migration.md`): a store's rows, read the same way on every
//! backend; a source read from one snapshot, which says whether anything
//! wrote to it since; and a target, which keeps a record of the copy it
//! holds. SQLite is a source and PostgreSQL a target today; ALG-768 adds the
//! other directions behind the same ports.

use super::storage::StorageError;
use crate::model::{
    Environment, StoreIdentity, TransferMarket, TransferRecord, TransferRow, TransferTable,
    TransferValue,
};

/// A store's rows, table by table, as both backends hold them: nothing
/// parsed, nothing normalized.
pub trait TransferSource: Send + Sync {
    /// The store's record: its ID, schema, environment and activation.
    fn transfer_identity(&self) -> Result<StoreIdentity, StorageError>;

    /// Every market a market table holds rows of, in any order.
    fn markets(&self, table: &TransferTable) -> Result<Vec<TransferMarket>, StorageError>;

    /// Up to `limit` rows of `table`, and of `market` for a market table,
    /// whose key comes after `after`, in the order of their keys: integers
    /// by value, text byte by byte, whatever the backend's collation.
    fn read_rows(
        &self,
        table: &TransferTable,
        market: Option<&TransferMarket>,
        after: Option<&TransferValue>,
        limit: usize,
    ) -> Result<Vec<TransferRow>, StorageError>;
}

/// The source of a copy: every read sees one snapshot of the store, taken
/// when it was opened, and nothing is ever written to it.
pub trait TransferSnapshot: TransferSource {
    /// Whether anything wrote to the store after the snapshot was taken, so
    /// that what was read is no longer all of it.
    fn moved(&self) -> Result<bool, StorageError>;
}

/// What [`TransferTarget::begin_transfer`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferBegun {
    /// The copy is recorded, and the store bound when asked.
    Recorded,
    /// Nothing was recorded: these tables, in copy order, hold rows.
    HoldingRows(Vec<&'static str>),
}

/// The target of a copy, which records what it holds. While a copy is
/// unfinished, the store takes no other write.
pub trait TransferTarget: TransferSource {
    /// The copy the store holds, if any.
    fn transfer_record(&self) -> Result<Option<TransferRecord>, StorageError>;

    /// Records the copy begun, and binds the store to `environment` when one
    /// is given, in one transaction that first finds every transferred table
    /// empty, with writers held off, so that no write slips in between.
    fn begin_transfer(
        &self,
        record: &TransferRecord,
        environment: Option<Environment>,
    ) -> Result<TransferBegun, StorageError>;

    /// Writes the rows whose keys it does not hold, keys and numbers as
    /// given, and answers how many it wrote. A row whose key it holds stays
    /// as it is, so a batch written twice is written once.
    fn write_rows(&self, table: &TransferTable, rows: &[TransferRow]) -> Result<u64, StorageError>;

    /// Moves the numbering of every numbered table past the numbers it
    /// holds, so that the next row the store numbers itself takes a new one,
    /// and records `record`, in one transaction.
    fn finish_transfer(&self, record: &TransferRecord) -> Result<(), StorageError>;
}
