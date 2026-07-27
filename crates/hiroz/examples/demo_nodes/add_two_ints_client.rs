use std::time::Duration;

use hiroz::{Builder, Result, context::ZContext};
use hiroz_msgs::example_interfaces::{AddTwoIntsRequest, srv::AddTwoInts};

// --8<-- [start:full_example]
/// AddTwoInts client node that calls the service to add two integers
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `a` - First number to add
/// * `b` - Second number to add
/// * `async_mode` - Whether to use async response waiting
pub fn run_add_two_ints_client(ctx: ZContext, a: i64, b: i64, async_mode: bool) -> Result<i64> {
    // --8<-- [start:node_setup]
    // Create a node named "add_two_ints_client"
    let node = ctx.create_node("add_two_ints_client").build()?;
    // --8<-- [end:node_setup]

    // --8<-- [start:client_setup]
    // Create a client for the service
    let client = node.create_client::<AddTwoInts>("add_two_ints").build()?;

    println!(
        "AddTwoInts service client started (mode: {})",
        if async_mode { "async" } else { "sync" }
    );
    // --8<-- [end:client_setup]

    // --8<-- [start:service_call]
    // Create the request
    let req = AddTwoIntsRequest { a, b };
    println!("Sending request: {} + {}", req.a, req.b);

    // Wait for the response
    let resp = if async_mode {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { client.call(&req).await })?
    } else {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { client.call_with_timeout(&req, Duration::from_secs(5)).await })?
    };

    println!("Received response: {}", resp.sum);
    // --8<-- [end:service_call]

    Ok(resp.sum)
}
// --8<-- [end:full_example]

// Only compile main when building as a binary (not when included as a module)
#[cfg(not(any(test, doctest)))]
fn main() -> Result<()> {
    use clap::Parser;
    use hiroz::context::ZContextBuilder;

    let args = Args::parse();

    // Initialize logging
    zenoh::init_log_from_env_or("error");

    // Create the hiroz context with optional configuration
    let mut builder = ZContextBuilder::default();
    if let Some(e) = args.endpoint {
        builder = builder.with_connect_endpoints([e]);
    } else {
        // Connect to local zenohd for testing
        builder = builder.with_connect_endpoints(["tcp/127.0.0.1:7447"]);
    }
    let ctx = builder.build()?;

    // Run the client
    let result = run_add_two_ints_client(ctx, args.a, args.b, args.async_mode)?;
    println!("Result: {}", result);

    Ok(())
}

#[cfg(not(any(test, doctest)))]
#[derive(Debug, clap::Parser)]
#[command(
    name = "demo_nodes_add_two_ints_client",
    about = "ROS 2 demo add_two_ints client node - calls addition service"
)]
struct Args {
    /// First number to add
    #[arg(short, long, default_value = "2")]
    a: i64,

    /// Second number to add
    #[arg(short, long, default_value = "3")]
    b: i64,

    /// Use async mode for response waiting
    #[arg(long)]
    async_mode: bool,

    /// Zenoh router endpoint to connect to (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}
