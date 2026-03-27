use std::sync::Arc;

use clap::Parser;
use pty_mcp::{AppState, Config, PtyMcpServer};
use rmcp::{ServiceExt, transport::stdio};

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| "pty_mcp=info,rmcp=info".into());

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .try_init();
}

#[derive(Debug, Parser)]
#[command(
    name = "pty-mcp",
    version,
    about = "Starts the PTY MCP server over stdio.",
    long_about = None
)]
struct Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = Cli::parse();

    init_tracing();

    let config = Config::from_env()?;
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app.clone());
    let service = server.serve(stdio()).await?;

    service.waiting().await?;
    app.shutdown().await?;
    Ok(())
}
