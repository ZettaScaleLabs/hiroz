//! Long-running multi-node testbed for the docker-compose interop harness.
//!
//! Runs every hiroz entity exercised by the RED/GREEN gates in `compose/gates/`
//! inside a single process and zenoh session:
//!
//! | Node                      | Behavior                                                        |
//! |---------------------------|-----------------------------------------------------------------|
//! | `talker`                  | publishes `std_msgs/String` "Hello World: N" on `chatter`       |
//! | `chatter_echo`            | republishes everything received on `ping` to `pong`             |
//! | `add_two_ints_server`     | serves `example_interfaces/srv/AddTwoInts` on `add_two_ints`    |
//! | `add_two_ints_client`     | polls `add_two_ints_ros2` until a ROS 2 server answers, then    |
//! |                           | publishes "client_ok:sum=5" on `add_two_ints_client_ok`         |
//! | `fibonacci_action_server` | serves the Fibonacci action on `fibonacci`                      |
//! | `param_node`              | declares parameters `count` (int) and `label` (string)          |
//!
//! Once all nodes are up it writes `--ready-file` (the container healthcheck)
//! and runs until ctrl+c.

use std::time::Duration;

use clap::Parser;
use hiroz::{
    Builder, Result,
    action::server::ExecutingGoal,
    context::{ZContext, ZContextBuilder},
    parameter::{ParameterDescriptor, ParameterType, ParameterValue},
};
// Distro-specific action interfaces:
// - Humble/Jazzy: action_tutorials_cpp uses action_tutorials_interfaces
// - Kilted: action_tutorials_cpp uses example_interfaces
#[cfg(not(feature = "kilted"))]
use hiroz_msgs::action_tutorials_interfaces::{
    FibonacciFeedback, FibonacciResult, action::Fibonacci,
};
#[cfg(feature = "kilted")]
use hiroz_msgs::example_interfaces::{FibonacciFeedback, FibonacciResult, action::Fibonacci};
use hiroz_msgs::{
    example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts},
    std_msgs::String as RosString,
};

#[derive(Debug, clap::Parser)]
#[command(
    name = "compose_testbed",
    about = "Multi-node testbed exercised by the docker-compose interop gates"
)]
struct Args {
    /// Zenoh router endpoint to connect to (e.g., tcp/router:7447)
    #[arg(short, long, default_value = "tcp/127.0.0.1:7447")]
    endpoint: String,

    /// File created once all nodes are up (used as the container healthcheck)
    #[arg(long, default_value = "/tmp/testbed_ready")]
    ready_file: String,
}

/// Publishes "Hello World: N" on `chatter` every 500 ms.
fn spawn_talker(ctx: &ZContext) -> Result<()> {
    let node = ctx.create_node("talker").build()?;
    let publisher = node.create_pub::<RosString>("chatter").build()?;

    tokio::spawn(async move {
        let _node = node;
        let mut count: u64 = 1;
        loop {
            let msg = RosString {
                data: format!("Hello World: {count}"),
            };
            if let Err(e) = publisher.async_publish(&msg).await {
                eprintln!("talker: publish failed: {e}");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            count += 1;
        }
    });

    Ok(())
}

/// Republishes everything received on `ping` to `pong`, closing the
/// ROS 2 -> hiroz -> ROS 2 round trip checked by the topics gate.
fn spawn_chatter_echo(ctx: &ZContext) -> Result<()> {
    let node = ctx.create_node("chatter_echo").build()?;
    let subscriber = node.create_sub::<RosString>("ping").build()?;
    let publisher = node.create_pub::<RosString>("pong").build()?;

    tokio::spawn(async move {
        let _node = node;
        loop {
            match subscriber.async_recv().await {
                Ok(msg) => {
                    println!("chatter_echo: ping -> pong: '{}'", msg.data);
                    if let Err(e) = publisher.async_publish(&msg).await {
                        eprintln!("chatter_echo: publish failed: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("chatter_echo: recv failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });

    Ok(())
}

/// Serves `example_interfaces/srv/AddTwoInts` on `add_two_ints`.
fn spawn_add_two_ints_server(ctx: &ZContext) -> Result<()> {
    let node = ctx.create_node("add_two_ints_server").build()?;
    let mut service = node.create_service::<AddTwoInts>("add_two_ints").build()?;

    tokio::spawn(async move {
        let _node = node;
        loop {
            match service.async_take_request().await {
                Ok(req) => {
                    let sum = req.message().a + req.message().b;
                    println!(
                        "add_two_ints_server: {} + {} = {sum}",
                        req.message().a,
                        req.message().b
                    );
                    if let Err(e) = req.reply(&AddTwoIntsResponse { sum }).await {
                        eprintln!("add_two_ints_server: reply failed: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("add_two_ints_server: take_request failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });

    Ok(())
}

/// Polls a ROS 2-hosted AddTwoInts server on `add_two_ints_ros2` until it
/// answers, then publishes "client_ok:sum=5" on `add_two_ints_client_ok` so
/// the services gate can observe the hiroz -> ROS 2 direction.
fn spawn_add_two_ints_client(ctx: &ZContext) -> Result<()> {
    let node = ctx.create_node("add_two_ints_client").build()?;
    let client = node
        .create_client::<AddTwoInts>("add_two_ints_ros2")
        .build()?;
    let ok_publisher = node
        .create_pub::<RosString>("add_two_ints_client_ok")
        .build()?;

    tokio::spawn(async move {
        let _node = node;
        let req = AddTwoIntsRequest { a: 2, b: 3 };
        loop {
            match client.call_with_timeout(&req, Duration::from_secs(2)).await {
                Ok(resp) if resp.sum == 5 => break,
                Ok(resp) => eprintln!("add_two_ints_client: unexpected sum {}", resp.sum),
                // Server not up yet; keep polling.
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        println!("add_two_ints_client: got sum=5 from ROS 2 server");

        let msg = RosString {
            data: "client_ok:sum=5".to_string(),
        };
        loop {
            if let Err(e) = ok_publisher.async_publish(&msg).await {
                eprintln!("add_two_ints_client: publish failed: {e}");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    Ok(())
}

/// Serves the Fibonacci action on `fibonacci`. The returned node and server
/// must be kept alive for the action server to keep running.
fn start_fibonacci_action_server(
    ctx: &ZContext,
) -> Result<(
    hiroz::node::ZNode,
    hiroz::action::server::ZActionServer<Fibonacci>,
)> {
    let node = ctx.create_node("fibonacci_action_server").build()?;
    let server = node
        .create_action_server::<Fibonacci>("fibonacci")
        .build()?
        .with_handler(|executing: ExecutingGoal<Fibonacci>| async move {
            let order = executing.goal.order;
            let mut sequence = vec![0, 1];

            println!("fibonacci_action_server: executing goal with order {order}");

            for i in 2..=order {
                if executing.is_cancel_requested() {
                    println!("fibonacci_action_server: goal canceled");
                    executing
                        .canceled(FibonacciResult { sequence })
                        .expect("Failed to report cancellation");
                    return;
                }

                let next = sequence[i as usize - 1] + sequence[i as usize - 2];
                sequence.push(next);

                // Distro-specific feedback field names
                #[cfg(feature = "kilted")]
                let feedback = FibonacciFeedback {
                    sequence: sequence.clone(),
                };
                #[cfg(not(feature = "kilted"))]
                let feedback = FibonacciFeedback {
                    partial_sequence: sequence.clone(),
                };
                executing
                    .publish_feedback(feedback)
                    .expect("Failed to publish feedback");

                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            println!("fibonacci_action_server: goal succeeded");
            executing
                .succeed(FibonacciResult { sequence })
                .expect("Failed to report success");
        });

    Ok((node, server))
}

/// Declares the parameters checked by the parameters gate and keeps the node
/// alive so `ros2 param list/get/set` can reach it.
fn start_param_node(ctx: &ZContext) -> Result<hiroz::node::ZNode> {
    let node = ctx.create_node("param_node").build()?;

    let desc = ParameterDescriptor::new("count", ParameterType::Integer);
    node.declare_parameter("count", ParameterValue::Integer(0), desc)?;

    let desc = ParameterDescriptor::new("label", ParameterType::String);
    node.declare_parameter("label", ParameterValue::String("hello".into()), desc)?;

    Ok(node)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    zenoh::init_log_from_env_or("error");

    // Connect to the router as a client with multicast scouting disabled so
    // discovery is deterministic inside the compose network.
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_connect_endpoints([args.endpoint.as_str()])
        .with_mode("client")
        .build()?;

    spawn_talker(&ctx)?;
    spawn_chatter_echo(&ctx)?;
    spawn_add_two_ints_server(&ctx)?;
    spawn_add_two_ints_client(&ctx)?;
    let _fibonacci = start_fibonacci_action_server(&ctx)?;
    let _param_node = start_param_node(&ctx)?;

    std::fs::write(&args.ready_file, "ok")?;
    println!(
        "compose_testbed: all nodes started, ready file written to {}",
        args.ready_file
    );

    tokio::signal::ctrl_c().await?;
    Ok(())
}
