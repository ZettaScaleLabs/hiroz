use hiroz::{Builder, Result, context::ZContext};
use hiroz_msgs::example_interfaces::{AddTwoIntsResponse, srv::AddTwoInts};

// --8<-- [start:full_example]
/// AddTwoInts server node that provides a service to add two integers
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `max_requests` - Optional maximum number of requests to handle. If None, handles requests indefinitely.
pub fn run_add_two_ints_server(ctx: ZContext, max_requests: Option<usize>) -> Result<()> {
    // --8<-- [start:node_setup]
    // Create a node named "add_two_ints_server"
    let node = ctx.create_node("add_two_ints_server").build()?;
    // --8<-- [end:node_setup]

    // --8<-- [start:service_setup]
    // Create a service that will handle requests
    let mut service = node.create_service::<AddTwoInts>("add_two_ints").build()?;

    println!("AddTwoInts service server started, waiting for requests...");
    // --8<-- [end:service_setup]

    let mut request_count = 0;

    // --8<-- [start:request_loop]
    loop {
        // Wait for a request
        let req = service.take_request()?;
        println!(
            "Incoming request\na: {} b: {}",
            req.message().a,
            req.message().b
        );

        // Compute the sum
        let sum = req.message().a + req.message().b;

        // Create the response
        let resp = AddTwoIntsResponse { sum };

        println!("Sending response: {}", resp.sum);

        // Send the response
        req.reply_blocking(&resp)?;

        request_count += 1;

        // Check if we've reached the max requests
        if let Some(max) = max_requests
            && request_count >= max
        {
            break;
        }
    }
    // --8<-- [end:request_loop]

    Ok(())
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

    // Run the server
    let max_requests = if args.count == 0 {
        None
    } else {
        Some(args.count)
    };
    run_add_two_ints_server(ctx, max_requests)
}

#[cfg(not(any(test, doctest)))]
#[derive(Debug, clap::Parser)]
#[command(
    name = "demo_nodes_add_two_ints_server",
    about = "ROS 2 demo add_two_ints server node - provides addition service"
)]
struct Args {
    /// Number of requests to handle before exiting (0 for unlimited)
    #[arg(short, long, default_value = "0")]
    count: usize,

    /// Zenoh session mode (peer, client, router)
    #[arg(short, long, default_value = "peer")]
    mode: String,

    /// Zenoh router endpoint to connect to (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}
