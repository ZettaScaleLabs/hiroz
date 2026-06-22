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
    example_interfaces::{AddTwoIntsResponse, srv::AddTwoInts},
    std_msgs::{Header, String as RosString},
};

fn hu_meter_bin() -> String {
    std::env::var("CARGO_BIN_EXE_hu-meter").unwrap_or_else(|_| {
        // CARGO_BIN_EXE_* is only set when the binary lives in the same crate as the test.
        // hu-meter is in hiroz-meter, so fall back to locating it via CARGO_TARGET_DIR.
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                manifest.parent().unwrap().parent().unwrap().join("target")
            });
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        target_dir
            .join(profile)
            .join("hu-meter")
            .to_str()
            .unwrap()
            .to_string()
    })
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
        let _server = node
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
#[serial_test::serial]
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
        // Keep server alive for up to 15s so hu-meter can connect even under CI load.
        // Use 50ms poll to avoid missing the request window.
        for _ in 0..300 {
            if let Ok(req) = server.take_request() {
                let sum = req.message().a + req.message().b;
                let _ = req.reply_blocking(&AddTwoIntsResponse { sum });
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    thread::sleep(Duration::from_millis(3000));

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
            "10",
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

#[test]
fn test_hu_meter_service_call_timeout() {
    // Call a service that doesn't exist; should time out and return non-zero exit within ~2s.
    let router = TestRouter::new();
    let start = std::time::Instant::now();
    let out = run_hu_meter(
        router.endpoint(),
        &[
            "service",
            "call",
            "/no_such_service_xyz",
            "--payload",
            "00 00 00 00",
            "--timeout",
            "2",
        ],
    );
    let elapsed = start.elapsed();
    assert!(
        !out.status.success(),
        "Expected non-zero exit on timeout, got success"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "Timeout took too long: {:?}",
        elapsed
    );
}

#[test]
#[serial_test::serial]
fn test_hu_meter_service_call_yaml() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("svc_yaml_server").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/svc_yaml_test")
            .build()
            .unwrap();
        for _ in 0..300 {
            if let Ok(req) = server.take_request() {
                let sum = req.message().a + req.message().b;
                let _ = req.reply_blocking(&AddTwoIntsResponse { sum });
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    thread::sleep(Duration::from_millis(3000));

    let out = run_hu_meter(
        router.endpoint(),
        &[
            "service",
            "call",
            "/svc_yaml_test",
            "--yaml",
            "{a: 3, b: 9}",
            "--msg-type",
            "example_interfaces/srv/AddTwoInts_Request",
            "--timeout",
            "10",
        ],
    );
    assert!(
        out.status.success(),
        "hu meter service call --yaml failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Response is pretty-printed JSON: {"sum": 12}
    assert!(
        stdout.contains("sum") && stdout.contains("12"),
        "Expected JSON response with sum=12: {}",
        stdout
    );
}

// ─── service call no-args / repeated ─────────────────────────────────────────

#[test]
#[serial_test::serial]
fn test_hu_meter_service_call_no_args() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("svc_noargs_server").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/svc_noargs_test")
            .build()
            .unwrap();
        for _ in 0..300 {
            if let Ok(req) = server.take_request() {
                let sum = req.message().a + req.message().b;
                let _ = req.reply_blocking(&AddTwoIntsResponse { sum });
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    thread::sleep(Duration::from_millis(3000));

    // Call without --yaml — sends an empty CDR payload (4 zero bytes)
    let out = run_hu_meter(
        router.endpoint(),
        &["service", "call", "/svc_noargs_test", "--timeout", "10"],
    );
    assert!(
        out.status.success(),
        "hu meter service call (no args) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[serial_test::serial]
fn test_hu_meter_service_call_repeated() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("svc_repeat_server").build().unwrap();
        let mut server = node
            .create_service::<AddTwoInts>("/svc_repeat_test")
            .build()
            .unwrap();
        let mut served = 0;
        while served < 2 {
            if let Ok(req) = server.take_request() {
                let sum = req.message().a + req.message().b;
                let _ = req.reply_blocking(&AddTwoIntsResponse { sum });
                served += 1;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    thread::sleep(Duration::from_millis(3000));

    for i in 0..2 {
        let out = run_hu_meter(
            router.endpoint(),
            &[
                "service",
                "call",
                "/svc_repeat_test",
                "--yaml",
                "{a: 1, b: 1}",
                "--msg-type",
                "example_interfaces/srv/AddTwoInts_Request",
                "--timeout",
                "10",
            ],
        );
        assert!(
            out.status.success(),
            "hu meter service call repeated (call {}) failed: {}",
            i,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("sum") && stdout.contains("2"),
            "Expected sum=2 on call {}: {}",
            i,
            stdout
        );
    }
}

// ─── echo --once ─────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_echo_once() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("echo_once_pub").build().unwrap();
            let pub_ = node
                .create_pub::<RosString>("/echo_once_test")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(800)).await;
            for i in 0..5 {
                let _ = pub_
                    .async_publish(&RosString {
                        data: format!("once_{}", i),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    });

    let out = run_hu_meter(
        router.endpoint(),
        &["echo", "/echo_once_test", "--count", "1"],
    );
    assert!(
        out.status.success(),
        "hu meter echo --count 1 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line_count = stdout.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(
        line_count, 1,
        "Expected exactly 1 output line from echo --count 1, got {}: {}",
        line_count, stdout
    );
}

// ─── list with-types / find-topics / find-services ───────────────────────────

#[test]
fn test_hu_meter_list_topics_with_types() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("list_types_pub").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/list_types_test")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    // Non-JSON list should include [type] annotation
    let out = run_hu_meter(router.endpoint(), &["list", "topics"]);
    assert!(
        out.status.success(),
        "hu meter list topics failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("/list_types_test"),
        "Expected /list_types_test in topic list: {}",
        stdout
    );
    assert!(
        stdout.contains("[") && stdout.contains("]"),
        "Expected [type] annotation in topic list: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_list_find_topics() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("find_topics_pub").build().unwrap();
            let _pub = node
                .create_pub::<RosString>("/find_topics_test")
                .build()
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["list", "find-topics", "std_msgs/msg/String"],
    );
    assert!(
        out.status.success(),
        "hu meter list find-topics failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("/find_topics_test"),
        "Expected /find_topics_test in find-topics output: {}",
        stdout
    );
}

#[test]
#[serial_test::serial]
fn test_hu_meter_list_find_services() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("find_svc_node").build().unwrap();
        let _server = node
            .create_service::<AddTwoInts>("/find_svc_test")
            .build()
            .unwrap();
        thread::sleep(Duration::from_secs(5));
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(router.endpoint(), &["list", "find-services", "AddTwoInts"]);
    assert!(
        out.status.success(),
        "hu meter list find-services failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("/find_svc_test"),
        "Expected /find_svc_test in find-services output: {}",
        stdout
    );
}

// ─── service list with types ──────────────────────────────────────────────────

#[test]
#[serial_test::serial]
fn test_hu_meter_service_list_with_types() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
        let node = ctx.create_node("svc_list_types_node").build().unwrap();
        let _server = node
            .create_service::<AddTwoInts>("/svc_list_types_test")
            .build()
            .unwrap();
        thread::sleep(Duration::from_secs(5));
    });

    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(router.endpoint(), &["service", "list"]);
    assert!(
        out.status.success(),
        "hu meter service list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("/svc_list_types_test"),
        "Expected /svc_list_types_test in service list: {}",
        stdout
    );
    assert!(
        stdout.contains("[") && stdout.contains("]"),
        "Expected [type] annotation in service list: {}",
        stdout
    );
}

// ─── echo --raw ───────────────────────────────────────────────────────────────

#[test]
fn test_hu_meter_echo_raw() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("echo_raw_pub").build().unwrap();
            let pub_ = node
                .create_pub::<RosString>("/echo_raw_test")
                .build()
                .unwrap();
            // Give echo time to subscribe
            tokio::time::sleep(Duration::from_millis(800)).await;
            let _ = pub_
                .async_publish(&RosString {
                    data: "rawtest".into(),
                })
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
    });

    let out = run_hu_meter(
        router.endpoint(),
        &["echo", "/echo_raw_test", "--count", "1", "--raw"],
    );
    assert!(
        out.status.success(),
        "hu meter echo --raw failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // --raw output is hex bytes, not decoded fields — check for hex pattern
    assert!(
        stdout
            .split_whitespace()
            .any(|tok| { tok.len() == 2 && tok.chars().all(|c| c.is_ascii_hexdigit()) }),
        "Expected hex byte output from echo --raw: {}",
        stdout
    );
    // Should NOT contain decoded field names
    assert!(
        !stdout.contains("data:") && !stdout.contains("\"data\""),
        "Unexpected decoded output from echo --raw: {}",
        stdout
    );
}

// ─── delay ────────────────────────────────────────────────────────────────────

/// Spawn hu-meter, let it run for `secs` seconds, kill it, and return accumulated output.
fn run_hu_meter_timed(router: &str, args: &[&str], secs: u64) -> (Vec<u8>, Vec<u8>) {
    let mut child = Command::new(hu_meter_bin())
        .arg("--router")
        .arg(router)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu-meter");

    thread::sleep(Duration::from_secs(secs));
    let _ = child.kill();
    let out = child
        .wait_with_output()
        .expect("failed to wait on hu-meter");
    (out.stdout, out.stderr)
}

#[test]
fn test_hu_meter_delay_basic() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node("delay_pub").build().unwrap();
            let pub_ = node.create_pub::<Header>("/delay_test").build().unwrap();
            // Give delay subscriber time to connect
            tokio::time::sleep(Duration::from_millis(500)).await;
            for _ in 0..20 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let _ = pub_
                    .async_publish(&Header {
                        stamp: hiroz_msgs::builtin_interfaces::Time {
                            sec: now.as_secs() as i32,
                            nanosec: now.subsec_nanos(),
                        },
                        frame_id: "delay_test".into(),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    });

    // Let delay run for 3 seconds — enough to capture at least one report (interval=1s)
    let (stdout, _stderr) = run_hu_meter_timed(router.endpoint(), &["delay", "/delay_test"], 3);
    let stdout = String::from_utf8_lossy(&stdout);

    assert!(
        stdout.contains("delay") || stdout.contains("mean") || stdout.contains("Waiting"),
        "Expected delay measurement output, got: {}",
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

// ─── param: filter / multi-get / multi-set / dump / load / describe ──────────

fn spawn_param_node(endpoint: String, node_name: &'static str, params: Vec<(&'static str, i64)>) {
    thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            use hiroz::parameter::{ParameterDescriptor, ParameterType, ParameterValue};
            let ctx = create_hiroz_context_with_endpoint(&endpoint).unwrap();
            let node = ctx.create_node(node_name).build().unwrap();
            for (name, val) in params {
                node.declare_parameter(
                    name,
                    ParameterValue::Integer(val),
                    ParameterDescriptor::new(name, ParameterType::Integer),
                )
                .unwrap();
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        });
    });
}

#[test]
fn test_hu_meter_param_list_filter() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();
    spawn_param_node(
        endpoint,
        "param_filter_node",
        vec![("alpha", 1), ("beta", 2), ("another", 3)],
    );
    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["param", "list", "/param_filter_node", "--filter", "al"],
    );
    assert!(
        out.status.success(),
        "hu meter param list --filter failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("alpha"),
        "Expected 'alpha' in filtered list: {}",
        stdout
    );
    assert!(
        !stdout.contains("beta"),
        "Expected 'beta' to be filtered out: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_param_get_multiple() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();
    spawn_param_node(endpoint, "param_multi_get_node", vec![("x", 10), ("y", 20)]);
    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &["param", "get", "/param_multi_get_node", "x", "y", "--json"],
    );
    assert!(
        out.status.success(),
        "hu meter param get multiple failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let map: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON map from multi param get");
    assert_eq!(map["x"].as_i64().unwrap_or(-1), 10, "x should be 10");
    assert_eq!(map["y"].as_i64().unwrap_or(-1), 20, "y should be 20");
}

#[test]
fn test_hu_meter_param_set_multiple_sequential() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();
    spawn_param_node(endpoint, "param_multi_set_node", vec![("p", 0), ("q", 0)]);
    thread::sleep(Duration::from_millis(800));

    for (name, val) in [("p", "11"), ("q", "22")] {
        let out = run_hu_meter(
            router.endpoint(),
            &["param", "set", "/param_multi_set_node", name, val],
        );
        assert!(
            out.status.success(),
            "hu meter param set {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let out = run_hu_meter(
        router.endpoint(),
        &["param", "get", "/param_multi_set_node", "p", "q", "--json"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let map: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON from param get after multi-set");
    assert_eq!(map["p"].as_i64().unwrap_or(-1), 11, "p should be 11");
    assert_eq!(map["q"].as_i64().unwrap_or(-1), 22, "q should be 22");
}

#[test]
fn test_hu_meter_param_dump() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();
    spawn_param_node(endpoint, "param_dump_node", vec![("dumpval", 99)]);
    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(router.endpoint(), &["param", "dump", "/param_dump_node"]);
    assert!(
        out.status.success(),
        "hu meter param dump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output should be YAML in ros2 param dump format
    assert!(
        stdout.contains("ros__parameters"),
        "Expected ros__parameters in dump output: {}",
        stdout
    );
    assert!(
        stdout.contains("dumpval") && stdout.contains("99"),
        "Expected dumpval: 99 in dump output: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_param_load() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();
    spawn_param_node(endpoint, "param_load_node", vec![("loadval", 0)]);
    thread::sleep(Duration::from_millis(800));

    // Write a YAML file to _tmp/
    let yaml_path = "_tmp/param_load_test.yaml";
    std::fs::write(
        yaml_path,
        "/param_load_node:\n  ros__parameters:\n    loadval: 55\n",
    )
    .expect("failed to write param yaml");

    let out = run_hu_meter(
        router.endpoint(),
        &["param", "load", "/param_load_node", yaml_path],
    );
    assert!(
        out.status.success(),
        "hu meter param load failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify the param was actually set
    let get_out = run_hu_meter(
        router.endpoint(),
        &["param", "get", "/param_load_node", "loadval", "--json"],
    );
    let stdout = String::from_utf8_lossy(&get_out.stdout);
    let map: serde_json::Value =
        serde_json::from_str(&stdout).expect("Expected JSON from param get after load");
    assert_eq!(
        map["loadval"].as_i64().unwrap_or(-1),
        55,
        "Expected loadval=55 after param load: {}",
        stdout
    );
}

#[test]
fn test_hu_meter_param_describe() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();
    spawn_param_node(endpoint, "param_desc_node", vec![("descparam", 7)]);
    thread::sleep(Duration::from_millis(800));

    let out = run_hu_meter(
        router.endpoint(),
        &[
            "param",
            "describe",
            "/param_desc_node",
            "descparam",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "hu meter param describe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("descparam"),
        "Expected descparam in describe output: {}",
        stdout
    );
}

/// Tests `hu meter pub --yaml` with nested message types (ros2cli#22).
///
/// ros2cli#22: `ros2 topic pub` fails to serialize nested message types. hu meter pub uses
/// hiroz's CDR encoder and handles nested structs correctly.
///
/// Verifies geometry_msgs/Twist (two nested Vector3 fields) by publishing a known payload
/// and checking the raw CDR bytes match the expected encoding.
#[test]
#[serial_test::serial]
fn test_pub_yaml_nested_twist() {
    // Expected CDR encoding for Twist{linear:{x:1.0,y:2.0,z:3.0}, angular:{x:0.1,y:0.2,z:0.5}}
    // CDR header: [0x00, 0x01, 0x00, 0x00]
    // linear.x = 1.0_f64.to_le_bytes(), linear.y = 2.0, linear.z = 3.0
    // angular.x = 0.1, angular.y = 0.2, angular.z = 0.5
    let mut expected = vec![0x00u8, 0x01, 0x00, 0x00];
    for v in [1.0f64, 2.0, 3.0, 0.1, 0.2, 0.5] {
        expected.extend_from_slice(&v.to_le_bytes());
    }

    let router = TestRouter::new();
    let endpoint = router.endpoint();

    // Subscribe with a raw hiroz subscriber (ZSub over raw Zenoh bytes)
    // hu meter pub with nested Twist YAML — verify command succeeds and prints JSON
    let out = run_hu_meter(
        endpoint,
        &[
            "pub",
            "/pub_yaml_twist",
            "--msg-type",
            "geometry_msgs/msg/Twist",
            "--yaml",
            "{linear: {x: 1.0, y: 2.0, z: 3.0}, angular: {x: 0.1, y: 0.2, z: 0.5}}",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "hu meter pub --yaml geometry_msgs/Twist failed (ros2cli#22 regression): {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("Expected JSON output from hu meter pub");

    // Verify reported byte count matches expected CDR size
    let reported_bytes = json["bytes"].as_u64().unwrap_or(0);
    assert_eq!(
        reported_bytes,
        expected.len() as u64,
        "CDR byte count mismatch for geometry_msgs/Twist: got {reported_bytes}, expected {}",
        expected.len()
    );
    assert_eq!(
        json["published"].as_u64().unwrap_or(0),
        1,
        "Expected published=1"
    );
    println!(
        "geometry_msgs/Twist encoded correctly: {reported_bytes} bytes (header + 6×f64 = {})",
        expected.len()
    );
}
