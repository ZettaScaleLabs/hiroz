//! Service client/server example using ros2dds backend
//!
//! This example demonstrates hiroz service communication using the ros2dds backend,
//! which is compatible with zenoh-bridge-ros2dds for interoperability
//! with standard ROS 2 service nodes.
//!
//! # Usage
//!
//! ## Direct hiroz to hiroz service communication
//!
//! ```bash
//! # Terminal 1: Run server
//! cargo run --example z_srvcli_ros2dds -- --role server
//!
//! # Terminal 2: Run client
//! cargo run --example z_srvcli_ros2dds -- --role client --a 5 --b 3
//! ```
//!
//! ## Interop with ROS 2 via zenoh-bridge-ros2dds
//!
//! ```bash
//! # Terminal 1: Run zenoh-bridge-ros2dds
//! zenoh-bridge-ros2dds
//!
//! # Terminal 2: Run ROS 2 service server
//! ros2 run demo_nodes_cpp add_two_ints_server
//!
//! # Terminal 3: Call service using hiroz client (ros2dds backend)
//! cargo run --example z_srvcli_ros2dds -- --role client --a 5 --b 3
//!
//! # Or vice versa - run hiroz server and call from ROS 2:
//! # Terminal 2: Run hiroz service server
//! cargo run --example z_srvcli_ros2dds -- --role server
//!
//! # Terminal 3: Call from ROS 2
//! ros2 service call /add_two_ints example_interfaces/srv/AddTwoInts "{a: 5, b: 3}"
//! ```

use std::time::Duration;

use clap::Parser;
use hiroz::{Builder, context::ZContextBuilder};
use hiroz_msgs::example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts};

#[derive(Debug, Parser)]
#[command(
    name = "z_srvcli_ros2dds",
    about = "ROS 2 service demo using ros2dds backend"
)]
struct Args {
    /// Role: 'server' or 'client'
    #[arg(short, long, default_value = "server")]
    role: String,

    /// First integer (for client)
    #[arg(short, long, default_value = "5")]
    a: i64,

    /// Second integer (for client)
    #[arg(short, long, default_value = "3")]
    b: i64,

    /// Service name
    #[arg(short, long, default_value = "add_two_ints")]
    service: String,

    /// Zenoh router endpoint to connect to
    #[arg(short, long, default_value = "tcp/127.0.0.1:7447")]
    endpoint: String,

    /// Number of requests to handle/send (0 for unlimited)
    #[arg(short, long, default_value = "1")]
    count: usize,
}

#[tokio::main]
async fn main() -> hiroz::Result<()> {
    zenoh::init_log_from_env_or("info");
    let args = Args::parse();

    println!("=== ros2dds Backend Service Demo ===");
    println!("Backend:        ros2dds");
    println!("Service:        {}", args.service);
    println!("Role:           {}", args.role);
    println!();

    // Create context pointing to zenoh-bridge-ros2dds
    let ctx = ZContextBuilder::default()
        .with_connect_endpoints([args.endpoint.as_str()])
        .keyexpr_format(hiroz_protocol::KeyExprFormat::Ros2Dds)
        .build()?;

    match args.role.as_str() {
        "server" => run_server(ctx, &args).await,
        "client" => run_client(ctx, &args).await,
        _ => {
            println!("Unknown role: {}. Use 'server' or 'client'.", args.role);
            Ok(())
        }
    }
}

async fn run_server(ctx: hiroz::context::ZContext, args: &Args) -> hiroz::Result<()> {
    println!("Starting AddTwoInts service server with ros2dds backend...\n");

    // Create node
    let node = ctx.create_node("add_two_ints_server").build()?;

    // Create service server with ros2dds backend
    let mut service = node.create_service::<AddTwoInts>(&args.service).build()?;

    println!("Service server ready, waiting for requests...\n");

    let max_requests = if args.count == 0 {
        None
    } else {
        Some(args.count)
    };
    let mut request_count = 0;

    loop {
        // Wait for a request
        let req = service.take_request()?;
        println!("Incoming request:");
        println!("  a: {}", req.message().a);
        println!("  b: {}", req.message().b);

        // Compute the sum
        let sum = req.message().a + req.message().b;

        // Create the response
        let resp = AddTwoIntsResponse { sum };

        println!("Sending response: {}\n", resp.sum);

        // Send the response
        req.reply_blocking(&resp)?;

        request_count += 1;

        // Check if we've reached the max requests
        if let Some(max) = max_requests
            && request_count >= max
        {
            println!("Handled {} requests, exiting.", request_count);
            break;
        }
    }

    Ok(())
}

async fn run_client(ctx: hiroz::context::ZContext, args: &Args) -> hiroz::Result<()> {
    println!("Starting AddTwoInts service client with ros2dds backend...\n");

    // Create node
    let node = ctx.create_node("add_two_ints_client").build()?;

    // Create service client with ros2dds backend
    let client = node.create_client::<AddTwoInts>(&args.service).build()?;

    println!("Service client ready\n");

    let max_requests = if args.count == 0 {
        usize::MAX
    } else {
        args.count
    };

    for i in 0..max_requests {
        // Create request
        let req = AddTwoIntsRequest {
            a: args.a + i as i64,
            b: args.b,
        };

        println!("Sending request #{}: a={}, b={}", i + 1, req.a, req.b);

        match client.call_with_timeout(&req, Duration::from_secs(5)).await {
            Ok(resp) => {
                println!("Received response: {} + {} = {}\n", req.a, req.b, resp.sum);
            }
            Err(e) => {
                println!("Failed to receive response: {}\n", e);
            }
        }

        if i + 1 < max_requests {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    println!("Client finished sending {} requests.", max_requests);

    Ok(())
}
