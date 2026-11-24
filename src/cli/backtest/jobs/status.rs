//! `quantforge backtest jobs status` — where a job a remote run left
//! stands and, once it has succeeded, the report the run would have
//! printed; with `--follow`, its log followed to its end first.

use anyhow::{Result, anyhow};
use clap::Args;
use quantforge::protocol::jobs::JobId;

use super::super::remote::{self, REMOTE_HEADING, TokenArgs, remote_url};

#[derive(Args, Debug)]
pub(crate) struct StatusArgs {
    /// The job's ID, as `backtest --remote` printed it
    job_id: String,
    /// QuantForge server that holds the job, such as http://127.0.0.1:9600
    #[arg(long, help_heading = REMOTE_HEADING)]
    remote: String,
    #[command(flatten)]
    token: TokenArgs,
    /// Follow the job's log until the job ends, then report it; Ctrl-C
    /// leaves it running again
    #[arg(long, default_value_t = false, help_heading = REMOTE_HEADING)]
    follow: bool,
}

pub(super) async fn handle_status(args: &StatusArgs) -> Result<()> {
    let job = JobId::new(args.job_id.as_str()).map_err(|error| anyhow!("<JOB_ID>: {error}"))?;
    let url = remote_url(&args.remote)?;
    let client = remote::connect(url, args.token.token()).await?;
    let status = client.status(&job).await.map_err(remote::reach)?;
    // The server keeps no entry with a job: the report names the strategy
    // by its bundle's code hash.
    let label = status.submission.bundle.to_string();
    println!("job_id: {job}");
    if args.follow {
        remote::follow_and_report(&client, &job, &label).await
    } else {
        remote::report_ended(&client, &job, &label).await
    }
}
