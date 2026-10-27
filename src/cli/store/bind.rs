//! `quantforge store bind` — declare, once and for life, the environment
//! the store is bound to. The declaration is checked against the endpoint
//! this invocation reaches: mainnet is a production endpoint and nothing
//! else is. Behind the `--yes` confirmation gate.

use crate::cli::common::ConfirmArgs;
use crate::cli::context::{AppContext, display_url};
use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use quantforge::{Environment, StoreControl};

#[derive(Args, Debug)]
pub(crate) struct StoreBindArgs {
    #[arg(long, value_enum)]
    environment: CliEnvironment,
    #[command(flatten)]
    confirm: ConfirmArgs,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum CliEnvironment {
    #[value(name = "mainnet")]
    Mainnet,
    #[value(name = "testnet")]
    Testnet,
    #[value(name = "demo")]
    Demo,
}

impl From<CliEnvironment> for Environment {
    fn from(value: CliEnvironment) -> Self {
        match value {
            CliEnvironment::Mainnet => Environment::Mainnet,
            CliEnvironment::Testnet => Environment::Testnet,
            CliEnvironment::Demo => Environment::Demo,
        }
    }
}

pub(crate) fn handle_store_bind(ctx: &AppContext, args: StoreBindArgs) -> Result<()> {
    let environment: Environment = args.environment.into();
    let production = ctx.exchange.market_data.is_production();
    if production != environment.is_production() {
        let endpoint = if production { "production" } else { "test" };
        bail!(
            "{} is a {endpoint} endpoint, so this invocation cannot bind the store to \
             {environment}; pass the {environment} base URL and run it again",
            display_url(&ctx.base_url)
        );
    }
    println!("store_id: {}", ctx.identity.store_id);
    if let Some(bound) = ctx.identity.environment {
        if bound == environment {
            println!("environment: {environment}");
            println!("already bound; nothing changed");
            return Ok(());
        }
        bail!(
            "store {} is bound to {bound}; a store keeps its environment for life, so use \
             another --db for {environment}",
            ctx.identity.store_id
        );
    }
    println!("environment: {environment}");
    if !args.confirm.yes {
        println!(
            "No binding written. Re-run with --yes to bind the store to {environment} for life."
        );
        return Ok(());
    }
    ctx.store.bind_environment(environment)?;
    println!("bound: {environment}");
    Ok(())
}
