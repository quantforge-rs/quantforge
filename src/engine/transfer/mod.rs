//! [`TransferEngine`]: copies one store into another and verifies the copy
//! (`docs/storage-migration.md`).
//!
//! A copy reads one snapshot of its source, table by table in the order of
//! [`TRANSFER_TABLES`] and each in key order, and writes it in batches that
//! stand alone: a batch writes only the rows the target does not hold, so a
//! copy run again after an interruption, or run twice, writes every row
//! once. The target records the copy when it begins and when it ends.
//! Before a row is written the engine refuses a target that holds anything
//! else, or the copy of another store, and a source that may still be
//! written to: one with runs not yet stopped, unless the operator says they
//! are not running. A copy that resumes first compares every row the target
//! holds with the source: an earlier attempt read an earlier snapshot, and a
//! row the source has since changed or deleted, which no batch rewrites,
//! refuses it. A source that moved while it was read fails the copy, which
//! then stays unfinished.
//!
//! A verification reads both stores side by side in key order and compares
//! every row; for each table, and each market of a market table, it reports
//! the rows each side read, the bounds of an integer key, and the checksum
//! of the canonical text, with the first differences found.

mod checks;
mod verify;

#[cfg(test)]
mod tests;

pub use verify::{DIFFERENCES_KEPT, Difference, DifferenceKind, Digest, ScopeReport, VerifyReport};

use std::fmt;
use std::time::{Duration, Instant};

use tracing::info;

use super::EngineError;
use crate::model::{
    StoreActivation, StoreIdentity, TRANSFER_TABLES, TransferMarket, TransferRecord, TransferRow,
    TransferState, TransferTable, TransferValue, now_utc_ms,
};
use crate::ports::{TransferBegun, TransferSnapshot, TransferSource, TransferTarget};
use checks::{binding, refuse_another_source, refuse_other_schemas, refused, rows_of_its_own};
use verify::Coverage;

/// The rows a batch holds unless its table holds larger ones.
pub const DEFAULT_BATCH_ROWS: usize = 5000;

/// How often a long copy says how far it has come.
const PROGRESS_EVERY: Duration = Duration::from_secs(10);

/// The most open runs a refusal names.
const RUNS_NAMED: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferConfig {
    /// Rows read and written at a time; a table of large rows takes fewer.
    pub batch_rows: usize,
    /// The operator checked that nothing writes to the source, whatever
    /// state its runs recorded: a process that died leaves its run running.
    pub assume_quiescent: bool,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            batch_rows: DEFAULT_BATCH_ROWS,
            assume_quiescent: false,
        }
    }
}

/// How a copy began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyStart {
    /// Into an empty target.
    Started,
    /// Where an earlier attempt stopped, over the rows it wrote.
    Resumed,
    /// Not at all: the target holds the finished copy, and nothing was
    /// written.
    AlreadyCopied,
}

impl CopyStart {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Resumed => "resumed",
            Self::AlreadyCopied => "already copied",
        }
    }
}

/// What a copy read from a table, and how much of it the target lacked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableCopy {
    pub table: &'static str,
    pub rows: u64,
    pub written: u64,
}

#[derive(Clone, Debug)]
pub struct TransferSummary {
    /// The source's record, as the copy found it.
    pub source: StoreIdentity,
    pub start: CopyStart,
    /// Every table, in the order copied; none when nothing was copied.
    pub tables: Vec<TableCopy>,
    /// What the target records now.
    pub record: TransferRecord,
}

/// See the module docs.
pub struct TransferEngine<'a> {
    source: &'a dyn TransferSnapshot,
    target: &'a dyn TransferTarget,
    config: TransferConfig,
}

impl fmt::Debug for TransferEngine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferEngine")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl<'a> TransferEngine<'a> {
    pub fn new(
        source: &'a dyn TransferSnapshot,
        target: &'a dyn TransferTarget,
        config: TransferConfig,
    ) -> Self {
        Self {
            source,
            target,
            config,
        }
    }

    /// Copies the source into the target, or finishes the copy an earlier
    /// attempt began; a target that holds the finished copy is left as it
    /// is. Everything is refused before a row is written: see the module
    /// docs.
    pub fn copy(&self) -> Result<TransferSummary, EngineError> {
        let source = self.source.transfer_identity()?;
        let target = self.target.transfer_identity()?;
        refuse_other_schemas(&source, &target)?;
        let earlier = self.target.transfer_record()?;
        if let Some(record) = &earlier {
            refuse_another_source(record, &source)?;
            if record.state == TransferState::Copied {
                return Ok(TransferSummary {
                    source,
                    start: CopyStart::AlreadyCopied,
                    tables: Vec::new(),
                    record: record.clone(),
                });
            }
        }
        self.refuse_open_runs()?;
        if target.activation != StoreActivation::Inactive {
            return Err(refused(format!(
                "the target is {}: a copy goes only into an inactive store, which an operator \
                 activates afterwards, on its own",
                target.activation
            )));
        }
        let binding = binding(&source, &target)?;
        let (record, start) = match earlier {
            Some(record) => {
                info!(source = %source.store_id, "checking the rows copied so far");
                self.refuse_stale_rows()?;
                (record, CopyStart::Resumed)
            }
            None => {
                let record = TransferRecord::begin(source.store_id.clone(), now_utc_ms());
                if let TransferBegun::HoldingRows(tables) =
                    self.target.begin_transfer(&record, binding)?
                {
                    return Err(rows_of_its_own(&tables));
                }
                (record, CopyStart::Started)
            }
        };
        info!(
            source = %source.store_id,
            start = start.as_str(),
            "copying the source"
        );
        let mut tables = Vec::with_capacity(TRANSFER_TABLES.len());
        for table in &TRANSFER_TABLES {
            tables.push(self.copy_table(table)?);
        }
        if self.source.moved()? {
            return Err(refused(
                "the source changed while it was copied, so the copy may lack what was written \
                 to it; stop whatever writes to the source, then run the same command again, \
                 which checks the rows copied so far against the source before it goes on"
                    .to_string(),
            ));
        }
        let record = record.copied(now_utc_ms());
        self.target.finish_transfer(&record)?;
        Ok(TransferSummary {
            source,
            start,
            tables,
            record,
        })
    }

    /// Compares every row of the finished copy with the source. Differences
    /// are the report's, not an error; a target that holds no finished copy
    /// of this source is refused, and so is a source that moved while it
    /// was read.
    pub fn verify(&self) -> Result<VerifyReport, EngineError> {
        let source = self.source.transfer_identity()?;
        match self.target.transfer_record()? {
            None => {
                return Err(refused("the target holds no copy to verify".to_string()));
            }
            Some(record) => {
                refuse_another_source(&record, &source)?;
                if record.state != TransferState::Copied {
                    return Err(refused(
                        "the copy is unfinished; finish it before verifying it".to_string(),
                    ));
                }
            }
        }
        let mut report = VerifyReport::default();
        for table in &TRANSFER_TABLES {
            verify::table(
                self.source,
                self.target,
                table,
                self.config.batch_rows,
                Coverage::Whole,
                &mut report,
            )?;
        }
        if self.source.moved()? {
            return Err(refused(
                "the source changed while it was verified; stop whatever writes to it, then \
                 verify again"
                    .to_string(),
            ));
        }
        Ok(report)
    }

    fn copy_table(&self, table: &TransferTable) -> Result<TableCopy, EngineError> {
        let limit = table.batch_rows(self.config.batch_rows);
        let mut copied = TableCopy {
            table: table.name,
            rows: 0,
            written: 0,
        };
        let stopped = |source| EngineError::CopyStopped {
            table: table.name,
            source,
        };
        let mut reported = Instant::now();
        for market in scopes(self.source, table)? {
            let mut after = None;
            loop {
                let rows = self
                    .source
                    .read_rows(table, market.as_ref(), after.as_ref(), limit)
                    .map_err(stopped)?;
                let Some(last) = rows.last() else {
                    break;
                };
                after = Some(key_of(table, last)?.clone());
                copied.written += self.target.write_rows(table, &rows).map_err(stopped)?;
                copied.rows += rows.len() as u64;
                if reported.elapsed() >= PROGRESS_EVERY {
                    info!(table = table.name, rows = copied.rows, "copying");
                    reported = Instant::now();
                }
                if rows.len() < limit {
                    break;
                }
            }
        }
        info!(
            table = table.name,
            rows = copied.rows,
            written = copied.written,
            "copied"
        );
        Ok(copied)
    }
}

/// The scopes a table is read in: each market of a market table, in byte
/// order, or the whole table.
fn scopes<S: TransferSource + ?Sized>(
    store: &S,
    table: &TransferTable,
) -> Result<Vec<Option<TransferMarket>>, EngineError> {
    if !table.by_market {
        return Ok(vec![None]);
    }
    let mut markets = store.markets(table)?;
    markets.sort();
    markets.dedup();
    Ok(markets.into_iter().map(Some).collect())
}

/// The key of a row of `table`.
fn key_of<'r>(
    table: &TransferTable,
    row: &'r TransferRow,
) -> Result<&'r TransferValue, EngineError> {
    row.get(table.key_index()).ok_or_else(|| {
        EngineError::InvalidState(format!(
            "a row of {} has no {} column",
            table.name, table.key
        ))
    })
}
