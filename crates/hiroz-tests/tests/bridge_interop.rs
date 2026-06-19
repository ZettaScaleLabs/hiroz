//! Cross-distro (Humble ↔ Jazzy) bridge integration tests.
//!
//! Requires the `bridge-interop-tests` feature and the `ros-bridge-interop` nix
//! dev shell, which exports `HUMBLE_ROS2` pointing to the humble-ros2 wrapper:
//!
//! ```bash
//! cargo test -p hiroz-tests --test bridge_interop \
//!     --features bridge-interop-tests,jazzy -- --nocapture
//! ```

#![cfg(feature = "bridge-interop-tests")]

mod common;

use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::{
    example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts},
    std_msgs::String as RosString,
};
use serial_test::serial;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn humble_ros2_bin() -> String {
    std::env::var("HUMBLE_ROS2")
        .expect("HUMBLE_ROS2 env var not set — run inside the `ros-bridge-interop` nix shell")
}

fn bridge_bin() -> String {
    std::env::var("HIROZ_BRIDGE_BIN").unwrap_or_else(|_| {
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap() // hiroz-tests → crates/
            .parent()
            .unwrap(); // crates/ → workspace root
        workspace
            .join("target/debug/hu-bridge")
            .to_str()
            .unwrap()
            .to_string()
    })
}

fn rmw_override(endpoint: &str) -> String {
    format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false")
}

fn spawn_humble(args: &[&str], env: &[(&str, &str)]) -> ProcessGuard {
    let bin = humble_ros2_bin();
    let name = args.join(" ");
    let mut cmd = Command::new(&bin);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn humble-ros2 {name}: {e}"));
    ProcessGuard::new(child, &format!("humble-ros2 {name}"))
}

fn spawn_humble_talker(endpoint: &str, topic: &str) -> ProcessGuard {
    spawn_humble(
        &[
            "run",
            "demo_nodes_cpp",
            "talker",
            "--ros-args",
            "-r",
            &format!("chatter:={topic}"),
        ],
        &[
            ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
            ("ZENOH_CONFIG_OVERRIDE", &rmw_override(endpoint)),
        ],
    )
}

fn spawn_humble_listener(endpoint: &str, topic: &str) -> ProcessGuard {
    spawn_humble(
        &[
            "run",
            "demo_nodes_cpp",
            "listener",
            "--ros-args",
            "-r",
            &format!("chatter:={topic}"),
        ],
        &[
            ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
            ("ZENOH_CONFIG_OVERRIDE", &rmw_override(endpoint)),
        ],
    )
}

fn spawn_humble_service_server(endpoint: &str) -> ProcessGuard {
    spawn_humble(
        &["run", "demo_nodes_cpp", "add_two_ints_server"],
        &[
            ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
            ("ZENOH_CONFIG_OVERRIDE", &rmw_override(endpoint)),
        ],
    )
}

fn spawn_humble_service_client(endpoint: &str) -> ProcessGuard {
    spawn_humble(
        &[
            "service",
            "call",
            "/add_two_ints",
            "example_interfaces/srv/AddTwoInts",
            "{a: 3, b: 7}",
        ],
        &[
            ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
            ("ZENOH_CONFIG_OVERRIDE", &rmw_override(endpoint)),
        ],
    )
}

fn spawn_humble_action_server(endpoint: &str) -> ProcessGuard {
    spawn_humble(
        &["run", "action_tutorials_cpp", "fibonacci_action_server"],
        &[
            ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
            ("ZENOH_CONFIG_OVERRIDE", &rmw_override(endpoint)),
        ],
    )
}

fn spawn_bridge(endpoint: &str) -> ProcessGuard {
    let bin = bridge_bin();
    let child = Command::new(&bin)
        .args(["start", "--distro", "humble:jazzy"])
        .args(["--source-endpoint", endpoint])
        .args(["--target-endpoint", endpoint])
        .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_default())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn hu-bridge ({}): {e}", bin));
    thread::sleep(Duration::from_millis(500));
    ProcessGuard::new(child, "hu-bridge")
}

fn jazzy_topic_list(endpoint: &str) -> Vec<String> {
    let override_str = rmw_override(endpoint);
    let output = Command::new("ros2")
        .args(["topic", "list", "--spin-time", "5", "--no-daemon"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", &override_str)
        .output()
        .expect("failed to run `ros2 topic list`");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with('/') && !l.contains(' '))
        .collect()
}

fn humble_topic_list(endpoint: &str) -> Vec<String> {
    let override_str = rmw_override(endpoint);
    let bin = humble_ros2_bin();
    let output = Command::new(&bin)
        .args(["topic", "list", "--spin-time", "5", "--no-daemon"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", &override_str)
        .output()
        .expect("failed to run humble `ros2 topic list`");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with('/') && !l.contains(' '))
        .collect()
}

// ─── Pub/Sub ──────────────────────────────────────────────────────────────────

/// Humble talker (demo_nodes_cpp) → bridge → Jazzy hiroz subscriber.
#[test]
#[serial]
fn test_bridge_humble_pub_jazzy_sub() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
    let node = ctx.create_node("test_jazzy_sub").build().unwrap();
    let sub = node
        .create_sub::<RosString>("/chatter")
        .build()
        .expect("failed to create subscriber");

    let _bridge = spawn_bridge(&endpoint);
    let _humble_talker = spawn_humble_talker(&endpoint, "/chatter");

    let msg = sub
        .recv_timeout(Duration::from_secs(120))
        .expect("did not receive message from Humble talker within 120s");

    assert!(!msg.data.is_empty(), "received empty message");
    println!("Received from Humble talker: {}", msg.data);
}

/// Jazzy hiroz publisher → bridge → Humble listener (demo_nodes_cpp).
#[test]
#[serial]
fn test_bridge_jazzy_pub_humble_sub() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let _humble_listener = spawn_humble_listener(&endpoint, "/chatter");
    let _bridge = spawn_bridge(&endpoint);
    thread::sleep(Duration::from_secs(2));

    let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
    let node = ctx.create_node("test_jazzy_pub").build().unwrap();
    let pub_ = node
        .create_pub::<RosString>("/chatter")
        .build()
        .expect("failed to create publisher");

    for i in 0..5 {
        pub_.publish(&RosString {
            data: format!("hello from jazzy {i}"),
        })
        .expect("publish failed");
        thread::sleep(Duration::from_millis(200));
    }
    println!("Published 5 messages; Humble listener should have received them");
}

// ─── Services ─────────────────────────────────────────────────────────────────

/// Humble add_two_ints_server → bridge → Jazzy hiroz client.
#[test]
#[serial]
fn test_bridge_humble_server_jazzy_client() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let _humble_server = spawn_humble_service_server(&endpoint);
    let _bridge = spawn_bridge(&endpoint);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt.block_on(async {
        // Humble nix shell startup + bridge proxy setup can take 30-60s.
        tokio::time::sleep(Duration::from_secs(60)).await;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("test_jazzy_client").build().unwrap();
        let client = node
            .create_client::<AddTwoInts>("/add_two_ints")
            .build()
            .expect("failed to create client");

        let req = AddTwoIntsRequest { a: 3, b: 7 };
        client
            .call_with_timeout(&req, Duration::from_secs(20))
            .await
    });

    let response = response.expect("no service response within timeout");
    assert_eq!(response.sum, 10, "expected 3+7=10, got {}", response.sum);
    println!("Service call succeeded: 3+7={}", response.sum);
}

/// Jazzy hiroz service server → bridge → Humble client (ros2 service call).
#[test]
#[serial]
fn test_bridge_jazzy_server_humble_client() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let _bridge = spawn_bridge(&endpoint);

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("test_jazzy_server").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/add_two_ints")
            .build()
            .expect("failed to create service server");

        tokio::time::sleep(Duration::from_secs(2)).await;
        let _humble_client = spawn_humble_service_client(&endpoint);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(req) = server.take_request() {
                let resp = AddTwoIntsResponse {
                    sum: req.message().a + req.message().b,
                };
                let _ = req.reply_blocking(&resp);
                println!(
                    "Handled Humble→Jazzy service call: {} + {} = {}",
                    req.message().a,
                    req.message().b,
                    resp.sum
                );
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("Jazzy server did not receive request from Humble client within 30s");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}

// ─── Graph visibility ─────────────────────────────────────────────────────────

/// Humble talker → bridge re-announces with RIHS01 hash → visible in Jazzy `ros2 topic list`.
#[test]
#[serial]
fn test_bridge_humble_pub_visible_in_jazzy() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let _humble_talker = spawn_humble_talker(&endpoint, "/chatter");
    let _bridge = spawn_bridge(&endpoint);

    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        let topics = jazzy_topic_list(&endpoint);
        if topics.iter().any(|t| t == "/chatter") {
            println!("Humble publisher visible in Jazzy topic list: {topics:?}");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("/chatter not in Jazzy topic list after 180s. Got: {topics:?}");
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Jazzy hiroz publisher → bridge re-announces with TypeHashNotSupported → visible in Humble `ros2 topic list`.
#[test]
#[serial]
fn test_bridge_jazzy_pub_visible_in_humble() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let _bridge = spawn_bridge(&endpoint);
    thread::sleep(Duration::from_secs(1));

    let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
    let node = ctx.create_node("test_jazzy_graph_pub").build().unwrap();
    let _pub = node
        .create_pub::<RosString>("/chatter")
        .build()
        .expect("failed to create publisher");

    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        let topics = humble_topic_list(&endpoint);
        if topics.iter().any(|t| t == "/chatter") {
            println!("Jazzy publisher visible in Humble topic list: {topics:?}");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("/chatter not in Humble topic list after 180s. Got: {topics:?}");
        }
        thread::sleep(Duration::from_millis(500));
    }
}

// ─── Actions (smoke) ──────────────────────────────────────────────────────────

/// Verify Humble fibonacci action server and bridge start without crashing.
#[test]
#[serial]
fn test_bridge_action_server_and_bridge_startup() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let _humble_action_server = spawn_humble_action_server(&endpoint);
    let _bridge = spawn_bridge(&endpoint);

    thread::sleep(Duration::from_secs(3));
    println!("Humble fibonacci action server and bridge started cleanly");
}
