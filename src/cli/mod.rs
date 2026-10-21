//! Command-line interface: the root parser and its `parse()` entry point,
//! the command tree, and the `run()` dispatch. Each command group owns a
//! module mirroring its command path (`backtest`, `data`, `monitor`, `serve`,
//! `trade`).

use anyhow::Result;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::error::ErrorKind;
use clap::parser::ValueSource;
use clap::{Arg, ArgMatches, Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use quantforge::{ExchangeId, exchange};
use std::fmt;
use std::path::PathBuf;

mod backtest;
mod common;
mod context;
mod data;
mod monitor;
mod serve;
mod trade;

use context::AppContext;

// Args types are only named by the Command enum below; the handlers are
// dispatched by run().
use backtest::{BacktestArgs, handle_backtest};
use data::DataCommand;
use monitor::MonitorCommand;
use serve::ServeCommand;
use trade::TradeCommand;

/// Parse the process arguments exactly like clap's `Parser::parse()`, then
/// refuse a renamed setting and record whether a credential arrived on the
/// command line rather than through its env fallback — facts the derived
/// struct cannot carry.
pub(crate) fn parse() -> Cli {
    let matches = command().get_matches();
    if let Some(message) = renamed_setting_error(&matches) {
        command().error(ErrorKind::UnknownArgument, message).exit();
    }
    let mut cli =
        Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.format(&mut command()).exit());
    cli.credentials.on_command_line = credentials_on_command_line(&matches);
    cli
}

/// The released name of `--base-url` and its variable. They are still
/// recognized, only to be refused, so that an exported testnet URL fails
/// loudly instead of quietly leaving the default production endpoint in
/// charge.
const LEGACY_BASE_URL_FLAG: &str = "binance-base-url";
const LEGACY_BASE_URL_ENV: &str = "QF_BINANCE_BASE_URL";
const LEGACY_BASE_URL_ID: &str = "legacy_base_url";

/// The root command as `parse()` runs it: the derived [`Cli`] plus the
/// hidden renamed settings, which are no part of the documented surface.
fn command() -> clap::Command {
    Cli::command().arg(
        Arg::new(LEGACY_BASE_URL_ID)
            .long(LEGACY_BASE_URL_FLAG)
            .env(LEGACY_BASE_URL_ENV)
            .value_name("URL")
            .global(true)
            .hide(true),
    )
}

/// The refusal for a renamed setting that is still set, naming its new
/// name. It never repeats the value: a URL can carry credentials.
fn renamed_setting_error(matches: &ArgMatches) -> Option<String> {
    let source = matches.value_source(LEGACY_BASE_URL_ID)?;
    Some(if source == ValueSource::CommandLine {
        format!("--{LEGACY_BASE_URL_FLAG} was renamed to --base-url")
    } else {
        format!("{LEGACY_BASE_URL_ENV} is no longer read; rename it to QF_BASE_URL")
    })
}

/// Global propagation leaves the highest-precedence source on the root
/// matches, so a flag counts on either side of the subcommand.
fn credentials_on_command_line(matches: &ArgMatches) -> bool {
    ["api_key", "api_secret"]
        .into_iter()
        .any(|id| matches.value_source(id) == Some(ValueSource::CommandLine))
}

/// Build the per-invocation context and dispatch the parsed command.
pub(crate) async fn run(cli: Cli) -> Result<()> {
    let ctx = AppContext::init(&cli)?;

    match cli.command {
        Command::Data { command } => data::dispatch(&ctx, command).await?,
        Command::Backtest(args) => handle_backtest(&ctx, args)?,
        Command::Trade { command } => trade::dispatch(&ctx, command).await?,
        Command::Monitor { command } => {
            let signed = ctx.require_signed("monitor commands require exchange credentials")?;
            monitor::dispatch(
                &ctx.store,
                ctx.exchange.market_data.as_ref(),
                signed,
                command,
            )
            .await?
        }
        Command::Serve { command } => serve::dispatch(command).await?,
    }

    Ok(())
}

/// `--exchange`'s default: the registry's name for its default exchange.
const DEFAULT_EXCHANGE_NAME: &str = exchange::name(exchange::DEFAULT_EXCHANGE);

/// `--exchange`'s values are the registry's names, so clap lists them in
/// `--help` and in the error for any other value.
fn exchange_parser() -> impl TypedValueParser<Value = ExchangeId> {
    PossibleValuesParser::new(exchange::registered().iter().map(|&id| exchange::name(id)))
        .try_map(|name| exchange::lookup(&name).ok_or_else(|| format!("no exchange named {name}")))
}

/// Help section for the credential flags, listed after every command's
/// regular options.
const CREDENTIALS_HEADING: &str = "Exchange credentials";

#[derive(Parser, Debug)]
#[command(
    name = "quantforge",
    version,
    about = "CLI-first market data ingestion, research, and controlled live trading framework."
)]
pub(crate) struct Cli {
    /// SQLite database path.
    #[arg(
        long,
        global = true,
        env = "QF_DB",
        default_value = "data/market.sqlite"
    )]
    pub(crate) db: PathBuf,

    /// Exchange to connect to.
    #[arg(
        long,
        global = true,
        env = "QF_EXCHANGE",
        default_value = DEFAULT_EXCHANGE_NAME,
        value_parser = exchange_parser()
    )]
    pub(crate) exchange: ExchangeId,

    /// Exchange API base URL; omitted, the exchange's production endpoint.
    #[arg(long, global = true, env = "QF_BASE_URL")]
    pub(crate) base_url: Option<String>,

    /// Log filter (trace, debug, info, warn, error or a full tracing filter expression).
    #[arg(long, global = true, env = "QF_LOG", default_value = "info")]
    pub(crate) log_level: String,

    // Flattened after the other globals so their display order stays put.
    #[command(flatten)]
    pub(crate) credentials: CredentialArgs,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// `--api-key` and `--api-secret`: global flags whose env fallbacks
/// `--help` names without printing their values. A flag beats its env var.
#[derive(Args)]
pub(crate) struct CredentialArgs {
    /// API key for the exchange's signed endpoints (monitor, trade close,
    /// trade run --mode live); prefer QF_API_KEY: command-line values can
    /// land in shell history and process listings
    #[arg(
        long,
        global = true,
        env = "QF_API_KEY",
        hide_env_values = true,
        help_heading = CREDENTIALS_HEADING
    )]
    pub(crate) api_key: Option<String>,

    /// API secret paired with --api-key; prefer QF_API_SECRET: command-line
    /// values can land in shell history and process listings
    #[arg(
        long,
        global = true,
        env = "QF_API_SECRET",
        hide_env_values = true,
        help_heading = CREDENTIALS_HEADING
    )]
    pub(crate) api_secret: Option<String>,

    /// Set by [`parse`]: a credential came from the command line, not from
    /// its env var.
    #[arg(skip)]
    pub(crate) on_command_line: bool,
}

/// Shows only whether each credential is set, so a `{:?}` of the parsed
/// [`Cli`] never prints a key or secret.
impl fmt::Debug for CredentialArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("CredentialArgs")
            .field("api_key", &redacted(&self.api_key))
            .field("api_secret", &redacted(&self.api_secret))
            .field("on_command_line", &self.on_command_line)
            .finish()
    }
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Historical and incremental data ingestion into SQLite.
    Data {
        #[command(subcommand)]
        command: DataCommand,
    },

    /// Deterministic backtest using locally stored candles.
    Backtest(BacktestArgs),

    /// Run or manually close the strategy bot.
    Trade {
        #[command(subcommand)]
        command: TradeCommand,
    },

    /// Observe exchange state and manage orders manually when needed.
    Monitor {
        #[command(subcommand)]
        command: MonitorCommand,
    },

    /// Run the server other QuantForge instances connect to.
    Serve {
        #[command(subcommand)]
        command: ServeCommand,
    },
}

#[cfg(test)]
mod tests;
