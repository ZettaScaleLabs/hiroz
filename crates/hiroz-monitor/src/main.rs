mod context;
mod graph;
mod log;
mod log_level;
mod manifest;
mod watch;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "hu-monitor",
    about = "Graph watch, log, and log-level commands for ROS 2 via hiroz"
)]
struct Cli {
    #[arg(long, default_value = "tcp/127.0.0.1:7447", global = true)]
    router: String,

    #[arg(long, default_value = "0", global = true)]
    domain: usize,

    #[arg(long, global = true)]
    json: bool,

    #[arg(long, hide = true, global = true)]
    hu_manifest: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Stream graph change events (node/topic/service appear/disappear)
    Watch(watch::WatchArgs),
    /// Show current graph snapshot
    Graph(graph::GraphArgs),
    /// Subscribe to /rosout and display log messages
    Log(log::LogArgs),
    /// Get or set node log levels
    LogLevel(log_level::LogLevelArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.hu_manifest {
        manifest::print_manifest();
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let router = std::env::var("HU_ROUTER").unwrap_or(cli.router);
    let domain: usize = std::env::var("HU_DOMAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cli.domain);

    let ctx = context::connect(&router, domain).await?;

    match cli.command {
        Commands::Watch(args) => watch::run(&ctx, args, cli.json).await,
        Commands::Graph(args) => graph::run(&ctx, args, cli.json).await,
        Commands::Log(args) => log::run(&ctx, args, cli.json).await,
        Commands::LogLevel(args) => log_level::run(&ctx, args, cli.json).await,
    }
}
