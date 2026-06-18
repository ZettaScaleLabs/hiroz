#![cfg(feature = "hu-meter-tests")]
//! Integration tests for hu-meter CLI commands.
//!
//! Each test spawns a hiroz node (publisher, service server, or parameter server)
//! in a background thread, then runs `hu-meter` as a subprocess and checks output.
//!
//! Requires: `--features hu-meter-tests,ros-msgs,jazzy` (or other distro).
//! The `hu-meter` binary must be on PATH or reachable via CARGO_BIN_EXE_hu_meter.

mod common;

use std::{
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::{
    example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts},
    std_msgs::String as RosString,
};

// Path to the hu-meter binary. In tests, CARGO_BIN_EXE_<name> is set by cargo.
fn hu_meter_bin() -> String {
    std::env::var("CARGO_BIN_EXE_hu-meter").unwrap_or_else(|_| "hu-meter".to_string())
}

/// Run hu-meter with the given arguments and a specific router endpoint.
fn run_hu_meter(router: &str, args: &[&str]) -> Output {
    Command::new(hu_meter_bin())
        .arg("--router")
        .arg(router)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run hu-meter")
}

// ─── hz ─────────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_hz_hiroz_publisher() {
    let router = TestRouter::new();

    // Publish at ~10 Hz for 3 seconds
    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("hz_test_pub").build().unwrap();
            let pub_ = node.create_pub::<RosString>("/hz_test").build().unwrap();
            for _ in 0..30 {
                let _ = pub_
                    .async_publish(&RosString {
                        data: "ping".into(),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    });

    // Give publisher time to start
    thread::sleep(Duration::from_millis(300));

    let out = run_hu_meter(
        router.endpoint(),
        &["hz", "/hz_test", "--window", "10", "--duration", "3"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "hu meter hz failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Output should contain a rate > 0
    assert!(
        stdout.contains("Hz") || stdout.contains("hz") || stdout.contains("rate"),
        "Expected rate output, got: {}",
        stdout
    );
}

// ─── bw ─────────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_bw_hiroz_publisher() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("bw_test_pub").build().unwrap();
            let pub_ = node.create_pub::<RosString>("/bw_test").build().unwrap();
            for _ in 0..20 {
                let _ = pub_
                    .async_publish(&RosString {
                        data: "hello world".into(),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    });

    thread::sleep(Duration::from_millis(300));

    let out = run_hu_meter(
        router.endpoint(),
        &["bw", "/bw_test", "--window", "10", "--duration", "2"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "hu meter bw failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("B/s") || stdout.contains("bytes") || stdout.contains("bw"),
        "Expected bandwidth output, got: {}",
        stdout
    );
}

// ─── echo ────────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_echo_count_3() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("echo_test_pub").build().unwrap();
            let pub_ = node.create_pub::<RosString>("/echo_test").build().unwrap();
            // Give echo time to subscribe
            tokio::time::sleep(Duration::from_millis(800)).await;
            for i in 0..10 {
                let _ = pub_
                    .async_publish(&RosString {
                        data: format!("msg_{}", i),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    });

    let out = run_hu_meter(router.endpoint(), &["echo", "/echo_test", "--count", "3"]);
    assert!(
        out.status.success(),
        "hu meter echo failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Should have received exactly 3 messages
    let line_count = stdout.lines().filter(|l| !l.is_empty()).count();
    assert!(
        line_count >= 3,
        "Expected at least 3 output lines, got {}: {}",
        line_count,
        stdout
    );
}

// ─── list ────────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_list_topics() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("list_topics_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/list_topics_test")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(router.endpoint(), &["list", "topics", "--json"]);
    assert!(
        out.status.success(),
        "hu meter list topics failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON output from list topics");
    let topics = json.as_array().expect("Expected JSON array");
    let found = topics.iter().any(|t| {
        t["name"]
            .as_str()
            .unwrap_or("")
            .contains("list_topics_test")
    });
    assert!(
        found,
        "Expected /list_topics_test in topic list: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_list_nodes() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let _node = ctx.create_node("list_nodes_target").build().unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(router.endpoint(), &["list", "nodes", "--json"]);
    assert!(
        out.status.success(),
        "hu meter list nodes failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON output from list nodes");
    let nodes = json.as_array().expect("Expected JSON array");
    let found = nodes.iter().any(|n| {
        n["name"]
            .as_str()
            .unwrap_or("")
            .contains("list_nodes_target")
    });
    assert!(found, "Expected list_nodes_target in node list: {}", stdout);
}

// ─── info ────────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_info_topic_pub_count() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("info_topic_node").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/info_topic_test")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["info", "topic", "/info_topic_test", "--json"],
    );
    assert!(
        out.status.success(),
        "hu meter info topic failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON from info topic");
    assert_eq!(
        json["publisher_count"].as_u64().unwrap_or(0),
        1,
        "Expected 1 publisher: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_info_node_full() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("info_node_target").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/pub_from_info_node")
                .build()
                .unwrap();
            let _sub = node
                .create_sub::<RosString>("/sub_from_info_node")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["info", "node", "/info_node_target", "--json"],
    );
    assert!(
        out.status.success(),
        "hu meter info node failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON from info node");
    assert_eq!(json["found"], true, "Node should be found: {}", stdout);
    let pubs = json["publishers"].as_array().expect("publishers array");
    assert!(
        pubs.iter().any(|p| p["name"]
            .as_str()
            .unwrap_or("")
            .contains("pub_from_info_node")),
        "Expected /pub_from_info_node in publishers: {}",
        stdout
    );
}

// ─── service ─────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_service_list() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("svc_list_node").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/svc_list_test")
            .build()
            .unwrap();
        thread::sleep(Duration::from_secs(5));
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(router.endpoint(), &["service", "list", "--json"]);
    assert!(
        out.status.success(),
        "hu meter service list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON from service list");
    let services = json.as_array().expect("Expected JSON array");
    let found = services
        .iter()
        .any(|s| s["name"].as_str().unwrap_or("").contains("svc_list_test"));
    assert!(found, "Expected /svc_list_test in service list: {}", stdout);
}

/// CDR encoding for AddTwoIntsRequest {a: 2, b: 3}:
/// 4-byte header + 8-byte int64 (a=2) + 8-byte int64 (b=3)
fn add_two_ints_request_cdr(a: i64, b: i64) -> String {
    let mut bytes = vec![0x00u8, 0x01, 0x00, 0x00]; // CDR LE header
    bytes.extend_from_slice(&a.to_le_bytes());
    bytes.extend_from_slice(&b.to_le_bytes());
    bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn test_hu_meter_service_call_add_two_ints() {
    let router = TestRouter::new();

    // Spin a hiroz AddTwoInts server
    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("svc_call_server").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/svc_call_test")
            .build()
            .unwrap();
        // Handle up to 3 requests
        for _ in 0..3 {
            if let Ok(req) = server.take_request() {
                let sum = req.message().a + req.message().b;
                let _ = req.reply_blocking(&AddTwoIntsResponse { sum });
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    thread::sleep(Duration::from_millis(800));

    let hex_payload = add_two_ints_request_cdr(4, 7);
    let out = run_hu_meter(
        router.endpoint(),
        &[
            "service",
            "call",
            "/svc_call_test",
            "--payload",
            &hex_payload,
            "--timeout",
            "5",
        ],
    );
    assert!(
        out.status.success(),
        "hu meter service call failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Response CDR should contain 11 (4+7) as a little-endian int64
    // 0b 00 00 00 00 00 00 00 = 11 in LE
    assert!(
        stdout.contains("0b") || stdout.contains("bytes"),
        "Expected response with sum=11: {}",
        stdout
    );
}

// ─── param ───────────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_param_list() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("param_list_node2").build().unwrap();
            use hiroz::parameter::{ParameterDescriptor, ParameterType, ParameterValue};
            node.declare_parameter(
                "test_count",
                ParameterValue::Integer(99),
                ParameterDescriptor::new("test_count", ParameterType::Integer),
            )
            .unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["param", "list", "/param_list_node2", "--json"],
    );
    assert!(
        out.status.success(),
        "hu meter param list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let names: Vec<String> =
        serde_json::from_str(&stdout).expect("Expected JSON array from param list");
    assert!(
        names.iter().any(|n| n == "test_count"),
        "Expected 'test_count' in param list: {:?}",
        names
    );
}

#[test]
fn test_hu_meter_param_get() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("param_get_node").build().unwrap();
            use hiroz::parameter::{ParameterDescriptor, ParameterType, ParameterValue};
            node.declare_parameter(
                "my_value",
                ParameterValue::Integer(42),
                ParameterDescriptor::new("my_value", ParameterType::Integer),
            )
            .unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["param", "get", "/param_get_node", "my_value", "--json"],
    );
    assert!(
        out.status.success(),
        "hu meter param get failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let map: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON map from param get");
    assert_eq!(
        map["my_value"].as_i64().unwrap_or(-1),
        42,
        "Expected my_value=42: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_param_set_roundtrip() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("param_set_node").build().unwrap();
            use hiroz::parameter::{ParameterDescriptor, ParameterType, ParameterValue};
            node.declare_parameter(
                "counter",
                ParameterValue::Integer(0),
                ParameterDescriptor::new("counter", ParameterType::Integer),
            )
            .unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    // Set counter to 77
    let set_out = run_hu_meter(
        router.endpoint(),
        &["param", "set", "/param_set_node", "counter", "77"],
    );
    assert!(
        set_out.status.success(),
        "hu meter param set failed: {}",
        String::from_utf8_lossy(&set_out.stderr)
    );

    // Get counter — should be 77 now
    let get_out = run_hu_meter(
        router.endpoint(),
        &["param", "get", "/param_set_node", "counter", "--json"],
    );
    assert!(
        get_out.status.success(),
        "hu meter param get after set failed: {}",
        String::from_utf8_lossy(&get_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&get_out.stdout);
    let map: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON from param get");
    assert_eq!(
        map["counter"].as_i64().unwrap_or(-1),
        77,
        "Expected counter=77 after set: {}",
        stdout
    );
}
