use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[cfg(feature = "cross-dds")]
use crate::dds::participant::DdsParticipant as _;
#[cfg(feature = "cross-dds")]
use crate::dds::{CyclorsParticipant, ZDdsBridge};
#[cfg(feature = "cross-distro")]
use crate::distro::Bridge;

#[derive(Parser)]
#[command(
    name = "hu-bridge",
    about = "Cross-distro and cross-DDS bridge for ROS 2"
)]
struct Cli {
    #[arg(long, default_value_t = 0, global = true)]
    domain: usize,

    #[arg(long, hide = true, global = true)]
    hu_manifest: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a bridge
    Start(StartArgs),
    /// Show bridge status (placeholder)
    Status,
}

#[derive(clap::Args)]
struct StartArgs {
    #[arg(long)]
    distro: Option<String>,

    #[cfg(feature = "cross-dds")]
    #[arg(long)]
    dds: bool,

    #[cfg(feature = "cross-distro")]
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    source_endpoint: String,

    #[cfg(feature = "cross-distro")]
    #[arg(long, default_value = "tcp/127.0.0.1:7448")]
    target_endpoint: String,

    #[cfg(feature = "cross-dds")]
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    dds_endpoint: String,

    #[cfg(feature = "cross-dds")]
    #[arg(long)]
    allow: Option<String>,

    #[cfg(feature = "cross-dds")]
    #[arg(long)]
    deny: Option<String>,
}

/// Run the bridge from a slice of argv (argv[0] is the program name).
/// Returns 0 on success.
pub fn run_argv(argv: &[String]) -> i32 {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("hu-bridge: failed to build tokio runtime: {e}");
            return 1;
        }
    };

    match rt.block_on(run_async(argv)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("hu-bridge: {e:#}");
            1
        }
    }
}

async fn run_async(argv: &[String]) -> Result<()> {
    let cli = Cli::parse_from(argv);

    if cli.hu_manifest {
        print_manifest();
        return Ok(());
    }

    let domain: usize = std::env::var("HU_DOMAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cli.domain);

    match cli.command {
        Commands::Status => {
            println!("hu-bridge: no running bridge tracked (stateless mode)");
            Ok(())
        }
        Commands::Start(args) => run_bridge(args, domain).await,
    }
}

async fn run_bridge(args: StartArgs, _domain: usize) -> Result<()> {
    let distro_enabled = args.distro.is_some();

    #[cfg(feature = "cross-dds")]
    let dds_enabled = args.dds;
    #[cfg(not(feature = "cross-dds"))]
    let dds_enabled = false;

    if !distro_enabled && !dds_enabled {
        bail!("Specify at least one mode: --distro <source>:<target> or --dds");
    }

    crate::limits::check_domain_pair_cap(if distro_enabled { 1 } else { 0 })?;

    #[cfg(feature = "cross-dds")]
    if dds_enabled {
        let rule_count = args.allow.is_some() as usize + args.deny.is_some() as usize;
        crate::limits::check_rule_cap(rule_count)?;
    }

    let mut tasks = tokio::task::JoinSet::new();

    #[cfg(feature = "cross-distro")]
    if let Some(ref pair) = args.distro {
        let (source, target) = parse_distro_pair(pair)?;
        tracing::info!(
            "Cross-distro bridge: {} ↔ {} (domain {})",
            source,
            target,
            _domain
        );
        let source_ep = args.source_endpoint.clone();
        let target_ep = args.target_endpoint.clone();
        let domain_id = _domain;
        tasks.spawn(async move {
            let mut b = Bridge::new(&source_ep, &target_ep, domain_id).await?;
            b.run().await
        });
    }

    #[cfg(feature = "cross-dds")]
    if args.dds {
        tracing::info!(
            "Cross-DDS bridge: domain {} endpoint {}",
            _domain,
            args.dds_endpoint
        );
        let endpoint = args.dds_endpoint.clone();
        let domain_id = _domain;
        let allow = args.allow.clone();
        let deny = args.deny.clone();
        tasks.spawn(async move {
            use hiroz::{Builder, context::ZContextBuilder};
            let ctx = ZContextBuilder::default()
                .with_connect_endpoints([endpoint.as_str()])
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let node = ctx
                .create_node("hu-bridge-dds")
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let participant = CyclorsParticipant::create(domain_id as u32)?;
            ZDdsBridge::new(node, participant)
                .allow_topics_regex(allow.as_deref())
                .deny_topics_regex(deny.as_deref())
                .run()
                .await
        });
    }

    while let Some(res) = tasks.join_next().await {
        res??;
    }

    Ok(())
}

fn parse_distro_pair(s: &str) -> Result<(&str, &str)> {
    let mut parts = s.splitn(2, ':');
    let source = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if source.is_empty() || target.is_empty() {
        bail!("--distro requires format <source>:<target>, e.g. humble:jazzy");
    }
    Ok((source, target))
}

fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "bridge",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Cross-distro and cross-DDS bridge for ROS 2",
        "commands": ["start", "status"],
    });
    println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
}
