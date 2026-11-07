//! `quantforge data` — historical and incremental data ingestion into the
//! store, stored-candle validation, and the copy of a SQLite store into
//! PostgreSQL.

use super::context::AppContext;
use anyhow::Result;
use clap::Subcommand;
use quantforge::StorageSettings;

mod migrate;
mod sync;
mod validate;

use migrate::{DataMigrateArgs, handle_data_migrate};
use sync::{DataSyncArgs, handle_data_sync};
use validate::{DataValidateArgs, handle_data_validate};

#[derive(Subcommand, Debug)]
pub(crate) enum DataCommand {
    /// Sync candles from the exchange into the selected store.
    Sync(DataSyncArgs),

    /// Validate stored candles for duplicates, gaps, ordering, and OHLC sanity.
    Validate(DataValidateArgs),

    /// Copy a SQLite store into the selected PostgreSQL store, and verify the copy.
    Migrate(DataMigrateArgs),
}

impl DataCommand {
    /// Refuses what the command cannot do with the selected storage, before
    /// the store is opened.
    pub(crate) fn check_storage(&self, storage: &StorageSettings) -> Result<()> {
        match self {
            Self::Migrate(args) => args.check(storage),
            Self::Sync(_) | Self::Validate(_) => Ok(()),
        }
    }

    /// Whether the command may open a store that holds an unfinished copy:
    /// only the copy itself, to finish it.
    pub(crate) fn opens_an_unfinished_copy(&self) -> bool {
        matches!(self, Self::Migrate(_))
    }
}

pub(crate) async fn dispatch(ctx: &AppContext, command: DataCommand) -> Result<()> {
    match command {
        DataCommand::Sync(args) => {
            ctx.verify_environment()?;
            handle_data_sync(ctx, args).await
        }
        DataCommand::Validate(args) => handle_data_validate(ctx, args),
        DataCommand::Migrate(args) => handle_data_migrate(ctx, args),
    }
}
