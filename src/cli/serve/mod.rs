//! `quantforge serve` — the server other QuantForge instances connect to.

use super::context::AppContext;
use anyhow::Result;
use clap::Subcommand;

mod run;

use run::{ServeRunArgs, handle_serve_run};

#[derive(Subcommand, Debug)]
pub(crate) enum ServeCommand {
    /// Answer HTTP requests until Ctrl-C or SIGTERM, then finish the ones in progress.
    Run(ServeRunArgs),
}

impl ServeCommand {
    /// Refuses settings that name what is not there, before the store is
    /// opened.
    pub(crate) fn check(&self) -> Result<()> {
        match self {
            Self::Run(args) => args.check(),
        }
    }
}

pub(crate) async fn dispatch(ctx: &AppContext, command: ServeCommand) -> Result<()> {
    match command {
        ServeCommand::Run(args) => handle_serve_run(ctx, args).await,
    }
}
