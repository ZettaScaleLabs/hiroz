//! HIROZ Router - Drop-in replacement for rmw_zenoh_cpp router
//!
//! This example provides a Zenoh router configured with ROS 2 compatible settings
//! that match the rmw_zenoh_cpp configuration.
//!
//! Usage:
//!   cargo run --example zenoh_router          # Default: tcp/[::]:7447
//!   cargo run --example zenoh_router -- --listen "tcp/[::]:7447"
//!   cargo run --example zenoh_router -- --connect "tcp/192.168.1.1:7447"
//!   cargo run --example zenoh_router -- --config /path/to/config.json5
//!
//! Enable logging with RUST_LOG:
//!   RUST_LOG=info cargo run --example zenoh_router
//!   RUST_LOG=debug cargo run --example zenoh_router

use clap::Parser;
use hiroz::config::RouterConfigBuilder;
use zenoh::Wait;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The configuration file. Currently, this file must be a valid JSON5 or YAML file.
    #[arg(short, long, value_name = "PATH")]
    config: Option<String>,

    /// Locators on which this router will listen for incoming sessions. Repeat this option to open several listeners.
    #[arg(short, long, value_name = "ENDPOINT")]
    listen: Vec<String>,

    /// A peer locator this router will try to connect to.
    /// Repeat this option to connect to several peers.
    #[arg(short = 'e', long, value_name = "ENDPOINT")]
    connect: Vec<String>,
}

fn main() {
    let args = Args::parse();

    // Initialize Zenoh logger (respects RUST_LOG env var, defaults to "error")
    zenoh::init_log_from_env_or("error");

    // Build router configuration
    let config = if let Some(config_path) = &args.config {
        zenoh::Config::from_file(config_path).expect("Failed to load config from file")
    } else {
        let mut builder = RouterConfigBuilder::new();

        // Add listen endpoints
        if args.listen.is_empty() {
            // Default: listen on tcp/[::]:7447
            builder = builder.with_listen_port(7447);
        } else {
            for endpoint in &args.listen {
                builder = builder.with_listen_endpoint(endpoint);
            }
        }

        builder
            .build_config()
            .expect("Failed to build router config")
    };

    // Apply connect endpoints if provided
    let config = if !args.connect.is_empty() {
        let mut config = config;
        let endpoints: Vec<_> = args
            .connect
            .iter()
            .map(|s| s.parse().expect("Invalid endpoint"))
            .collect();
        config
            .connect
            .endpoints
            .set(endpoints)
            .expect("Failed to set connect endpoints");
        config
    } else {
        config
    };

    // Extract listen endpoint for display
    let listen_info = if let Some(config_path) = &args.config {
        format!("Using config file: {}", config_path)
    } else if !args.listen.is_empty() {
        format!("Listening: {}", args.listen.join(", "))
    } else {
        "Listening: tcp/[::]:7447".to_string()
    };

    println!("HIROZ Router (rmw_zenoh_cpp compatible)");
    println!("{}", listen_info);
    if !args.connect.is_empty() {
        println!("Connecting to: {}", args.connect.join(", "));
    }
    println!("Run your hiroz examples in other terminals now!");
    println!("Example: cargo run --example demo_nodes_talker");
    println!("Press Ctrl-C to stop");
    println!();

    // Open Zenoh session in router mode
    let session = zenoh::open(config)
        .wait()
        .expect("Failed to open Zenoh session");

    // Print session info
    let zid = session.zid();
    println!("Router started with ZID: {}", zid);
    println!();

    // Create runtime for async operations
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    runtime.block_on(async {
        // Wait for Ctrl-C
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl-C");
    });

    println!();
    println!("Router shutting down...");

    // Session will be closed when dropped
    drop(session);
}
