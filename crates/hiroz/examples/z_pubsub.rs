use std::time::Duration;

use clap::Parser;
use hiroz::{
    Builder, Result,
    context::{ZContext, ZContextBuilder},
};
use hiroz_msgs::std_msgs::String as RosString;

/// Subscriber function that continuously receives messages from a topic
async fn run_subscriber(ctx: ZContext, topic: String) -> Result<()> {
    // Create a ROS 2 node - the fundamental unit of computation
    // Nodes are logical groupings of publishers, subscribers, services, etc.
    let node = ctx.create_node("Sub").build()?;

    // Create a subscriber for the specified topic
    // The type parameter RosString determines what message type we'll receive
    let zsub = node.create_sub::<RosString>(&topic).build()?;

    // Continuously receive messages asynchronously
    // This loop will block waiting for messages on the topic
    while let Ok(msg) = zsub.async_recv().await {
        println!("Hearing:>> {}", msg.data);
    }
    Ok(())
}

/// Publisher function that continuously publishes messages to a topic
async fn run_publisher(
    ctx: ZContext,
    topic: String,
    period: Duration,
    payload: String,
) -> Result<()> {
    // Create a ROS 2 node for publishing
    let node = ctx.create_node("Pub").build()?;

    // Create a publisher for the specified topic
    // The type parameter RosString determines what message type we'll send
    let zpub = node.create_pub::<RosString>(&topic).build()?;

    let mut count = 0;
    loop {
        // Create a new message with incrementing counter
        let str = RosString {
            data: format!("{payload} - #{count}"),
        };
        println!("Telling:>> {}", str.data);

        // Publish the message asynchronously to all subscribers on this topic
        zpub.async_publish(&str).await?;

        // Wait for the specified period before publishing again
        let _ = tokio::time::sleep(period).await;
        count += 1;
    }
}

// The #[tokio::main] attribute sets up the async runtime
// hiroz requires an async runtime (Tokio is the most common choice)
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let format = hiroz_protocol::KeyExprFormat::RmwZenoh;

    // Create a ZContext - the entry point for hiroz applications
    // ZContext manages the connection to the Zenoh network and coordinates
    // communication between nodes. It can be configured with different modes:
    // - "peer" mode: nodes discover each other via multicast scouting
    // - "client" mode: nodes connect to a Zenoh router
    let ctx = if let Some(e) = args.endpoint {
        ZContextBuilder::default()
            .with_mode(args.mode)
            .with_connect_endpoints([e])
            .keyexpr_format(format)
            .build()?
    } else {
        ZContextBuilder::default()
            .with_mode(args.mode)
            .keyexpr_format(format)
            .build()?
    };

    let period = std::time::Duration::from_secs_f64(args.period);
    zenoh::init_log_from_env_or("error");

    // Run as either a publisher (talker) or subscriber (listener)
    // Both share the same ZContext but perform different roles
    match args.role.as_str() {
        "listener" => run_subscriber(ctx, args.topic).await?,
        "talker" => run_publisher(ctx, args.topic, period, args.data).await?,
        role => println!(
            "Please use \"talker\" or \"listener\" as role, {} is not supported.",
            role
        ),
    }
    Ok(())
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "Hello hiroz")]
    data: String,
    #[arg(short, long, default_value = "/chatter")]
    topic: String,
    #[arg(short, long, default_value = "1.0")]
    period: f64,
    #[arg(short, long, default_value = "listener")]
    role: String,
    #[arg(short, long, default_value = "peer")]
    mode: String,
    #[arg(short, long)]
    endpoint: Option<String>,
}
