//! hiroz-bridge: bridges ROS 2 Humble and Jazzy Zenoh networks.
//!
//! Humble uses `TypeHashNotSupported` as type hash in key expressions.
//! Jazzy uses `RIHS01_<hex>` hashes. The bridge subscribes to both networks,
//! rewrites key expressions so they match, and forwards messages bidirectionally.

mod bridge;
mod discovery;
mod forwarder;
mod hash_registry;
mod ke;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "hiroz-bridge", about = "Bridge Humble ↔ Jazzy Zenoh networks")]
struct Args {
    /// Zenoh locator for the Humble network (e.g. tcp/127.0.0.1:7447)
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    humble_endpoint: String,

    /// Zenoh locator for the Jazzy network (e.g. tcp/127.0.0.1:7448)
    #[arg(long, default_value = "tcp/127.0.0.1:7448")]
    jazzy_endpoint: String,

    /// ROS domain ID
    #[arg(long, default_value_t = 0)]
    domain_id: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    tracing::info!(
        "Starting hiroz-bridge: humble={} jazzy={} domain={}",
        args.humble_endpoint,
        args.jazzy_endpoint,
        args.domain_id,
    );

    let mut b =
        bridge::Bridge::new(&args.humble_endpoint, &args.jazzy_endpoint, args.domain_id).await?;

    b.run().await
}
