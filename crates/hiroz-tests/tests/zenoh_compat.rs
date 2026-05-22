//! Direct zenoh compatibility test: zenoh-c 1.6.2 (via rmw_zenoh_cpp) ↔ zenoh 1.9.0.
//!
//! No bridge involved. Verifies that a humble rmw_zenoh_cpp publisher can be
//! received by a raw zenoh 1.9.0 subscriber on the same key expression, and
//! vice versa. This confirms (or denies) wire-level pub/sub compatibility.
//!
//! Run with:
//!   cargo test -p hiroz-tests --test zenoh_compat --features humble-jazzy-bridge-tests -- --nocapture

#![cfg(feature = "humble-jazzy-bridge-tests")]

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use common::TestRouter;
use serial_test::serial;
use zenoh::{Wait, bytes::ZBytes};

/// Key expression that rmw_zenoh_cpp humble uses for `/chatter` (std_msgs/String).
/// Domain 0, topic chatter, type std_msgs::msg::dds_::String_, humble hash sentinel.
const HUMBLE_CHATTER_KE: &str = "0/chatter/std_msgs::msg::dds_::String_/TypeHashNotSupported";

/// Test: humble talker (zenoh-c 1.6.2) → raw zenoh 1.9.0 subscriber.
///
/// If zenoh-c 1.6.2 and zenoh 1.9.0 are pub/sub compatible, the subscriber
/// should receive messages published by the humble talker.
#[test]
#[serial]
fn test_zenoh_c_162_pub_to_zenoh_190_sub() {
    let router = TestRouter::new();
    let endpoint = router.endpoint();

    // Raw zenoh 1.9.0 subscriber on the humble KE.
    let mut cfg = zenoh::Config::default();
    cfg.insert_json5("mode", r#""peer""#).unwrap();
    cfg.insert_json5("connect/endpoints", &format!(r#"["{}"]"#, endpoint))
        .unwrap();
    cfg.insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let session = zenoh::open(cfg).wait().expect("open zenoh session");

    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();
    let _sub = session
        .declare_subscriber(HUMBLE_CHATTER_KE)
        .callback(move |sample| {
            let bytes = sample.payload().to_bytes();
            println!(
                "[zenoh-sub] received {} bytes on {}",
                bytes.len(),
                sample.key_expr()
            );
            received_clone.store(true, Ordering::Relaxed);
        })
        .wait()
        .expect("declare subscriber");

    // Spawn humble rmw_zenoh_cpp talker.
    let _talker = common::spawn_humble_ros2_talker(&endpoint, "/chatter");

    // Wait up to 90s — humble talker takes 30-60s to start in nix shell.
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while std::time::Instant::now() < deadline {
        if received.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    assert!(
        received.load(Ordering::Relaxed),
        "zenoh 1.9.0 subscriber received NOTHING from humble rmw_zenoh_cpp talker (zenoh-c 1.6.2) \
         — zenoh-c 1.6.2 pub/sub is incompatible with zenoh 1.9.0"
    );
    println!("PASS: zenoh-c 1.6.2 → zenoh 1.9.0 pub/sub works");
}

/// Test: humble talker (zenoh-c 1.6.2) → zenoh 1.9.0 subscriber in a SEPARATE process.
///
/// The previous test had the subscriber co-located with the router (same process).
/// This test spawns the subscriber as a separate process to match the bridge topology.
/// If this fails while test_zenoh_c_162_pub_to_zenoh_190_sub passes, the issue is
/// cross-process routing when publisher connects before subscriber registers.
#[test]
#[serial]
fn test_zenoh_c_162_pub_to_zenoh_190_sub_cross_process() {
    use std::process::{Command, Stdio};

    let router = TestRouter::new();
    let endpoint = router.endpoint();

    // Write a small helper script that opens a subscriber and waits.
    // We use the bridge binary indirectly — actually let's use the bridge-sim binary
    // which supports "jazzy-sub" mode.
    // Instead: spawn a simple python subprocess or use a pre-built helper.
    // Simplest: spawn another instance of THIS test binary with a special env var.

    // Actually, use a simpler approach: spawn the current test binary with a marker env
    // to act as a subscriber-only process, writing received count to a temp file.
    let tmp_file = format!("/tmp/zenoh_compat_sub_{}.txt", std::process::id());

    // Spawn subscriber process: runs the binary with SUB_MODE=1
    let mut sub_proc = Command::new(std::env::current_exe().unwrap())
        .args(["sub_process_helper", "--nocapture"])
        .env("ZENOH_COMPAT_SUB_ENDPOINT", &endpoint)
        .env("ZENOH_COMPAT_SUB_OUTPUT", &tmp_file)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn sub process");

    std::thread::sleep(Duration::from_millis(500));

    // Spawn humble talker after subscriber is ready.
    let _talker = common::spawn_humble_ros2_talker(&endpoint, "/chatter");

    // Wait for result.
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while std::time::Instant::now() < deadline {
        if std::path::Path::new(&tmp_file).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    sub_proc.kill().ok();

    let received = std::path::Path::new(&tmp_file).exists();
    let _ = std::fs::remove_file(&tmp_file);

    assert!(
        received,
        "cross-process subscriber received NOTHING — routing fails when subscriber is in a \
         separate process from the router"
    );
    println!("PASS: zenoh-c 1.6.2 → zenoh 1.9.0 sub (cross-process) works");
}

// Helper: run as a subscriber-only process (used by tests above).
// Not a test itself — called via sub-process spawn.
#[test]
fn sub_process_helper() {
    let endpoint = match std::env::var("ZENOH_COMPAT_SUB_ENDPOINT") {
        Ok(e) => e,
        Err(_) => return, // Not called as a helper, skip.
    };
    let output_file = std::env::var("ZENOH_COMPAT_SUB_OUTPUT").unwrap();
    let mode = std::env::var("ZENOH_COMPAT_SUB_MODE").unwrap_or_else(|_| "peer".into());

    let mut cfg = zenoh::Config::default();
    cfg.insert_json5("mode", &format!(r#""{}""#, mode)).unwrap();
    cfg.insert_json5("connect/endpoints", &format!(r#"["{}"]"#, endpoint))
        .unwrap();
    cfg.insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let session = zenoh::open(cfg).wait().expect("open session");

    let output_clone = output_file.clone();
    let _sub = session
        .declare_subscriber(HUMBLE_CHATTER_KE)
        .callback(move |sample| {
            let bytes = sample.payload().to_bytes();
            println!("[sub-proc] received {} bytes", bytes.len());
            let _ = std::fs::write(&output_clone, "received");
        })
        .wait()
        .expect("declare subscriber");

    println!("[sub-proc] subscriber ready on {HUMBLE_CHATTER_KE}");
    std::thread::sleep(Duration::from_secs(120));
}

/// Same as test_zenoh_c_162_pub_to_zenoh_190_sub_cross_process but subscriber uses CLIENT mode.
/// If peer mode fails but client mode succeeds, the fix for the bridge is to use client mode
/// for the humble_session.
#[test]
#[serial]
fn test_zenoh_c_162_pub_to_zenoh_190_client_sub_cross_process() {
    use std::process::{Command, Stdio};

    let router = TestRouter::new();
    let endpoint = router.endpoint();
    let tmp_file = format!("/tmp/zenoh_compat_client_sub_{}.txt", std::process::id());

    let mut sub_proc = Command::new(std::env::current_exe().unwrap())
        .args(["sub_process_helper", "--nocapture"])
        .env("ZENOH_COMPAT_SUB_ENDPOINT", &endpoint)
        .env("ZENOH_COMPAT_SUB_OUTPUT", &tmp_file)
        .env("ZENOH_COMPAT_SUB_MODE", "client") // CLIENT mode
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn sub process");

    std::thread::sleep(Duration::from_millis(500));

    let _talker = common::spawn_humble_ros2_talker(&endpoint, "/chatter");

    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while std::time::Instant::now() < deadline {
        if std::path::Path::new(&tmp_file).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    sub_proc.kill().ok();

    let received = std::path::Path::new(&tmp_file).exists();
    let _ = std::fs::remove_file(&tmp_file);

    assert!(
        received,
        "client-mode cross-process subscriber received NOTHING"
    );
    println!("PASS: zenoh-c 1.6.2 → zenoh 1.9.0 CLIENT sub (cross-process) works");
}

/// Test: raw zenoh 1.9.0 publisher → humble listener (zenoh-c 1.6.2).
///
/// Publishes raw CDR bytes for std_msgs/String on the humble KE.
/// If the humble listener receives them, the reverse direction works too.
#[test]
#[serial]
fn test_zenoh_190_pub_to_zenoh_c_162_sub() {
    let router = TestRouter::new();
    let endpoint = router.endpoint();

    // Spawn humble listener first.
    let _listener = common::spawn_humble_ros2_listener(&endpoint, "/chatter");
    std::thread::sleep(Duration::from_secs(45)); // wait for humble listener to start

    // Raw zenoh 1.9.0 publisher on the humble KE.
    let mut cfg = zenoh::Config::default();
    cfg.insert_json5("mode", r#""peer""#).unwrap();
    cfg.insert_json5("connect/endpoints", &format!(r#"["{}"]"#, endpoint))
        .unwrap();
    cfg.insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let session = zenoh::open(cfg).wait().expect("open zenoh session");

    // CDR encoding for std_msgs/String "hello from zenoh 1.9.0":
    // 4-byte CDR header (little-endian: 00 01 00 00) + 4-byte string length + string bytes + null
    let msg = b"hello from zenoh 1.9.0";
    let mut cdr = Vec::new();
    cdr.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // CDR header LE
    let len = (msg.len() + 1) as u32;
    cdr.extend_from_slice(&len.to_le_bytes());
    cdr.extend_from_slice(msg);
    cdr.push(0); // null terminator

    println!(
        "[zenoh-pub] publishing {} bytes on {}",
        cdr.len(),
        HUMBLE_CHATTER_KE
    );
    for i in 0..10 {
        session
            .put(HUMBLE_CHATTER_KE, ZBytes::from(cdr.clone()))
            .wait()
            .expect("put");
        println!("[zenoh-pub] sent message {i}");
        std::thread::sleep(Duration::from_secs(1));
    }

    // This test is observational — we can't easily check if the humble listener
    // received anything from Rust. Run with --nocapture and check humble listener output.
    println!(
        "Check humble listener output above for 'hello from zenoh 1.9.0' — \
         if present, reverse direction works"
    );
}
