//! `quantforge store` — the database's own record: the environment it is
//! bound to, its identity and activation, and the runs an earlier release
//! wrote without a verified identity.

use super::context::AppContext;
use anyhow::Result;
use clap::Subcommand;

mod adopt_run;
mod bind;
mod status;

use adopt_run::{StoreAdoptRunArgs, handle_store_adopt_run};
use bind::{StoreBindArgs, handle_store_bind};
use status::{StoreStatusArgs, handle_store_status};

#[derive(Subcommand, Debug)]
pub(crate) enum StoreCommand {
    /// Show the store's identity, environment, activation, migrations and legacy runs.
    Status(StoreStatusArgs),

    /// Bind the store to the environment its connectors reach, once and for life.
    Bind(StoreBindArgs),

    /// Give a legacy run the store's environment and this invocation's account.
    AdoptRun(StoreAdoptRunArgs),
}

pub(crate) fn dispatch(ctx: &AppContext, command: StoreCommand) -> Result<()> {
    match command {
        StoreCommand::Status(args) => handle_store_status(ctx, args),
        StoreCommand::Bind(args) => handle_store_bind(ctx, args),
        StoreCommand::AdoptRun(args) => handle_store_adopt_run(ctx, args),
    }
}
