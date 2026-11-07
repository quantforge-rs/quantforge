//! `quantforge data migrate` — copy a SQLite store into the selected
//! PostgreSQL store and, with `--verify`, compare every row of the copy with
//! its source (`docs/storage-migration.md`). The source is opened read-only
//! and read from one snapshot. A copy cut off is finished by running the
//! same command again, and a finished one is only verified.

use crate::cli::common::describe_transfer;
use crate::cli::context::AppContext;
use anyhow::{Context, Result, bail};
use clap::Args;
use quantforge::{
    DEFAULT_BATCH_ROWS, Digest, StorageBackend, StorageSettings, TransferConfig, TransferEngine,
    VerifyReport,
};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct DataMigrateArgs {
    /// SQLite database to copy; opened read-only and never changed
    #[arg(long, value_name = "PATH")]
    from: PathBuf,

    /// After copying, compare every row of the copy with the source; any difference exits non-zero
    #[arg(long, default_value_t = false)]
    verify: bool,

    /// Rows read and written at a time
    #[arg(
        long,
        value_name = "ROWS",
        default_value_t = DEFAULT_BATCH_ROWS as u64,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    batch_size: u64,

    /// Copy although the source records runs still starting or running, once you have checked that nothing writes to it
    #[arg(long, default_value_t = false)]
    assume_quiescent: bool,
}

impl DataMigrateArgs {
    /// What the store cannot be opened for: a copy goes into PostgreSQL
    /// only, from a file that exists. Checked before anything is opened.
    pub(crate) fn check(&self, storage: &StorageSettings) -> Result<()> {
        if storage.backend() != StorageBackend::Postgres {
            bail!(
                "data migrate copies into PostgreSQL: select it with --storage postgres and \
                 --postgres-url, or QF_STORAGE and QF_POSTGRES_URL"
            );
        }
        if !self.from.is_file() {
            bail!("no SQLite database at {} (--from)", self.from.display());
        }
        Ok(())
    }
}

pub(crate) fn handle_data_migrate(ctx: &AppContext, args: DataMigrateArgs) -> Result<()> {
    let target = ctx
        .store
        .transfer_target()
        .context("data migrate copies into PostgreSQL only")?;
    let source = ctx.open_snapshot(&args.from)?;
    let engine = TransferEngine::new(
        source.as_ref(),
        target,
        TransferConfig {
            batch_rows: usize::try_from(args.batch_size).unwrap_or(usize::MAX),
            assume_quiescent: args.assume_quiescent,
        },
    );
    println!("storage: {}", ctx.storage);
    println!("source: {}", args.from.display());

    let summary = engine.copy()?;
    println!("source_store_id: {}", summary.source.store_id);
    println!("copy: {}", summary.start.as_str());
    for table in &summary.tables {
        println!(
            "copied: {} rows={} written={}",
            table.table, table.rows, table.written
        );
    }
    println!("transfer: {}", describe_transfer(Some(&summary.record)));
    if !args.verify {
        return Ok(());
    }

    let report = engine.verify()?;
    print_report(&report);
    if !report.is_ok() {
        bail!(
            "verification failed: the target differs from the source in {} row(s), listed above; \
             the source is untouched, so a new copy into an empty database starts over cleanly",
            report.difference_count
        );
    }
    Ok(())
}

/// Each table and market as both sides read it, then the differences.
fn print_report(report: &VerifyReport) {
    for scope in &report.scopes {
        let name = match &scope.market {
            Some(market) => format!("{} {market}", scope.table),
            None => scope.table.to_string(),
        };
        if scope.matches() {
            println!("verified: {name} {}", digest_text(scope.key, &scope.source));
        } else {
            println!(
                "mismatch: {name} source {}; target {}",
                digest_text(scope.key, &scope.source),
                digest_text(scope.key, &scope.target)
            );
        }
    }
    println!("differences: {}", report.difference_count);
    for difference in &report.differences {
        println!("  {difference}");
    }
    let shown = report.differences.len() as u64;
    if report.difference_count > shown {
        println!("  ... ({} more)", report.difference_count - shown);
    }
    println!("verify: {}", if report.is_ok() { "ok" } else { "failed" });
}

/// `rows=N`, the key's bounds when it is an integer, and the checksum.
fn digest_text(key: &str, digest: &Digest) -> String {
    let bounds = match (&digest.first, &digest.last) {
        (Some(first), Some(last)) => format!(" {key}={first}..{last}"),
        _ => String::new(),
    };
    format!("rows={}{bounds} checksum={}", digest.rows, digest.checksum)
}
