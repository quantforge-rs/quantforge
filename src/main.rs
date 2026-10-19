use anyhow::Result;

mod cli;

#[tokio::main]
async fn main() -> Result<()> {
    cli::run(cli::parse()).await
}
