mod action;
mod bw;
mod context;
mod delay;
mod echo;
mod hz;
mod info;
mod list;
mod manifest;
mod param;
mod r#pub;
mod service;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "hu-meter",
    about = "Rate, bandwidth, echo, service, and param commands for ROS 2 via hiroz"
)]
struct Cli {
    /// Zenoh router (overridden by HU_ROUTER env var)
    #[arg(long, default_value = "tcp/127.0.0.1:7447", global = true)]
    router: String,

    /// ROS domain ID (overridden by HU_DOMAIN env var)
    #[arg(long, default_value = "0", global = true)]
    domain: usize,

    /// Output structured JSON
    #[arg(long, global = true)]
    json: bool,

    /// Enable Zenoh shared memory (SHM) for zero-copy transport
    #[arg(long, global = true)]
    shm: bool,

    /// Emit plugin manifest and exit
    #[arg(long, hide = true, global = true)]
    hu_manifest: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Measure publishing rate of a topic
    Hz(hz::HzArgs),
    /// Measure bandwidth of a topic
    Bw(bw::BwArgs),
    /// Measure message delay (header stamp vs receive time)
    Delay(delay::DelayArgs),
    /// Print received messages from a topic
    Echo(echo::EchoArgs),
    /// Publish messages to a topic
    Pub(r#pub::PubArgs),
    /// List topics, nodes, services, or actions
    List(list::ListArgs),
    /// Show info about a topic, node, service, or action
    Info(info::InfoArgs),
    /// Call a service
    Service(service::ServiceArgs),
    /// List, inspect, or send goals to action servers
    Action(action::ActionArgs),
    /// Get, set, list, dump, or load parameters
    Param(param::ParamArgs),
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

    // Env vars from `hu` take precedence
    let router = std::env::var("HU_ROUTER").unwrap_or(cli.router);
    let domain: usize = std::env::var("HU_DOMAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cli.domain);

    let ctx = context::connect(&router, domain, cli.shm).await?;

    match cli.command {
        Commands::Hz(args) => hz::run(&ctx, args, cli.json).await,
        Commands::Bw(args) => bw::run(&ctx, args, cli.json).await,
        Commands::Delay(args) => delay::run(&ctx, args, cli.json).await,
        Commands::Echo(args) => echo::run(&ctx, args, cli.json).await,
        Commands::Pub(args) => r#pub::run(&ctx, args, cli.json).await,
        Commands::List(args) => list::run(&ctx, args, cli.json).await,
        Commands::Info(args) => info::run(&ctx, args, cli.json).await,
        Commands::Service(args) => service::run(&ctx, args, cli.json).await,
        Commands::Action(args) => action::run(&ctx, args, cli.json).await,
        Commands::Param(args) => param::run(&ctx, args, cli.json).await,
    }
}
