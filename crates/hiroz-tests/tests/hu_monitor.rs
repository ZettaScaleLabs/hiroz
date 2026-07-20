#![cfg(feature = "hu-monitor-tests")]
//! Integration tests for hu monitor WASM plugin commands.
//!
//! Each test spawns a hiroz node in a background thread, then runs
//! `hu monitor <subcommand>` as a subprocess and checks output.
//!
//! Requires: `--features hu-monitor-tests,ros-msgs,jazzy` (or other distro).
//! The `hu` binary must be on PATH and HU_PLUGIN_PATH must include hu-monitor.wasm.

mod common;

use std::{
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::{example_interfaces::srv::AddTwoInts, std_msgs::String as RosString};

/// Run `hu monitor <args>` with a specific router endpoint, capturing all output.
fn run_hu_monitor(router: &str, args: &[&str]) -> std::process::Output {
    Command::new("hu")
        .arg("monitor")
        .arg("--router")
        .arg(router)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run hu monitor")
}

// ─── graph ────────────────────────────────────────────────────────────────────

#[test]
fn test_monitor_graph_json_valid() {
    let router = TestRouter::new();

    // Spin a node so the graph is non-empty.
    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("graph_json_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/graph_json_topic")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_monitor(router.endpoint(), &["graph", "--json", "--once"]);
    assert!(
        out.status.success(),
        "hu monitor graph --json --once failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Expected JSON from hu monitor graph: {e}\noutput: {stdout}"));

    assert!(
        json.get("topics").is_some(),
        "Expected 'topics' field in graph JSON: {}",
        stdout
    );
    assert!(
        json.get("nodes").is_some(),
        "Expected 'nodes' field in graph JSON: {}",
        stdout
    );
    assert!(
        json.get("services").is_some(),
        "Expected 'services' field in graph JSON: {}",
        stdout
    );
}

#[test]
fn test_monitor_graph_once_exits() {
    let router = TestRouter::new();

    // `hu monitor graph --once` should exit cleanly (code 0) rather than hanging.
    let start = std::time::Instant::now();
    let out = run_hu_monitor(router.endpoint(), &["graph", "--once"]);
    let elapsed = start.elapsed();

    assert!(
        out.status.success(),
        "hu monitor graph --once exited with non-zero status: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "hu monitor graph --once took too long ({:?}) — possible hang",
        elapsed
    );
}

#[test]
fn test_monitor_graph_contains_known_topic() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("graph_topic_check_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/graph_check_topic")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_monitor(router.endpoint(), &["graph", "--json", "--once"]);
    assert!(
        out.status.success(),
        "hu monitor graph failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("Expected JSON from hu monitor graph");

    let topics = json["topics"].as_array().expect("topics must be array");
    let found = topics.iter().any(|t| {
        t["name"]
            .as_str()
            .unwrap_or("")
            .contains("graph_check_topic")
    });
    assert!(
        found,
        "Expected /graph_check_topic in graph topics: {}",
        stdout
    );
}

#[test]
fn test_monitor_graph_contains_known_node() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let _node = ctx.create_node("graph_node_check").build().unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_monitor(router.endpoint(), &["graph", "--json", "--once"]);
    assert!(
        out.status.success(),
        "hu monitor graph failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("Expected JSON from hu monitor graph");

    let nodes = json["nodes"].as_array().expect("nodes must be array");
    let found = nodes.iter().any(|n| {
        n["name"]
            .as_str()
            .unwrap_or("")
            .contains("graph_node_check")
    });
    assert!(
        found,
        "Expected graph_node_check in graph nodes: {}",
        stdout
    );
}

// ─── watch ────────────────────────────────────────────────────────────────────

#[test]
#[serial_test::serial]
fn test_monitor_watch_fires_on_topic_create() {
    let router = TestRouter::new();
    let endpoint_for_watch = router.endpoint().to_string();

    // Start `hu monitor watch` in the background — it polls graph changes each tick.
    let mut watch_child = Command::new("hu")
        .arg("monitor")
        .arg("--router")
        .arg(router.endpoint())
        .arg("watch")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu monitor watch");

    // Give watch time to record the initial graph snapshot.
    thread::sleep(Duration::from_secs(2));

    // Now create a new topic — watch should fire "topic appeared".
    let endpoint = router.endpoint().to_string();
    let pub_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("watch_fire_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/watch_fire_topic")
                .build()
                .unwrap();
            // Keep it alive long enough for watch to detect it.
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    // Wait for at least one more tick (tick_ms = 1000ms in the plugin manifest).
    thread::sleep(Duration::from_secs(3));

    let _ = watch_child.kill();
    let watch_out = watch_child
        .wait_with_output()
        .expect("failed to collect watch output");

    pub_handle.join().ok();

    let stdout = String::from_utf8_lossy(&watch_out.stdout);
    let stderr = String::from_utf8_lossy(&watch_out.stderr);
    assert!(
        stdout.contains("watch_fire_topic") || stdout.contains("topic appeared"),
        "Expected watch to report new topic; stdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    drop(endpoint_for_watch);
}

// ─── graph non-JSON (text output) ────────────────────────────────────────────

#[test]
fn test_monitor_graph_text_output_structure() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("graph_text_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/graph_text_topic")
                .build()
                .unwrap();
            let _srv = node
                .create_service::<AddTwoInts>("/graph_text_service")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_monitor(router.endpoint(), &["graph", "--once"]);
    assert!(
        out.status.success(),
        "hu monitor graph (text) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Text output should have section headers
    assert!(
        stdout.contains("Topics:"),
        "Expected 'Topics:' section in text output: {}",
        stdout
    );
    assert!(
        stdout.contains("Nodes:"),
        "Expected 'Nodes:' section in text output: {}",
        stdout
    );
    assert!(
        stdout.contains("Services:"),
        "Expected 'Services:' section in text output: {}",
        stdout
    );
    assert!(
        stdout.contains("/graph_text_topic"),
        "Expected /graph_text_topic in text output: {}",
        stdout
    );
}
