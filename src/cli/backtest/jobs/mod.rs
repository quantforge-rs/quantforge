//! `quantforge backtest jobs` — the jobs a QuantForge server holds.

use anyhow::Result;
use clap::Subcommand;

mod status;

use status::{StatusArgs, handle_status};

#[derive(Subcommand, Debug)]
pub(crate) enum JobsCommand {
    /// Where a job stands and, once it has succeeded, its report; --follow waits for it.
    Status(StatusArgs),
}

pub(super) async fn dispatch(command: &JobsCommand) -> Result<()> {
    match command {
        JobsCommand::Status(args) => handle_status(args).await,
    }
}
