//! `quantforge serve` — the server other QuantForge instances connect to.

use anyhow::Result;
use clap::Subcommand;

mod run;

use run::{ServeRunArgs, handle_serve_run};

#[derive(Subcommand, Debug)]
pub(crate) enum ServeCommand {
    /// Answer HTTP requests until Ctrl-C or SIGTERM, then finish the ones in progress.
    Run(ServeRunArgs),
}

pub(crate) async fn dispatch(command: ServeCommand) -> Result<()> {
    match command {
        ServeCommand::Run(args) => handle_serve_run(args).await,
    }
}
