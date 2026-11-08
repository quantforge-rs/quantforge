use anyhow::{Context, Result};

mod cli;

fn main() -> Result<()> {
    let cli = cli::parse();
    // A job's worker starts no runtime: the limits and sandbox it puts on
    // itself cover only the threads it starts afterwards.
    if cli.is_worker() {
        return cli::worker::run();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start the async runtime")?
        .block_on(cli::run(cli))
}
