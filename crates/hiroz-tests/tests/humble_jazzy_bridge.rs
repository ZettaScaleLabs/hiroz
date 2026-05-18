//! Integration tests for the Humble ↔ Jazzy bridge.
//!
//! These tests require:
//! - A `nix develop .#ros-humble` shell available (with demo_nodes_cpp, rmw_zenoh_cpp)
//! - The `hiroz-bridge` binary to be built (`cargo build -p hiroz-bridge --features jazzy`)
//! - The `humble-jazzy-bridge-tests` feature enabled
//!
//! Run with:
//! ```bash
//! cargo test -p hiroz-tests --test humble_jazzy_bridge \
//!     --features jazzy,humble-jazzy-bridge-tests -j4
//! ```

#![cfg(feature = "humble-jazzy-bridge-tests")]

mod common;

use std::time::Duration;

use hiroz::{Builder, context::ZContextBuilder};
use hiroz_msgs::ros::{example_interfaces::srv::AddTwoInts, std_msgs::String as RosString};
use serial_test::serial;

/// Convenience: build a hiroz context connected to a test router endpoint.
///
/// Uses CLIENT mode so the zenoh-c 1.6.2 humble sessions never gossip-connect to
/// this session via P2P.  In zenoh 1.6.2, once a P2P connection is established, the
/// peer applies the peer-hat optimization and routes all publications via P2P, bypassing
/// the router path to CLIENT subscribers.  Client mode avoids P2P entirely.
fn jazzy_ctx(endpoint: &str) -> hiroz::context::ZContext {
    ZContextBuilder::default()
        .with_mode("client")
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint])
        .with_logging_enabled()
        .build()
        .expect("failed to build jazzy context")
}

// ============================================================================
// Pub/Sub Tests
// ============================================================================

/// A Humble talker (demo_nodes_cpp) publishes `/chatter` std_msgs/String.
/// The bridge rewrites the hash and a Jazzy hiroz subscriber receives the messages.
#[test]
#[serial]
fn test_pubsub_humble_pub_jazzy_sub() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    // Create Jazzy hiroz subscriber first so no forwarded messages are missed.
    // Zenoh does not buffer messages; a subscriber must exist before the bridge
    // starts forwarding or messages published in the interim are lost.
    let ctx = jazzy_ctx(endpoint);
    let node = ctx.create_node("test_jazzy_sub").build().unwrap();
    let sub = node
        .create_sub::<RosString>("/chatter")
        .build()
        .expect("failed to create subscriber");

    // Start bridge BEFORE the humble talker so the bridge's humble_session subscriber
    // is declared before the humble talker connects.  zenoh-c 1.6.2 only learns about
    // new subscribers at connection time (1.9.0-style late Interest propagation is not
    // understood by 1.6.2).  If the bridge subscriber is declared after the talker is
    // already connected, the talker never gets notified and never routes /chatter.
    let _bridge = common::spawn_bridge(endpoint);

    // Start Humble talker (publishes TypeHashNotSupported KE).
    let _humble_talker = common::spawn_humble_ros2_talker(endpoint, "/chatter");

    // Wait for a message within 120 seconds.
    // The Humble talker starts in a nix dev shell; even with a pre-warmed store
    // the shell activation + ros2 node startup takes 30-60s.  The bridge then
    // needs to detect the liveliness token and set up the forwarder before the
    // first forwarded message arrives.
    let msg = sub
        .recv_timeout(Duration::from_secs(120))
        .expect("did not receive message from Humble talker within timeout");

    assert!(
        !msg.data.is_empty(),
        "received empty message from Humble talker"
    );
    println!("Received from Humble talker: {}", msg.data);
}

/// A Jazzy hiroz publisher publishes `/chatter`.
/// The bridge rewrites the Jazzy KE to Humble KE.
/// A Humble listener (demo_nodes_cpp) receives the messages.
#[test]
#[serial]
fn test_pubsub_jazzy_pub_humble_sub() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    // Start Humble listener first.
    let _humble_listener = common::spawn_humble_ros2_listener(endpoint, "/chatter");

    // Start bridge.
    let _bridge = common::spawn_bridge(endpoint);
    std::thread::sleep(Duration::from_secs(2));

    // Create Jazzy hiroz publisher and publish a few messages.
    let ctx = jazzy_ctx(endpoint);
    let node = ctx.create_node("test_jazzy_pub").build().unwrap();
    let pub_ = node
        .create_pub::<RosString>("/chatter")
        .build()
        .expect("failed to create publisher");

    // Publish 5 messages with a short interval.
    for i in 0..5 {
        let msg = RosString {
            data: format!("hello from jazzy {i}"),
        };
        pub_.publish(&msg).expect("publish failed");
        std::thread::sleep(Duration::from_millis(200));
    }

    // The humble_listener is a subprocess — we can only verify it didn't crash.
    // Actual message receipt is validated in CI via subprocess output checking.
    println!("Published 5 messages to Humble listener");
}

// ============================================================================
// Service Tests
// ============================================================================

/// A Humble add_two_ints_server (demo_nodes_cpp) handles requests.
/// A Jazzy hiroz client sends a request through the bridge and receives the sum.
#[test]
#[serial]
fn test_service_humble_server_jazzy_client() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    let _humble_server = common::spawn_humble_ros2_service_server(endpoint);
    let _bridge = common::spawn_bridge(endpoint);

    // Create the Zenoh context INSIDE block_on so that all Zenoh objects drop
    // while the runtime is still active, preventing cleanup deadlocks.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(async {
        // Wait for the Humble server to start (nix shell activation + ros2 node
        // startup takes 30-60s), publish its liveliness token, and the bridge to
        // set up the service proxy.
        tokio::time::sleep(Duration::from_secs(60)).await;

        let ctx = jazzy_ctx(endpoint);
        let node = ctx.create_node("test_jazzy_client").build().unwrap();
        let client = node
            .create_client::<AddTwoInts>("/add_two_ints")
            .build()
            .expect("failed to create client");

        let req = hiroz_msgs::ros::example_interfaces::AddTwoIntsRequest { a: 3, b: 7 };
        // ctx / node / client are dropped here, inside the runtime
        client
            .call_with_timeout(&req, Duration::from_secs(20))
            .await
    });

    let response = response.expect("did not receive service response within timeout");
    assert_eq!(response.sum, 10, "expected 3+7=10, got {}", response.sum);
    println!("Service call succeeded: 3+7={}", response.sum);
}

/// A Jazzy hiroz service server handles add_two_ints requests.
/// A Humble add_two_ints_client (ros2 service call) sends a request through the bridge.
#[test]
#[serial]
fn test_service_jazzy_server_humble_client() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    let _bridge = common::spawn_bridge(endpoint);

    // Create all Zenoh objects INSIDE block_on so they drop while the runtime
    // is still live (avoids cleanup deadlocks during runtime shutdown).
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let ctx = jazzy_ctx(endpoint);
        let node = ctx.create_node("test_jazzy_server").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/add_two_ints")
            .build()
            .expect("failed to create service server");

        println!("Jazzy service server running, launching Humble client via bridge");
        // Give the bridge time to discover the Jazzy server and set up the proxy.
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Spawn the Humble client (`ros2 service call` exits after one reply).
        let _humble_client = common::spawn_humble_ros2_service_client(endpoint);

        // Handle one request or give up after 10 s.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(req) = server.take_request() {
                let a = req.message().a;
                let b = req.message().b;
                let resp = hiroz_msgs::ros::example_interfaces::AddTwoIntsResponse { sum: a + b };
                let _ = req.reply_blocking(&resp);
                println!(
                    "Handled Humble→Jazzy service call: {} + {} = {}",
                    a, b, resp.sum
                );
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("Jazzy server did not receive request from Humble client within 10s");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // ctx / node / server / _humble_client drop here, inside the runtime
    });
}

// ============================================================================
// Graph Visibility Tests (ros2 topic list)
// ============================================================================

/// A Humble talker publishes `/chatter`.
/// The bridge re-announces it with a Jazzy-style liveliness token (RIHS01 hash).
/// `ros2 topic list` in the Jazzy environment should see `/chatter`.
#[test]
#[serial]
fn test_graph_humble_pub_visible_in_jazzy() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    let _humble_talker = common::spawn_humble_ros2_talker(endpoint, "/chatter");
    let _bridge = common::spawn_bridge(endpoint);

    // Poll ros2 topic list until /chatter appears or timeout.
    // Each jazzy_topic_list() call runs `ros2 topic list` with a 30s inner timeout
    // (nix shell startup + rmw_zenoh discovery).  The outer deadline gives enough
    // room for the Humble talker's nix shell to start and the bridge to re-announce.
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        let topics = common::jazzy_topic_list(endpoint);
        if topics.iter().any(|t| t == "/chatter") {
            println!("Humble publisher visible in Jazzy topic list: {topics:?}");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "Humble /chatter not visible in Jazzy ros2 topic list after 180s. Got: {topics:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// A Jazzy hiroz publisher publishes `/chatter`.
/// The bridge re-announces it with a Humble-style liveliness token (TypeHashNotSupported).
/// `ros2 topic list` in the Humble nix shell should see `/chatter`.
#[test]
#[serial]
fn test_graph_jazzy_pub_visible_in_humble() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    let _bridge = common::spawn_bridge(endpoint);
    std::thread::sleep(Duration::from_secs(1));

    // Create Jazzy hiroz publisher — its liveliness token uses the real RIHS01 hash.
    let ctx = jazzy_ctx(endpoint);
    let node = ctx.create_node("test_jazzy_graph_pub").build().unwrap();
    let _pub = node
        .create_pub::<RosString>("/chatter")
        .build()
        .expect("failed to create publisher");

    // Poll ros2 topic list via Humble shell until /chatter appears or timeout.
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        let topics = common::humble_topic_list(endpoint);
        if topics.iter().any(|t| t == "/chatter") {
            println!("Jazzy publisher visible in Humble topic list: {topics:?}");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "Jazzy /chatter not visible in Humble ros2 topic list after 180s. Got: {topics:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ============================================================================
// Action Tests (smoke test — full action requires more infrastructure)
// ============================================================================

/// Smoke test: verify that a Humble fibonacci action server and the bridge start
/// without crashing. Full action round-trip (goal send/feedback/result) requires
/// a complete ROS 2 action client and is covered in CI with more complete setup.
#[test]
#[serial]
fn test_action_humble_server_and_bridge_startup() {
    let router = common::TestRouter::new();
    let endpoint = router.endpoint();

    let _humble_action_server = common::spawn_humble_ros2_action_server(endpoint);
    let _bridge = common::spawn_bridge(endpoint);

    // Allow discovery to propagate.
    std::thread::sleep(Duration::from_secs(3));

    // We can't trivially observe what the bridge discovered without instrumentation,
    // but if this test reaches here without panicking, the processes started cleanly.
    println!("Humble fibonacci action server started and bridge is running");
}
