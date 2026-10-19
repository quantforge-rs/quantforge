//! Command-line interface: the root parser and its `parse()` entry point,
//! the command tree, and the `run()` dispatch. Each command group owns a
//! module mirroring its command path (`backtest`, `data`, `monitor`, `trade`).

use anyhow::Result;
use clap::parser::ValueSource;
use clap::{ArgMatches, Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::fmt;
use std::path::PathBuf;

mod backtest;
mod common;
mod context;
mod data;
mod monitor;
mod trade;

use context::AppContext;

// Args types are only named by the Command enum below; the handlers are
// dispatched by run().
use backtest::{BacktestArgs, handle_backtest};
use data::DataCommand;
use monitor::MonitorCommand;
use trade::TradeCommand;

/// Parse the process arguments exactly like clap's `Parser::parse()`, then
/// record whether a credential arrived on the command line rather than
/// through its env fallback — a fact the derived struct cannot carry.
pub(crate) fn parse() -> Cli {
    let matches = Cli::command().get_matches();
    let mut cli = Cli::from_arg_matches(&matches)
        .unwrap_or_else(|err| err.format(&mut Cli::command()).exit());
    cli.credentials.on_command_line = credentials_on_command_line(&matches);
    cli
}

/// Global propagation leaves the highest-precedence source on the root
/// matches, so a flag counts on either side of the subcommand.
fn credentials_on_command_line(matches: &ArgMatches) -> bool {
    ["binance_api_key", "binance_api_secret"]
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
            let private_client =
                ctx.require_private_client("monitor commands require Binance credentials")?;
            monitor::dispatch(&ctx.store, private_client, command).await?
        }
    }

    Ok(())
}

/// Production Binance Spot endpoint — the default when neither
/// `--binance-base-url` nor `QF_BINANCE_BASE_URL` is set.
const DEFAULT_BINANCE_BASE_URL: &str = "https://api.binance.com/";

/// Help section for the credential flags, listed after every command's
/// regular options.
const CREDENTIALS_HEADING: &str = "Binance credentials";

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

    /// Binance API base URL. Use https://testnet.binance.vision/ for Spot testnet.
    #[arg(
        long,
        global = true,
        env = "QF_BINANCE_BASE_URL",
        default_value = DEFAULT_BINANCE_BASE_URL
    )]
    pub(crate) binance_base_url: String,

    /// Log filter (trace, debug, info, warn, error or a full tracing filter expression).
    #[arg(long, global = true, env = "QF_LOG", default_value = "info")]
    pub(crate) log_level: String,

    // Flattened after the other globals so their display order, and with
    // it every command's existing --help lines, stays put.
    #[command(flatten)]
    pub(crate) credentials: CredentialArgs,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// `--binance-api-key` and `--binance-api-secret`: global flags whose env
/// fallbacks `--help` names without printing their values. A flag beats its
/// env var.
#[derive(Args)]
pub(crate) struct CredentialArgs {
    /// Binance API key for signed endpoints (monitor, trade close, trade run
    /// --mode live); prefer QF_BINANCE_API_KEY: command-line values can land
    /// in shell history and process listings
    #[arg(
        long,
        global = true,
        env = "QF_BINANCE_API_KEY",
        hide_env_values = true,
        help_heading = CREDENTIALS_HEADING
    )]
    pub(crate) binance_api_key: Option<String>,

    /// Binance API secret paired with --binance-api-key; prefer
    /// QF_BINANCE_API_SECRET: command-line values can land in shell history
    /// and process listings
    #[arg(
        long,
        global = true,
        env = "QF_BINANCE_API_SECRET",
        hide_env_values = true,
        help_heading = CREDENTIALS_HEADING
    )]
    pub(crate) binance_api_secret: Option<String>,

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
            .field("binance_api_key", &redacted(&self.binance_api_key))
            .field("binance_api_secret", &redacted(&self.binance_api_secret))
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

    /// Observe Binance state and manage orders manually when needed.
    Monitor {
        #[command(subcommand)]
        command: MonitorCommand,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // clap's own consistency self-check: duplicate flags or ids, broken
    // defaults, and malformed value parsers panic here rather than at the
    // first user invocation.
    #[test]
    fn cli_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    // A shell that exports the env fallbacks only turns the absent case's
    // source into EnvVariable, which still is not the command line.
    #[test]
    fn credential_flags_count_as_command_line_on_either_side_of_the_subcommand() {
        let cases: [(&[&str], bool); 3] = [
            (
                &[
                    "quantforge",
                    "--binance-api-key",
                    "k",
                    "data",
                    "validate",
                    "--symbol",
                    "BTCUSDT",
                ],
                true,
            ),
            (
                &[
                    "quantforge",
                    "data",
                    "validate",
                    "--symbol",
                    "BTCUSDT",
                    "--binance-api-secret",
                    "s",
                ],
                true,
            ),
            (
                &["quantforge", "data", "validate", "--symbol", "BTCUSDT"],
                false,
            ),
        ];
        for (argv, expected) in cases {
            let matches = Cli::command().try_get_matches_from(argv).expect("matches");
            assert_eq!(
                credentials_on_command_line(&matches),
                expected,
                "for {argv:?}"
            );
        }
    }

    #[test]
    fn credential_args_debug_never_prints_values() {
        let cli = Cli::try_parse_from([
            "quantforge",
            "--binance-api-key",
            "key-sentinel",
            "--binance-api-secret",
            "secret-sentinel",
            "data",
            "validate",
            "--symbol",
            "BTCUSDT",
        ])
        .expect("cli");

        let debug = format!("{cli:?}");
        assert!(debug.contains("<redacted>"), "{debug}");
        assert!(!debug.contains("key-sentinel"), "key leaked: {debug}");
        assert!(!debug.contains("secret-sentinel"), "secret leaked: {debug}");
    }
}
