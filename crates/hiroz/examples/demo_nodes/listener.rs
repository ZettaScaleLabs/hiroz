use std::{num::NonZeroUsize, time::Duration};

use hiroz::{
    Builder, Result,
    context::ZContext,
    qos::{QosHistory, QosProfile},
};
use hiroz_msgs::std_msgs::String as RosString;

// --8<-- [start:full_example]
/// Listener node that subscribes to a topic
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `topic` - The topic name to subscribe to
/// * `max_count` - Optional maximum number of messages to receive. If None, listens indefinitely.
/// * `timeout` - Optional timeout duration. If None, waits indefinitely.
///
/// # Returns
/// A vector of received messages
pub async fn run_listener(
    ctx: ZContext,
    topic: &str,
    max_count: Option<usize>,
    timeout: Option<Duration>,
) -> Result<Vec<String>> {
    // --8<-- [start:node_setup]
    // Create a node named "listener"
    let node = ctx.create_node("listener").build()?;
    // --8<-- [end:node_setup]

    // --8<-- [start:subscriber_setup]
    // Create a subscription to the "chatter" topic
    let qos = QosProfile {
        history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        ..Default::default()
    };
    let subscriber = node.create_sub::<RosString>(topic).with_qos(qos).build()?;
    // --8<-- [end:subscriber_setup]

    let mut received_messages = Vec::new();
    let start = std::time::Instant::now();

    // --8<-- [start:receive_loop]
    // Receive messages in a loop
    loop {
        // Check timeout
        if let Some(t) = timeout
            && start.elapsed() > t
        {
            break;
        }

        // Try to receive with a small timeout to allow checking other conditions
        let recv_result = if timeout.is_some() || max_count.is_some() {
            subscriber.recv_timeout(Duration::from_millis(100))
        } else {
            // If no limits, use async_recv
            subscriber.async_recv().await
        };

        match recv_result {
            Ok(msg) => {
                // Log the received message
                println!("I heard: [{}]", msg.data);
                received_messages.push(msg.data.clone());

                // Check if we've received enough messages
                if let Some(max) = max_count
                    && received_messages.len() >= max
                {
                    break;
                }
            }
            Err(_) => {
                // Continue if timeout on recv_timeout
                if timeout.is_some() || max_count.is_some() {
                    continue;
                } else {
                    break;
                }
            }
        }
    }
    // --8<-- [end:receive_loop]

    Ok(received_messages)
}
// --8<-- [end:full_example]

// Only compile main when building as a binary (not when included as a module)
#[cfg(not(any(test, doctest)))]
#[tokio::main]
async fn main() -> Result<()> {
    use clap::Parser;
    use hiroz::context::ZContextBuilder;

    let args = Args::parse();

    // Initialize logging
    zenoh::init_log_from_env_or("error");

    // Create the hiroz context with optional configuration
    let ctx = if let Some(e) = args.endpoint {
        ZContextBuilder::default()
            .with_mode(args.mode)
            .with_connect_endpoints([e])
            .build()?
    } else {
        ZContextBuilder::default().with_mode(args.mode).build()?
    };

    // Run the listener node indefinitely
    run_listener(ctx, &args.topic, None, None).await?;

    Ok(())
}

#[cfg(not(any(test, doctest)))]
#[derive(Debug, clap::Parser)]
#[command(
    name = "demo_nodes_listener",
    about = "ROS 2 demo listener node - subscribes to messages"
)]
struct Args {
    /// Topic name to subscribe to
    #[arg(short, long, default_value = "chatter")]
    topic: String,

    /// Zenoh session mode (peer, client, router)
    #[arg(short, long, default_value = "peer")]
    mode: String,

    /// Zenoh router endpoint to connect to (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}
