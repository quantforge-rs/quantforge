//! `quantforge store status` — what the store knows about itself: its ID,
//! schema, environment and activation, the schema steps it went through,
//! and the legacy runs still waiting for an operator. Needs no credentials
//! and sends nothing.

use crate::cli::context::AppContext;
use anyhow::Result;
use clap::Args;
use quantforge::{StoreControl, ms_to_rfc3339};

#[derive(Args, Debug)]
pub(crate) struct StoreStatusArgs {}

pub(crate) fn handle_store_status(ctx: &AppContext, _args: StoreStatusArgs) -> Result<()> {
    let identity = &ctx.identity;
    println!("store_id: {}", identity.store_id);
    println!("schema_version: {}", identity.schema_version);
    println!("environment: {}", ctx.environment_text());
    println!("activation: {}", identity.activation);
    println!("activation_epoch: {}", identity.activation_epoch.0);

    let migrations = ctx.store.migrations()?;
    println!("migrations: {}", migrations.len());
    for migration in &migrations {
        println!(
            "  {} -> {} at {} by quantforge {}",
            migration.from_version,
            migration.to_version,
            ms_to_rfc3339(migration.applied_at_ms),
            migration.quantforge_version
        );
    }

    let legacy = ctx.store.legacy_runs()?;
    println!("legacy_runs: {}", legacy.len());
    for run in &legacy {
        println!(
            "  {} {} {} {} status={} mode={} started={}",
            run.run_id,
            run.market.exchange,
            run.market.symbol,
            run.market.interval,
            run.status.as_str(),
            run.execution_mode
                .map_or("unrecorded", |mode| mode.as_str()),
            ms_to_rfc3339(run.started_at_ms)
        );
    }
    Ok(())
}
