//! A backtest run on a QuantForge server: the bundle made from the
//! strategy flags for the server's runtime, uploaded, the job submitted
//! over the same candles a local run would read, its log followed as it
//! runs, and its result downloaded and printed as a local run prints its
//! own. Ctrl-C while the log is followed leaves the job running and says
//! how to pick it up again.

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use quantforge::net::client::{ClientError, LogEvent, RemoteClient, server_url};
use quantforge::protocol::ExactDecimal;
use quantforge::protocol::jobs::{
    BacktestSettings, DataSelection, JobId, JobState, JobSubmission, LogLevel,
};
use quantforge::protocol::stream::AuthToken;
use quantforge::{MarketId, TimestampMs, ms_to_rfc3339};
use rust_decimal::Decimal;
use url::Url;

use super::report;
use super::strategy::Draft;

/// Help section for the flags that reach a server.
pub(super) const REMOTE_HEADING: &str = "Remote";

/// `--remote-token`, beside every `--remote`.
#[derive(Args)]
pub(crate) struct TokenArgs {
    /// Token sent to the --remote server with every request, as
    /// `Authorization: Bearer`, only over https or to this machine; no server
    /// checks one yet. Prefer
    /// QF_REMOTE_TOKEN: command-line values can land in shell history and
    /// process listings
    #[arg(
        long,
        env = "QF_REMOTE_TOKEN",
        hide_env_values = true,
        help_heading = REMOTE_HEADING
    )]
    remote_token: Option<String>,
}

/// Shows only whether the token is set.
impl std::fmt::Debug for TokenArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenArgs")
            .field(
                "remote_token",
                &self.remote_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl TokenArgs {
    /// The token, if one was given; an empty one counts as none, and never
    /// falls back to the variable.
    pub(super) fn token(&self) -> Option<AuthToken> {
        self.remote_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(AuthToken::new)
    }
}

/// A server's URL as `--remote` names it.
pub(super) fn remote_url(text: &str) -> Result<Url> {
    server_url(text).map_err(|error| anyhow!("--remote: {error}"))
}

/// What a remote run needs, every flag parsed and the strategy read and
/// checked, before any server is asked anything.
#[derive(Debug)]
pub(super) struct RemoteRun {
    pub(super) url: Url,
    pub(super) token: Option<AuthToken>,
    pub(super) market: MarketId,
    pub(super) start: Option<TimestampMs>,
    pub(super) end: Option<TimestampMs>,
    pub(super) cash: Decimal,
    pub(super) fee_bps: Decimal,
    pub(super) draft: Draft,
    pub(super) label: String,
    pub(super) no_wait: bool,
}

/// Connects to the server, saying which one answered.
pub(super) async fn connect(url: Url, token: Option<AuthToken>) -> Result<RemoteClient> {
    let client = RemoteClient::connect(url, token).await.map_err(reach)?;
    println!(
        "remote: {} (QuantForge {}, protocol {})",
        client.url(),
        client.release(),
        client.version().path_segment()
    );
    Ok(client)
}

/// A client failure, with what to check where the server was not reached.
pub(super) fn reach(error: ClientError) -> anyhow::Error {
    match error {
        ClientError::Unreachable { .. } => {
            anyhow!("{error}; check --remote, and that `quantforge serve run` listens there")
        }
        other => anyhow!(other),
    }
}

/// Runs the backtest on the server, and prints its report.
pub(super) async fn run(args: RemoteRun) -> Result<()> {
    let client = connect(args.url, args.token).await?;
    let runtime = client.runtime().await.map_err(reach)?.ok_or_else(|| {
        anyhow!(
            "the server at {} runs no Python bundles: it was started without --job-python",
            client.url()
        )
    })?;
    let bundle = args.draft.bundle(&runtime.python.to_string())?;
    bundle
        .admit(&runtime)
        .with_context(|| format!("the server at {} cannot run this bundle", client.url()))?;
    let (start, end) = window(&client, &args.market, args.start, args.end).await?;
    let uploaded = client.upload(&bundle).await.map_err(reach)?;
    println!(
        "bundle: {} ({} bytes, {})",
        uploaded.receipt.code_hash,
        uploaded.receipt.size_bytes,
        if uploaded.stored {
            "stored"
        } else {
            "already held"
        }
    );
    let submission = JobSubmission {
        bundle: bundle.code_hash(),
        data: DataSelection {
            exchange: args.market.exchange.venue().as_str().to_string(),
            category: Some(args.market.exchange.category().as_str().to_string()),
            symbol: args.market.symbol.as_str().to_string(),
            interval: args.market.interval.as_str().to_string(),
            start_open_ms: start,
            end_open_ms: end,
        },
        backtest: BacktestSettings {
            initial_cash: ExactDecimal::new(args.cash),
            fee_bps: ExactDecimal::new(args.fee_bps),
            close_out_at_end: true,
        },
    };
    let status = client
        .submit(&submission)
        .await
        .map_err(|error| anyhow!("the server at {} refused the job: {error}", client.url()))?;
    println!("job_id: {}", status.job_id);
    if args.no_wait {
        return Ok(());
    }
    follow_and_report(&client, &status.job_id, &args.label).await
}

/// The range a local run over the same store would read: the market's
/// bars from `start` to `end`, each bound left out reaching the first or
/// the last bar the server stores. A window outside those is refused here;
/// one inside a gap between them reaches the server, whose job then fails
/// as a local run over no candles does.
async fn window(
    client: &RemoteClient,
    market: &MarketId,
    start: Option<TimestampMs>,
    end: Option<TimestampMs>,
) -> Result<(TimestampMs, TimestampMs)> {
    let named = format!(
        "{} {} {} {}",
        market.exchange.venue().as_str(),
        market.exchange.category().as_str(),
        market.symbol.as_str(),
        market.interval.as_str()
    );
    let index = client.markets().await.map_err(reach)?;
    let stored = index
        .markets
        .iter()
        .find(|entry| {
            entry.exchange == market.exchange.venue().as_str()
                && entry.category == market.exchange.category().as_str()
                && entry.symbol == market.symbol.as_str()
                && entry.interval == market.interval.as_str()
        })
        .ok_or_else(|| anyhow!("the server at {} stores no bars of {named}", client.url()))?;
    let (first, last) = (stored.first_open_ms, stored.last_open_ms);
    let from = start.map_or(first, |start| start.max(first));
    let to = end.map_or(last, |end| end.min(last));
    if from > to {
        bail!(
            "the server at {} stores {named} from {} to {}, outside the window from {} to {}",
            client.url(),
            ms_to_rfc3339(first),
            ms_to_rfc3339(last),
            start.map_or_else(|| ms_to_rfc3339(first), ms_to_rfc3339),
            end.map_or_else(|| ms_to_rfc3339(last), ms_to_rfc3339)
        );
    }
    println!("range: {} to {}", ms_to_rfc3339(from), ms_to_rfc3339(to));
    Ok((from, to))
}

/// Follows `job`'s log to its end, then prints its report under `label`.
/// Ctrl-C leaves the job running on the server.
pub(super) async fn follow_and_report(
    client: &RemoteClient,
    job: &JobId,
    label: &str,
) -> Result<()> {
    let mut shown = Shown::default();
    let followed = tokio::select! {
        followed = client.follow(job, 0, |event| shown.event(event)) => followed,
        interrupted = tokio::signal::ctrl_c() => {
            interrupted.context("failed to wait for Ctrl-C")?;
            println!(
                "detached: job {job} keeps running on the server; `quantforge backtest jobs \
                 status {job} --remote {} --follow` picks it up again",
                client.url()
            );
            return Ok(());
        }
    };
    followed.map_err(reach)?;
    report_ended(client, job, label).await
}

/// Prints where `job` stands now and, once it has succeeded, its report.
/// A job that failed or was cancelled is an error.
pub(super) async fn report_ended(client: &RemoteClient, job: &JobId, label: &str) -> Result<()> {
    let status = client.status(job).await.map_err(reach)?;
    let state = match &status.state {
        JobState::Queued => "queued",
        JobState::Running { .. } => "running",
        JobState::Succeeded { .. } => "succeeded",
        JobState::Failed { .. } => "failed",
        JobState::Cancelled { .. } => "cancelled",
        _ => "in a state this build does not know",
    };
    println!("state: {state}");
    match status.state {
        JobState::Succeeded { .. } => {
            let result = client.result(job).await.map_err(reach)?;
            report::print(label, &result.result.metrics, &result.trades);
            Ok(())
        }
        JobState::Failed { error, .. } => bail!("job {job} failed: {}", error.message),
        JobState::Cancelled { .. } => bail!("job {job} was cancelled"),
        _ => Ok(()),
    }
}

/// How much of a followed log has been printed: progress in tenths, so a
/// job of any length prints at most eleven progress lines.
#[derive(Default)]
struct Shown {
    tenths: Option<u64>,
}

impl Shown {
    fn event(&mut self, event: LogEvent) {
        match event {
            LogEvent::Line(line) => {
                let level = match line.level {
                    LogLevel::Info => String::new(),
                    LogLevel::Debug => "debug: ".to_string(),
                    LogLevel::Warning => "warning: ".to_string(),
                    LogLevel::Error => "error: ".to_string(),
                    _ => "unknown level: ".to_string(),
                };
                let cut = line
                    .dropped_bytes
                    .map(|bytes| format!(" [{bytes} more bytes left out]"))
                    .unwrap_or_default();
                println!("log: {level}{}{cut}", line.message);
            }
            LogEvent::Dropped(count) => {
                println!("log: [{count} earlier lines no longer held by the server]");
            }
            LogEvent::Progress(progress) => {
                let tenths = progress
                    .bars_done
                    .saturating_mul(10)
                    .checked_div(progress.bars_total)
                    .unwrap_or(10);
                if self.tenths.is_none_or(|shown| tenths > shown) {
                    self.tenths = Some(tenths);
                    println!(
                        "progress: {} of {} bars",
                        progress.bars_done, progress.bars_total
                    );
                }
            }
            _ => {}
        }
    }
}
