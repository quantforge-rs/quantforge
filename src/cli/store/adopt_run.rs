//! `quantforge store adopt-run` — give a run that an earlier release wrote
//! without a verified identity the store's environment and the account
//! behind this invocation's credentials, so it can be resumed or closed
//! again. A run QuantForge 0.2.0 wrote recorded no execution mode either;
//! the operator states it. Behind the `--yes` confirmation gate.

use crate::cli::common::{CliExecutionMode, ConfirmArgs};
use crate::cli::context::AppContext;
use anyhow::{Result, anyhow, bail};
use clap::Args;
use quantforge::{ExecutionMode, RunIdentity, StoreControl};

#[derive(Args, Debug)]
pub(crate) struct StoreAdoptRunArgs {
    #[arg(long)]
    run_id: String,
    #[arg(long, value_enum)]
    execution_mode: Option<CliExecutionMode>,
    #[command(flatten)]
    confirm: ConfirmArgs,
}

pub(crate) fn handle_store_adopt_run(ctx: &AppContext, args: StoreAdoptRunArgs) -> Result<()> {
    let Some(environment) = ctx.identity.environment else {
        bail!(
            "store {} is not bound to an environment; bind it first with `quantforge store \
             bind --environment <mainnet|testnet|demo> --yes`",
            ctx.identity.store_id
        );
    };
    let legacy = ctx.store.legacy_runs()?;
    let run = legacy
        .iter()
        .find(|run| run.run_id == args.run_id)
        .ok_or_else(|| {
            anyhow!(
                "run {} is not a legacy run of this store; `quantforge store status` lists them",
                args.run_id
            )
        })?;
    let mode = match (run.execution_mode, args.execution_mode) {
        (Some(recorded), None) => recorded,
        (Some(recorded), Some(_)) => bail!(
            "run {} already records {} mode; drop --execution-mode",
            run.run_id,
            recorded.as_str()
        ),
        (None, Some(stated)) => stated.into(),
        (None, None) => bail!(
            "run {} was recorded by QuantForge 0.2.0 without its execution mode; state it with \
             --execution-mode dry-run or --execution-mode live",
            run.run_id
        ),
    };
    // A live run is adopted by the account that will close or resume it,
    // so the credentials must be in hand and must reach the environment
    // the store is bound to.
    let account = match mode {
        ExecutionMode::Live => {
            ctx.verify_environment()?;
            ctx.require_signed("store adopt-run requires exchange credentials for a live run")?;
            ctx.run_identity(mode).account
        }
        ExecutionMode::DryRun => None,
    };

    println!("run_id: {}", run.run_id);
    println!(
        "market: {} {} {}",
        run.market.exchange, run.market.symbol, run.market.interval
    );
    println!("execution_mode: {}", mode.as_str());
    println!("environment: {environment}");
    println!(
        "account: {}",
        if account.is_some() { "present" } else { "none" }
    );
    if !args.confirm.yes {
        println!("No identity written. Re-run with --yes to adopt the run.");
        return Ok(());
    }
    ctx.store.adopt_run(
        &run.run_id,
        &RunIdentity::adopted(Some(environment), account),
        run.execution_mode.is_none().then_some(mode),
    )?;
    println!("adopted: {}", run.run_id);
    Ok(())
}
