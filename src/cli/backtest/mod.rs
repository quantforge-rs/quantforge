//! `quantforge backtest` — deterministic bar-by-bar simulation: the built-in
//! strategy over locally stored candles, or, with `--remote`, a Python
//! strategy bundle on a QuantForge server, whose report prints here as a
//! local run's does. `backtest jobs` picks up a job a remote run left.
//!
//! - `remote`: a run on a server, its log followed, its report printed.
//! - `strategy`: the strategy flags made into a bundle.
//! - `report`: the report both kinds of run print.
//! - `jobs`: the jobs a server holds.

mod jobs;
mod remote;
mod report;
mod strategy;

use std::path::PathBuf;

use crate::cli::common::{
    parse_market, parse_non_negative_decimal, parse_positive_decimal, strategy_config,
};
use crate::cli::context::AppContext;
use anyhow::{Context, Result};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::parser::ValueSource;
use clap::{ArgMatches, Args, Subcommand};
use quantforge::server::BacktestOutput;
use quantforge::{
    BacktestConfig, BacktestEngine, CandleQuery, CandleStore, ExchangeId, parse_rfc3339_to_ms,
};

use remote::{REMOTE_HEADING, RemoteRun, TokenArgs, remote_url};
use strategy::Draft;

pub(crate) use jobs::JobsCommand;

// A run's own flags are refused before `jobs` by `refuse_run_flags`: clap's
// `args_conflicts_with_subcommands` would refuse a global flag there too.
#[derive(Args, Debug)]
#[command(subcommand_negates_reqs = true)]
pub(crate) struct BacktestArgs {
    #[command(subcommand)]
    command: Option<BacktestCommand>,
    // `MarketArgs`' two flags, but read as options: `backtest jobs` names
    // no market, and clap waives `--symbol` there.
    #[arg(long, required = true)]
    symbol: Option<String>,
    #[arg(long, default_value = "1m")]
    interval: String,
    #[arg(long)]
    start: Option<String>,
    #[arg(long)]
    end: Option<String>,
    #[arg(long, default_value_t = 20, conflicts_with = "remote")]
    fast: usize,
    #[arg(long, default_value_t = 50, conflicts_with = "remote")]
    slow: usize,
    #[arg(long, default_value = "10000")]
    cash: String,
    #[arg(long, default_value = "10")]
    fee_bps: String,
    /// QuantForge server to run the backtest on, such as
    /// http://127.0.0.1:9600: it runs the --strategy bundle on the candles
    /// it stores, and the report prints here
    #[arg(long, requires = "strategy", help_heading = REMOTE_HEADING)]
    remote: Option<String>,
    #[command(flatten)]
    token: TokenArgs,
    /// Python strategy to run on --remote: a .py file, or a directory to
    /// bundle, whose own quantforge.toml is used as it is
    #[arg(long, requires = "remote", help_heading = REMOTE_HEADING)]
    strategy: Option<PathBuf>,
    /// The strategy's file and class, as in sma_cross.py:SmaCross, for a
    /// --strategy without a quantforge.toml
    #[arg(long, requires = "strategy", help_heading = REMOTE_HEADING)]
    entry: Option<String>,
    /// The strategy's parameters as TOML key = value pairs, as in
    /// fast=20,slow=50, for a --strategy without a quantforge.toml
    #[arg(long, requires = "entry", help_heading = REMOTE_HEADING)]
    params: Option<String>,
    /// Print the job's ID once the server has queued it, and leave; `backtest
    /// jobs status` reports it later
    #[arg(long, default_value_t = false, requires = "remote", help_heading = REMOTE_HEADING)]
    no_wait: bool,
}

#[derive(Subcommand, Debug)]
enum BacktestCommand {
    /// The jobs a QuantForge server holds: their state, their log and their report.
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
}

impl BacktestArgs {
    /// `--symbol`, which clap requires of a run.
    fn symbol(&self) -> String {
        self.symbol.clone().unwrap_or_default()
    }

    /// Whether the command runs on a server and opens no local store.
    pub(crate) fn is_remote(&self) -> bool {
        self.remote.is_some() || self.command.is_some()
    }
}

/// The refusal of a run's own flags given before `backtest jobs`, which
/// takes none of them, worded as clap words a conflict. A global flag,
/// which every command takes, may stand there. `root` is the command that
/// made `matches`.
pub(crate) fn refuse_run_flags(
    root: &mut clap::Command,
    matches: &ArgMatches,
) -> Option<clap::Error> {
    let ("backtest", backtest) = matches.subcommand()? else {
        return None;
    };
    let (jobs, _) = backtest.subcommand()?;
    root.build();
    let command = root.find_subcommand_mut("backtest")?;
    let mut given: Vec<String> = command
        .get_arguments()
        .filter(|arg| !arg.is_global_set())
        .filter(|arg| {
            backtest.value_source(arg.get_id().as_str()) == Some(ValueSource::CommandLine)
        })
        .map(ToString::to_string)
        .collect();
    let prior = match given.len() {
        0 => return None,
        1 => ContextValue::String(given.remove(0)),
        _ => ContextValue::Strings(given),
    };
    let usage = command.render_usage();
    let mut error = clap::Error::new(ErrorKind::ArgumentConflict).with_cmd(command);
    error.insert(
        ContextKind::InvalidSubcommand,
        ContextValue::String(jobs.to_string()),
    );
    error.insert(ContextKind::PriorArg, prior);
    error.insert(ContextKind::Usage, ContextValue::StyledStr(usage));
    Some(error)
}

/// `backtest --remote` and `backtest jobs`, which reach a server and read
/// no local store: every flag is parsed and the strategy read before any
/// request.
pub(crate) async fn handle_remote(exchange: ExchangeId, args: &BacktestArgs) -> Result<()> {
    if let Some(BacktestCommand::Jobs { command }) = &args.command {
        return jobs::dispatch(command).await;
    }
    let url = remote_url(args.remote.as_deref().unwrap_or_default())?;
    let token = args.token.token();
    let market = parse_market(exchange, args.symbol(), args.interval.clone())?;
    let start = parse_time("--start", args.start.as_deref())?;
    let end = parse_time("--end", args.end.as_deref())?;
    let cash = parse_positive_decimal("--cash", &args.cash)?;
    let fee_bps = parse_non_negative_decimal("--fee-bps", &args.fee_bps)?;
    let path = args.strategy.clone().unwrap_or_default();
    let draft = Draft::read(&path, args.entry.as_deref(), args.params.as_deref())?;
    remote::run(RemoteRun {
        url,
        token,
        market,
        start,
        end,
        cash,
        fee_bps,
        draft,
        no_wait: args.no_wait,
    })
    .await
}

fn parse_time(flag: &str, raw: Option<&str>) -> Result<Option<i64>> {
    raw.map(parse_rfc3339_to_ms)
        .transpose()
        .with_context(|| format!("failed to parse {flag}"))
}

pub(crate) fn handle_backtest(ctx: &AppContext, args: BacktestArgs) -> Result<()> {
    let store = &ctx.store;
    let market = parse_market(ctx.exchange_id(), args.symbol(), args.interval.clone())?;
    let initial_cash = parse_positive_decimal("--cash", &args.cash)?;
    let fee_bps = parse_non_negative_decimal("--fee-bps", &args.fee_bps)?;
    let start_time_ms = parse_time("--start", args.start.as_deref())?;
    let end_time_ms = parse_time("--end", args.end.as_deref())?;
    let candles = store.load_candles(
        &market,
        CandleQuery {
            start_time_ms,
            end_time_ms,
            limit: None,
        },
    )?;

    let mut strategy = strategy_config(args.fast, args.slow)
        .build()
        .context("failed to build strategy")?;
    let engine = BacktestEngine::new(BacktestConfig {
        initial_cash,
        fee_bps,
        close_out_at_end: true,
    });

    let result = engine.run(&market, &candles, strategy.as_mut())?;
    let output = BacktestOutput::of(&result);
    report::print(strategy.name(), &output.metrics, &output.trades);
    Ok(())
}
