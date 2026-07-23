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
use hiroz_msgs::{
    example_interfaces::srv::AddTwoInts,
    rcl_interfaces::{
        GetLoggerLevelsResponse, LoggerLevel, SetLoggerLevelsResponse, SetLoggerLevelsResult,
        srv::{GetLoggerLevels, SetLoggerLevels},
    },
    std_msgs::String as RosString,
};

/// Run `hu monitor <args>` with a specific router endpoint, capturing all output.
fn run_hu_monitor(router: &str, args: &[&str]) -> std::process::Output {
    Command::new("hu")
        .arg("--connect")
        .arg(router)
        .arg("monitor")
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
        .arg("--connect")
        .arg(router.endpoint())
        .arg("monitor")
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

// ─── log (/rosout subscription, --count) ─────────────────────────────────────

/// `hu monitor log --count <n>` subscribes to /rosout and must stop after
/// exactly <n> messages rather than streaming forever. hiroz core does not ship
/// rcl_interfaces/msg/Log, so a String publisher stands in on /rosout purely to
/// exercise the subscribe + --count-limit + clean-exit code path.
#[test]
#[serial_test::serial]
fn test_monitor_log_count_limits_output() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx
                .create_node("rosout_pub_node")
                .with_type_description_service()
                .build()
                .unwrap();
            let pub_ = node.create_pub::<RosString>("/rosout").build().unwrap();
            tokio::time::sleep(Duration::from_millis(1000)).await;
            for i in 0..20 {
                let _ = pub_
                    .async_publish(&RosString {
                        data: format!("log line {i}"),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        });
    });

    let out = run_hu_monitor(router.endpoint(), &["log", "--count", "2"]);
    assert!(
        out.status.success(),
        "hu monitor log --count 2 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        lines, 2,
        "Expected exactly 2 log lines from --count 2, got {lines}: {stdout}"
    );
}

// ─── log-level (get/set via rcl_interfaces service) ──────────────────────────

/// `hu monitor log-level <node>` connects to `<node>/get_logger_levels`. hiroz
/// core does not implement logger-level services, so this asserts the command's
/// graceful-failure path: it must terminate promptly (no hang) and emit an
/// ERROR rather than panicking or blocking when the service is absent. This
/// guards the LogLevel dispatch + connect-service host call + clean-exit path.
#[test]
#[serial_test::serial]
fn test_monitor_log_level_reports_missing_service() {
    let router = TestRouter::new();

    let start = std::time::Instant::now();
    let out = run_hu_monitor(
        router.endpoint(),
        &["log-level", "/no_such_logger_node_xyzzy"],
    );
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(15),
        "hu monitor log-level took too long ({elapsed:?}) — possible hang"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("ERROR"),
        "Expected an ERROR line when the logger service is absent: {combined}"
    );
}

// ─── watch: node + service appearance ────────────────────────────────────────

/// The watch diffing loop reports node and service appearance the same way it
/// reports topics. test_monitor_watch_fires_on_topic_create only covers topics;
/// this covers the node-appeared and service-appeared branches (lib.rs diffing),
/// including the namespace+name concatenation for nodes.
#[test]
#[serial_test::serial]
fn test_monitor_watch_fires_on_node_and_service() {
    let router = TestRouter::new();

    let mut watch_child = Command::new("hu")
        .arg("--connect")
        .arg(router.endpoint())
        .arg("monitor")
        .arg("watch")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu monitor watch");

    // Let watch record the initial (empty) graph snapshot.
    thread::sleep(Duration::from_secs(2));

    let endpoint = router.endpoint().to_string();
    let node_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("watch_ns_node").build().unwrap();
            let _srv = node
                .create_service::<AddTwoInts>("/watch_appear_service")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    // Wait for several watch ticks (tick_ms = 1000ms) to detect the changes.
    thread::sleep(Duration::from_secs(4));

    let _ = watch_child.kill();
    let watch_out = watch_child
        .wait_with_output()
        .expect("failed to collect watch output");
    node_handle.join().ok();

    let stdout = String::from_utf8_lossy(&watch_out.stdout);
    let stderr = String::from_utf8_lossy(&watch_out.stderr);
    assert!(
        stdout.contains("node appeared:") && stdout.contains("watch_ns_node"),
        "Expected 'node appeared: ...watch_ns_node'; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("service appeared:") && stdout.contains("watch_appear_service"),
        "Expected 'service appeared: /watch_appear_service'; stdout: {stdout}\nstderr: {stderr}"
    );
}

// ─── log-level GET/SET success paths (real logger services) ──────────────────
//
// hiroz core does not ship logger-level services, so these tests stand up a
// node that exposes real `rcl_interfaces/srv/GetLoggerLevels` and
// `SetLoggerLevels` servers (generated from hiroz-codegen/assets). This lets
// `hu monitor log-level` exercise its actual get_logger_levels/set_logger_levels
// service-call logic end-to-end — the GET-then-SET success paths that the
// docs advertise as worked examples — instead of only the missing-service
// error branch covered by test_monitor_log_level_reports_missing_service.

/// Spawn a node exposing `<base>/get_logger_levels` and `<base>/set_logger_levels`.
/// GET always reports `level` for the requested logger; SET replies success.
fn spawn_logger_service_node(endpoint: String, base: &'static str, level: u32) {
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx
            .create_node("loglevel_srv")
            .with_type_description_service()
            .build()
            .unwrap();
        let mut get_server = node
            .create_service::<GetLoggerLevels>(&format!("{base}/get_logger_levels"))
            .build()
            .unwrap();
        let mut set_server = node
            .create_service::<SetLoggerLevels>(&format!("{base}/set_logger_levels"))
            .build()
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if let Ok(req) = get_server.take_request() {
                let name = req
                    .message()
                    .names
                    .first()
                    .cloned()
                    .unwrap_or_else(|| base.to_string());
                let _ = req.reply_blocking(&GetLoggerLevelsResponse {
                    levels: vec![LoggerLevel { name, level }],
                });
            }
            if let Ok(req) = set_server.take_request() {
                let n = req.message().levels.len();
                let _ = req.reply_blocking(&SetLoggerLevelsResponse {
                    results: (0..n)
                        .map(|_| SetLoggerLevelsResult {
                            successful: true,
                            reason: String::new(),
                        })
                        .collect(),
                });
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
}

#[test]
#[serial_test::serial]
fn test_monitor_log_level_get_roundtrip() {
    let router = TestRouter::new();
    spawn_logger_service_node(router.endpoint().to_string(), "/loglevel_get_node", 20);
    thread::sleep(Duration::from_millis(2500));

    let out = run_hu_monitor(router.endpoint(), &["log-level", "/loglevel_get_node"]);
    assert!(
        out.status.success(),
        "hu monitor log-level (get) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Plugin prints `log levels: <decoded-json>`; the decoded response must carry
    // the level (20 = INFO) our server reported.
    assert!(
        stdout.contains("log levels") && stdout.contains("20"),
        "Expected decoded logger levels with level 20 in output: {stdout}"
    );
    assert!(
        !stdout.contains("ERROR"),
        "log-level GET success path should not report an error: {stdout}"
    );
}

#[test]
#[serial_test::serial]
fn test_monitor_log_level_set_roundtrip() {
    let router = TestRouter::new();
    spawn_logger_service_node(router.endpoint().to_string(), "/loglevel_set_node", 20);
    thread::sleep(Duration::from_millis(2500));

    // `log-level <node> DEBUG` runs GET then SET; assert the SET confirmation.
    let out = run_hu_monitor(
        router.endpoint(),
        &["log-level", "/loglevel_set_node", "DEBUG"],
    );
    assert!(
        out.status.success(),
        "hu monitor log-level (set) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("set log level to DEBUG"),
        "Expected 'set log level to DEBUG' confirmation in output: {stdout}"
    );
    assert!(
        !stdout.contains("ERROR: set"),
        "log-level SET success path should not report a set error: {stdout}"
    );
}

// ─── log default (unbounded) streaming: count defaults to 0 ──────────────────

/// `hu monitor log` with no `--count` uses count=0, which means "stream every
/// /rosout message indefinitely" (not "zero and exit"). Publish a burst, let it
/// run, kill it, and assert it printed *more than one* line — proving 0 is
/// unlimited streaming, so a regression flipping the default to exit-immediately
/// would be caught.
#[test]
#[serial_test::serial]
fn test_monitor_log_default_streams_unbounded() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx
                .create_node("rosout_stream_node")
                .with_type_description_service()
                .build()
                .unwrap();
            let pub_ = node.create_pub::<RosString>("/rosout").build().unwrap();
            tokio::time::sleep(Duration::from_millis(1000)).await;
            for i in 0..30 {
                let _ = pub_
                    .async_publish(&RosString {
                        data: format!("stream line {i}"),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        });
    });

    // No --count → count=0 → stream forever; run for a few seconds then kill.
    let mut child = Command::new("hu")
        .arg("--connect")
        .arg(router.endpoint())
        .arg("monitor")
        .arg("log")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu monitor log");
    thread::sleep(Duration::from_secs(4));
    let _ = child.kill();
    let out = child.wait_with_output().expect("wait on hu monitor log");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    // With count=0 meaning "stop after zero" it would print nothing; unbounded
    // streaming prints many. Require clearly more than one to disambiguate.
    assert!(
        lines >= 3,
        "Expected many streamed /rosout lines (count=0 == unbounded), got {lines}: {stdout}"
    );
}

// ─── HU_CONNECT / HU_DOMAIN environment configuration ─────────────────────────

/// docs/tools/hu.md's Quick Start recommends exporting HU_CONNECT/HU_DOMAIN
/// once per session instead of passing --connect each time. Every other test
/// uses the explicit --connect flag; this one omits the flags entirely and
/// configures the router via the environment, so a regression in the env-var
/// parsing (or a default flag value silently winning) would be caught.
#[test]
#[serial_test::serial]
fn test_monitor_env_var_router_config() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("env_monitor_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/env_monitor_topic")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(8)).await;
        });
    });

    thread::sleep(Duration::from_millis(1000));

    // No --connect / --domain flags — configure entirely via the environment.
    let out = Command::new("hu")
        .env("HU_CONNECT", router.endpoint())
        .env("HU_DOMAIN", "0")
        .args(["monitor", "graph", "--json", "--once"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run hu monitor via env config");

    assert!(
        out.status.success(),
        "hu monitor graph via HU_CONNECT failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Expected JSON from env-configured hu monitor: {e}\n{stdout}"));
    let topics = json["topics"].as_array().expect("topics array");
    assert!(
        topics.iter().any(|t| t["name"]
            .as_str()
            .unwrap_or("")
            .contains("env_monitor_topic")),
        "Env-configured hu must reach the router and see /env_monitor_topic: {stdout}"
    );
}
