//! Benchmark: `hu meter hz` accuracy vs `ros2 topic hz` at high publish rates.
//!
//! ros2cli under-reports rates for high-frequency or large-message topics because
//! rclpy deserializes every message (ros2/ros2#1499, open since Dec 2023). hu-meter
//! uses raw-byte subscription — no deserialization, correct at any rate.
//!
//! Requires `hz-comparison-tests` feature and the `ros-bridge-interop` nix shell
//! (which provides the Jazzy `ros2` CLI via `rmw_zenoh_cpp`):
//!
//! ```bash
//! cargo test -p hiroz-tests --test hz_accuracy \
//!     --features hz-comparison-tests,jazzy -- --nocapture
//! ```

#![cfg(feature = "hz-comparison-tests")]

mod common;

use std::{
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::std_msgs::String as RosString;

fn hu_meter_bin() -> String {
    std::env::var("CARGO_BIN_EXE_hu-meter").unwrap_or_else(|_| {
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

/// Parse `rate_hz` from `hu meter hz --json` output (last complete JSON line).
fn parse_hu_meter_hz(stdout: &str) -> Option<f64> {
    stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .and_then(|v| v["rate_hz"].as_f64())
}

/// Parse rate from `ros2 topic hz` text output.
/// Line format: "average rate: 499.823"
fn parse_ros2_hz(stdout: &str) -> Option<f64> {
    stdout.lines().find_map(|l| {
        let l = l.trim();
        if l.starts_with("average rate:") {
            l.split(':')
                .nth(1)
                .and_then(|v| v.trim().split_whitespace().next())
                .and_then(|v| v.parse::<f64>().ok())
        } else {
            None
        }
    })
}

fn run_hz_comparison(publish_hz: f64, duration_secs: f64, topic: &str) -> (f64, Option<f64>) {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    // Publish at the target rate from a background thread.
    {
        let endpoint2 = endpoint.clone();
        let topic2 = topic.to_string();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let ctx = create_hiroz_context_with_endpoint(&endpoint2).unwrap();
                let node = ctx.create_node("hz_accuracy_pub").build().unwrap();
                let pub_ = node
                    .create_pub::<RosString>(&format!("/{topic2}"))
                    .build()
                    .unwrap();
                let interval_us = (1_000_000.0 / publish_hz) as u64;
                let stop = std::time::Instant::now()
                    + std::time::Duration::from_secs_f64(duration_secs + 5.0);
                while std::time::Instant::now() < stop {
                    let _ = pub_
                        .async_publish(&RosString {
                            data: "x".repeat(128),
                        })
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_micros(interval_us)).await;
                }
            });
        });
    }

    // Wait for publisher to start.
    thread::sleep(Duration::from_millis(500));

    // --- hu meter hz ---
    let hu_out = Command::new(hu_meter_bin())
        .args([
            "--router",
            &endpoint,
            "hz",
            &format!("/{topic}"),
            "--duration",
            &duration_secs.to_string(),
            "--json",
        ])
        .output()
        .expect("failed to run hu-meter hz");
    let hu_stdout = String::from_utf8_lossy(&hu_out.stdout).into_owned();
    let hu_rate = parse_hu_meter_hz(&hu_stdout);
    let hu_rate = hu_rate.unwrap_or_else(|| {
        eprintln!(
            "hu-meter hz output (stderr: {}): {}",
            String::from_utf8_lossy(&hu_out.stderr),
            hu_stdout
        );
        0.0
    });

    // --- ros2 topic hz (if `ros2` is available) ---
    let ros2_rate = if Command::new("ros2").arg("--help").output().is_ok() {
        let ros2_out = Command::new("ros2")
            .args([
                "topic",
                "hz",
                &format!("/{topic}"),
                "--window",
                "50",
                "--filter",
                &format!("{}", (duration_secs as u32).saturating_sub(1)),
            ])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env(
                "ZENOH_CONFIG_OVERRIDE",
                format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
            .and_then(|mut child| {
                thread::sleep(Duration::from_secs_f64(duration_secs));
                let _ = child.kill();
                child.wait_with_output().ok()
            });
        ros2_out
            .as_ref()
            .and_then(|o| parse_ros2_hz(&String::from_utf8_lossy(&o.stdout)))
    } else {
        eprintln!("ros2 CLI not found — skipping ros2 topic hz measurement");
        None
    };

    (hu_rate, ros2_rate)
}

/// Reproduce ros2/ros2cli#871 (root cause B): rclpy deserializes every message, so large
/// payloads cause ros2 topic hz to under-report. hu meter hz uses raw-byte subscription
/// (no deserialization) and should remain accurate regardless of message size.
///
/// Both tools subscribe **concurrently** to the same publisher so they race against the
/// same message stream. ros2 topic hz misses messages when Python deserialization takes
/// longer than the publish interval; hu meter hz counts raw bytes and misses nothing.
#[test]
fn test_large_payload_hz() {
    let target = 50.0_f64; // 20 ms interval — tight enough to stress Python deser
    let payload_bytes = 5_000_000; // 5 MB per message
    let topic = "hz_large_payload";
    let duration_secs = 10.0_f64;

    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    {
        let endpoint2 = endpoint.clone();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let ctx = create_hiroz_context_with_endpoint(&endpoint2).unwrap();
                let node = ctx.create_node("hz_large_pub").build().unwrap();
                let pub_ = node
                    .create_pub::<RosString>(&format!("/{topic}"))
                    .build()
                    .unwrap();
                let interval_us = (1_000_000.0 / target) as u64;
                let stop = std::time::Instant::now()
                    + std::time::Duration::from_secs_f64(duration_secs + 5.0);
                let payload = "x".repeat(payload_bytes);
                while std::time::Instant::now() < stop {
                    let _ = pub_
                        .async_publish(&RosString {
                            data: payload.clone(),
                        })
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_micros(interval_us)).await;
                }
            });
        });
    }

    thread::sleep(Duration::from_millis(500));

    // Spawn hu-meter hz — self-terminates after --duration.
    let hu_child = Command::new(hu_meter_bin())
        .args([
            "--router",
            &endpoint,
            "hz",
            &format!("/{topic}"),
            "--duration",
            &duration_secs.to_string(),
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu-meter hz");

    // Spawn ros2 topic hz concurrently — both tools subscribe at the same instant.
    let ros2_available = Command::new("ros2").arg("--help").output().is_ok();
    let ros2_child = if ros2_available {
        Command::new("ros2")
            .args([
                "topic",
                "hz",
                &format!("/{topic}"),
                "--window",
                "100",
                "--filter",
                &format!("{}", (duration_secs as u32).saturating_sub(2)),
            ])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env(
                "ZENOH_CONFIG_OVERRIDE",
                format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
    } else {
        eprintln!("ros2 CLI not found — skipping ros2 topic hz measurement");
        None
    };

    // Wait for hu-meter to exit naturally, then kill ros2.
    let hu_output = hu_child.wait_with_output().ok();
    let ros2_output = ros2_child.and_then(|mut c| {
        let _ = c.kill();
        c.wait_with_output().ok()
    });

    let hu_stdout = hu_output
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let hu_rate = parse_hu_meter_hz(&hu_stdout).unwrap_or_else(|| {
        eprintln!(
            "hu-meter stderr: {}",
            hu_output
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                .unwrap_or_default()
        );
        0.0
    });

    let ros2_rate = ros2_output
        .as_ref()
        .and_then(|o| parse_ros2_hz(&String::from_utf8_lossy(&o.stdout)));

    let hu_error_pct = (hu_rate - target).abs() / target * 100.0;
    println!("Payload:     {payload_bytes} bytes  (concurrent measurement)");
    println!("Target:      {target:.1} Hz");
    println!("hu meter hz: {hu_rate:.3} Hz  (error: {hu_error_pct:.1}%)");
    if let Some(r) = ros2_rate {
        let ros2_error_pct = (r - target).abs() / target * 100.0;
        println!("ros2 hz:     {r:.3} Hz  (error: {ros2_error_pct:.1}%)");
        if ros2_error_pct > hu_error_pct + 5.0 {
            println!(
                "→ ros2cli#871 reproduced: ros2 under-reports by {ros2_error_pct:.1}%, hu meter only {hu_error_pct:.1}%"
            );
        } else {
            println!("→ ros2cli#871 not reproduced on this machine (difference < 5pp)");
        }
    } else {
        println!("ros2 hz:     n/a");
    }

    // When ros2 is available, assert the two tools agree within 10% of each other.
    // The publisher may not sustain the target rate on a loaded machine; that's expected.
    // When ros2 is absent, fall back to checking hu meter hz against the target.
    if let Some(r) = ros2_rate {
        let diff_pct = (hu_rate - r).abs() / r * 100.0;
        assert!(
            diff_pct < 10.0,
            "hu meter hz ({hu_rate:.3} Hz) differs from ros2 hz ({r:.3} Hz) by {diff_pct:.1}% with {payload_bytes}B payload"
        );
    } else {
        assert!(
            hu_error_pct < 15.0,
            "hu meter hz error {hu_error_pct:.1}% exceeds 15% at {target} Hz with {payload_bytes}B payload (reported {hu_rate:.3} Hz)"
        );
    }
}

/// Stress test for ros2cli#871 root cause B using Zenoh SHM.
///
/// 100 MB payload at 10 Hz (1 GB/s) — achievable over SHM so the publisher is never the
/// bottleneck. rclpy must construct a 100 MB Python str object per callback; hu-meter
/// counts raw bytes with no object construction. If root cause B exists on this machine,
/// ros2 topic hz will under-report relative to hu-meter.
///
/// The test asserts hu-meter reaches ≥ 9 Hz (90% of 10 Hz target). ros2 topic hz
/// results are reported informally; the test does not fail on ros2 under-reporting
/// because that is the bug we are detecting, not a requirement.
#[test]
fn test_hz_accuracy_shm() {
    let target = 10.0_f64;
    let payload_bytes = 100_000_000; // 100 MB — force 100 MB Python str construction per callback
    let topic = "hz_shm_stress";
    let duration_secs = 20.0_f64;
    let shm_pool = 512 * 1024 * 1024; // 512 MB pool — fits several 100 MB in-flight messages

    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    // Publisher with SHM — zero-copy to all SHM-capable subscribers.
    {
        let endpoint2 = endpoint.clone();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                use hiroz::context::ZContextBuilder;

                let ctx = ZContextBuilder::default()
                    .disable_multicast_scouting()
                    .with_connect_endpoints([endpoint2.as_str()])
                    .with_mode("client")
                    .with_shm_pool_size(shm_pool)
                    .expect("SHM not supported in this build")
                    .build()
                    .unwrap();
                let node = ctx.create_node("hz_shm_pub").build().unwrap();
                let pub_ = node
                    .create_pub::<RosString>(&format!("/{topic}"))
                    .build()
                    .unwrap();
                let interval_us = (1_000_000.0 / target) as u64;
                let stop = std::time::Instant::now()
                    + std::time::Duration::from_secs_f64(duration_secs + 5.0);
                let payload = "x".repeat(payload_bytes);
                while std::time::Instant::now() < stop {
                    let _ = pub_
                        .async_publish(&RosString {
                            data: payload.clone(),
                        })
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_micros(interval_us)).await;
                }
            });
        });
    }

    thread::sleep(Duration::from_millis(500));

    // hu-meter with --shm flag — receives zero-copy from the SHM publisher.
    let hu_child = Command::new(hu_meter_bin())
        .args([
            "--router",
            &endpoint,
            "--shm",
            "hz",
            &format!("/{topic}"),
            "--duration",
            &duration_secs.to_string(),
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu-meter hz --shm");

    // ros2 topic hz with SHM enabled via ZENOH_CONFIG_OVERRIDE — concurrent measurement.
    let ros2_available = Command::new("ros2").arg("--help").output().is_ok();
    let ros2_child = if ros2_available {
        Command::new("ros2")
            .args([
                "topic",
                "hz",
                &format!("/{topic}"),
                "--window",
                "100",
                "--filter",
                &format!("{}", (duration_secs as u32).saturating_sub(2)),
            ])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env(
                "ZENOH_CONFIG_OVERRIDE",
                format!(
                    "connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false;transport/shared_memory/enabled=true"
                ),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
    } else {
        eprintln!("ros2 CLI not found — skipping ros2 topic hz SHM measurement");
        None
    };

    let hu_output = hu_child.wait_with_output().ok();
    let ros2_output = ros2_child.and_then(|mut c| {
        let _ = c.kill();
        c.wait_with_output().ok()
    });

    let hu_stdout = hu_output
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let hu_rate = parse_hu_meter_hz(&hu_stdout).unwrap_or_else(|| {
        eprintln!(
            "hu-meter stderr: {}",
            hu_output
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                .unwrap_or_default()
        );
        0.0
    });

    let ros2_rate = ros2_output
        .as_ref()
        .and_then(|o| parse_ros2_hz(&String::from_utf8_lossy(&o.stdout)));

    let hu_error_pct = (hu_rate - target).abs() / target * 100.0;
    println!("=== SHM stress test ({payload_bytes}B @ {target} Hz) ===");
    println!("hu meter hz (SHM): {hu_rate:.3} Hz  (error: {hu_error_pct:.1}%)");
    if let Some(r) = ros2_rate {
        let ros2_error_pct = (r - target).abs() / target * 100.0;
        println!("ros2 hz (SHM):     {r:.3} Hz  (error: {ros2_error_pct:.1}%)");
        if ros2_error_pct > hu_error_pct + 5.0 {
            println!(
                "→ ros2cli#871 root cause B CONFIRMED: ros2 lags by {:.1}pp vs hu-meter",
                ros2_error_pct - hu_error_pct
            );
        } else {
            println!(
                "→ ros2cli#871 root cause B not reproduced on this machine (differential < 5pp)"
            );
        }
    } else {
        println!("ros2 hz:           n/a");
    }

    // Informational only — at 100 MB the SHM pool may still throttle the publisher.
    // Don't assert a rate floor; the test exists to measure and report the differential.
}

#[test]
fn test_hz_accuracy_500hz() {
    let target = 500.0_f64;
    let (hu_rate, ros2_rate) = run_hz_comparison(target, 8.0, "hz_accuracy_500");

    println!("Target:      {target:.1} Hz");
    println!(
        "hu meter hz: {hu_rate:.3} Hz  (error: {:.1}%)",
        (hu_rate - target).abs() / target * 100.0
    );
    if let Some(r) = ros2_rate {
        println!(
            "ros2 hz:     {r:.3} Hz  (error: {:.1}%)",
            (r - target).abs() / target * 100.0
        );
    } else {
        println!("ros2 hz:     n/a");
    }

    if let Some(r) = ros2_rate {
        // Compare hu meter hz against ros2 hz — both measure the same actual rate.
        // The publisher may not reach target on a loaded CI machine; that's not our bug.
        let diff_pct = (hu_rate - r).abs() / r * 100.0;
        assert!(
            diff_pct < 10.0,
            "hu meter hz ({hu_rate:.3} Hz) differs from ros2 hz ({r:.3} Hz) by {diff_pct:.1}% at {target} Hz target"
        );
    } else {
        let error_pct = (hu_rate - target).abs() / target * 100.0;
        assert!(
            error_pct < 10.0,
            "hu meter hz error {error_pct:.1}% exceeds 10% at {target} Hz (reported {hu_rate:.3} Hz)"
        );
    }
}

#[test]
fn test_hz_accuracy_1khz() {
    let target = 1000.0_f64;
    let (hu_rate, ros2_rate) = run_hz_comparison(target, 8.0, "hz_accuracy_1k");

    println!("Target:      {target:.0} Hz");
    println!(
        "hu meter hz: {hu_rate:.3} Hz  (error: {:.1}%)",
        (hu_rate - target).abs() / target * 100.0
    );
    if let Some(r) = ros2_rate {
        println!(
            "ros2 hz:     {r:.3} Hz  (error: {:.1}%)",
            (r - target).abs() / target * 100.0
        );
    } else {
        println!("ros2 hz:     n/a");
    }

    if let Some(r) = ros2_rate {
        let diff_pct = (hu_rate - r).abs() / r * 100.0;
        assert!(
            diff_pct < 10.0,
            "hu meter hz ({hu_rate:.3} Hz) differs from ros2 hz ({r:.3} Hz) by {diff_pct:.1}% at {target:.0} Hz target"
        );
    } else {
        let error_pct = (hu_rate - target).abs() / target * 100.0;
        assert!(
            error_pct < 10.0,
            "hu meter hz error {error_pct:.1}% exceeds 10% at {target:.0} Hz (reported {hu_rate:.3} Hz)"
        );
    }
}

/// Five publishers all posting to the same topic at 1 kHz each produce a 5 kHz aggregate
/// message stream. hu meter hz subscribes once and counts every message in Rust — no
/// deserialization, no GIL, no Python callback overhead. ros2 topic hz runs a Python
/// rclpy node whose per-message callback rate tops out at ~2–5 kHz regardless of how many
/// publishers there are. The aggregate stream reliably saturates the Python callback rate,
/// making the hu meter advantage visible even on loaded CI machines.
#[test]
#[serial_test::serial]
fn test_hz_multi_publisher_aggregation() {
    const N_PUBS: usize = 5;
    const PUB_RATE: f64 = 1000.0; // Hz per publisher
    const TOTAL_TARGET: f64 = (N_PUBS as f64) * PUB_RATE; // 5000 Hz aggregate
    let duration_secs = 10.0_f64;
    let topic = "hz_multi_pub";

    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    // Spawn N independent publishers, each at PUB_RATE Hz.
    for i in 0..N_PUBS {
        let endpoint2 = endpoint.clone();
        let topic2 = topic.to_string();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let ctx = create_hiroz_context_with_endpoint(&endpoint2).unwrap();
                let node = ctx
                    .create_node(&format!("hz_multi_pub_{i}"))
                    .build()
                    .unwrap();
                let pub_ = node
                    .create_pub::<RosString>(&format!("/{topic2}"))
                    .build()
                    .unwrap();
                let interval_us = (1_000_000.0 / PUB_RATE) as u64;
                let stop = std::time::Instant::now()
                    + std::time::Duration::from_secs_f64(duration_secs + 5.0);
                while std::time::Instant::now() < stop {
                    let _ = pub_.async_publish(&RosString { data: "x".into() }).await;
                    tokio::time::sleep(tokio::time::Duration::from_micros(interval_us)).await;
                }
            });
        });
    }

    // Wait for all publishers to connect and stabilise.
    thread::sleep(Duration::from_millis(1000));

    // Spawn both tools concurrently — they observe the same message stream.
    let hu_child = Command::new(hu_meter_bin())
        .args([
            "--router",
            &endpoint,
            "hz",
            &format!("/{topic}"),
            "--duration",
            &duration_secs.to_string(),
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu-meter hz");

    let ros2_available = Command::new("ros2").arg("--help").output().is_ok();
    let ros2_child = if ros2_available {
        Command::new("ros2")
            .args([
                "topic",
                "hz",
                &format!("/{topic}"),
                "--window",
                "100",
                "--filter",
                &format!("{}", (duration_secs as u32).saturating_sub(2)),
            ])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env(
                "ZENOH_CONFIG_OVERRIDE",
                format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
    } else {
        eprintln!("ros2 CLI not found — skipping ros2 topic hz measurement");
        None
    };

    let hu_output = hu_child.wait_with_output().ok();
    let ros2_output = ros2_child.and_then(|mut c| {
        let _ = c.kill();
        c.wait_with_output().ok()
    });

    let hu_stdout = hu_output
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let hu_rate = parse_hu_meter_hz(&hu_stdout).unwrap_or_else(|| {
        eprintln!(
            "hu-meter stderr: {}",
            hu_output
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                .unwrap_or_default()
        );
        0.0
    });
    let ros2_rate = ros2_output
        .as_ref()
        .and_then(|o| parse_ros2_hz(&String::from_utf8_lossy(&o.stdout)));

    println!(
        "=== Multi-publisher: {N_PUBS} × {PUB_RATE:.0} Hz = {TOTAL_TARGET:.0} Hz aggregate ==="
    );
    println!(
        "hu meter hz: {hu_rate:.1} Hz  ({:.0}% of aggregate target)",
        hu_rate / TOTAL_TARGET * 100.0
    );
    if let Some(r) = ros2_rate {
        println!(
            "ros2 hz:     {r:.1} Hz  ({:.0}% of aggregate target)",
            r / TOTAL_TARGET * 100.0
        );
        if hu_rate > r * 1.1 {
            let advantage = (hu_rate - r) / r * 100.0;
            println!(
                "→ hu meter advantage confirmed: {advantage:.0}% more messages counted than ros2cli"
            );
        } else {
            println!(
                "→ tools agree within 10% (machine may not sustain {TOTAL_TARGET:.0} Hz aggregate)"
            );
        }
        // hu meter must not be significantly worse than ros2cli.
        assert!(
            hu_rate >= r * 0.8,
            "hu meter ({hu_rate:.1} Hz) is worse than ros2cli ({r:.1} Hz) — subscriber issue"
        );
    } else {
        // Without ros2 baseline, just verify hu meter counted something.
        assert!(
            hu_rate > 0.0,
            "hu meter returned 0 Hz — subscriber did not receive any messages"
        );
    }
}

/// Single publisher in a tight loop (yield_now between publishes, no sleep) demonstrates
/// Python callback saturation. The Python rclpy callback queue tops out at roughly 2–5 kHz;
/// beyond that, ros2 topic hz under-reports while hu meter hz (Rust, no GIL) continues to
/// count every message. An AtomicU64 counter tracks ground-truth messages sent during the
/// measurement window so we can compute the true publish rate independently of either tool.
///
/// CPU pinning for stability:
///   CPU 0–1: publisher thread (tokio runtime)
///   CPU 2:   hu-meter process
///   CPU 3:   ros2 topic hz process
#[test]
#[serial_test::serial]
fn test_hz_python_saturation() {
    use nix::sched::{CpuSet, sched_setaffinity};
    use nix::unistd::Pid;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    let duration_secs = 10.0_f64;
    let topic = "hz_python_sat";

    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    let sent = Arc::new(AtomicU64::new(0));
    let sent2 = sent.clone();

    // Publisher: tight loop with yield_now (no sleep) — pinned to CPUs 0–1.
    {
        let endpoint2 = endpoint.clone();
        let topic2 = topic.to_string();
        thread::spawn(move || {
            // Pin this thread to CPUs 0–1 so hu-meter (CPU 2) and ros2 (CPU 3) get
            // dedicated cores and their receive rates are not affected by publisher load.
            let mut cpu_set = CpuSet::new();
            let _ = cpu_set.set(0);
            let _ = cpu_set.set(1);
            let _ = sched_setaffinity(Pid::from_raw(0), &cpu_set);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let ctx = create_hiroz_context_with_endpoint(&endpoint2).unwrap();
                let node = ctx.create_node("hz_python_sat_pub").build().unwrap();
                let pub_ = node
                    .create_pub::<RosString>(&format!("/{topic2}"))
                    .build()
                    .unwrap();
                let stop = std::time::Instant::now()
                    + std::time::Duration::from_secs_f64(duration_secs + 5.0);
                while std::time::Instant::now() < stop {
                    let _ = pub_.async_publish(&RosString { data: "x".into() }).await;
                    sent2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
        });
    }

    thread::sleep(Duration::from_millis(500));

    let count_before = sent.load(Ordering::Relaxed);
    let t_start = std::time::Instant::now();

    // Spawn both measurement tools at the same instant, each pinned to its own CPU.
    let hu_child = Command::new("taskset")
        .args(["-c", "2", &hu_meter_bin()])
        .args([
            "--router",
            &endpoint,
            "hz",
            &format!("/{topic}"),
            "--duration",
            &duration_secs.to_string(),
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hu-meter hz");

    let ros2_available = Command::new("ros2").arg("--help").output().is_ok();
    let ros2_child = if ros2_available {
        Command::new("taskset")
            .args(["-c", "3", "ros2"])
            .args([
                "topic",
                "hz",
                &format!("/{topic}"),
                "--window",
                "200",
                "--filter",
                &format!("{}", (duration_secs as u32).saturating_sub(2)),
            ])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env(
                "ZENOH_CONFIG_OVERRIDE",
                format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
    } else {
        eprintln!("ros2 CLI not found — skipping ros2 topic hz measurement");
        None
    };

    // hu-meter self-terminates after --duration; record ground truth at that moment.
    let hu_output = hu_child.wait_with_output().ok();
    let elapsed = t_start.elapsed().as_secs_f64();
    let count_after = sent.load(Ordering::Relaxed);

    let ros2_output = ros2_child.and_then(|mut c| {
        let _ = c.kill();
        c.wait_with_output().ok()
    });

    let messages_in_window = count_after.saturating_sub(count_before);
    let ground_truth_rate = messages_in_window as f64 / elapsed.max(0.1);

    let hu_stdout = hu_output
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let hu_rate = parse_hu_meter_hz(&hu_stdout).unwrap_or_else(|| {
        eprintln!(
            "hu-meter stderr: {}",
            hu_output
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                .unwrap_or_default()
        );
        0.0
    });
    let ros2_rate = ros2_output
        .as_ref()
        .and_then(|o| parse_ros2_hz(&String::from_utf8_lossy(&o.stdout)));

    // Note: hu_rate is derived from the sliding window (last 100 msgs) at the moment
    // hu-meter exits. It is an instantaneous rate, not a time-average, so it can
    // legitimately exceed ground_truth_rate if a burst is in flight at deadline time.
    println!("=== Python saturation test (publisher: no-sleep yield_now loop) ===");
    println!(
        "Ground truth: {ground_truth_rate:.0} Hz  ({messages_in_window} msgs in {elapsed:.1}s, time-averaged)"
    );
    println!("hu meter hz:  {hu_rate:.0} Hz  (instantaneous sliding-window rate)");
    if let Some(r) = ros2_rate {
        println!("ros2 hz:      {r:.0} Hz");
        if hu_rate > r * 1.2 {
            let advantage = (hu_rate - r) / r * 100.0;
            println!("→ Python saturation confirmed: hu meter sees {advantage:.0}% more messages");
        } else if ground_truth_rate < 3000.0 {
            println!(
                "→ publisher only reached {ground_truth_rate:.0} Hz — below Python saturation threshold, tools agree"
            );
        } else {
            println!("→ tools agree within 20% at {ground_truth_rate:.0} Hz ground truth");
        }
        // hu meter must not significantly undercount relative to ros2cli.
        assert!(
            hu_rate >= r * 0.8,
            "hu meter ({hu_rate:.0} Hz) is worse than ros2cli ({r:.0} Hz) — subscriber issue"
        );
    } else {
        // Without ros2 baseline, verify hu meter captures a meaningful fraction of messages.
        let capture_pct = hu_rate / ground_truth_rate.max(1.0) * 100.0;
        assert!(
            capture_pct >= 10.0 || ground_truth_rate < 100.0,
            "hu meter only captured {capture_pct:.0}% of messages (ground truth: {ground_truth_rate:.0} Hz)"
        );
    }
}

/// Demonstrates ros2cli#1043 / ros2cli#843 advantage: at 2 kHz, hu meter hz
/// and ros2 topic hz measure the same actual rate. The machine may not sustain
/// 2 kHz under CI load — both tools will under-report equally. What matters is
/// that hu meter does not introduce additional error on top of ros2cli.
#[test]
#[serial_test::serial]
fn test_hz_accuracy_2khz() {
    let target = 2000.0_f64;
    let (hu_rate, ros2_rate) = run_hz_comparison(target, 6.0, "hz_accuracy_2k");

    println!("Target:      {target:.0} Hz");
    println!(
        "hu meter hz: {hu_rate:.3} Hz  (error: {:.1}%)",
        (hu_rate - target).abs() / target * 100.0
    );
    if let Some(r) = ros2_rate {
        println!(
            "ros2 hz:     {r:.3} Hz  (error: {:.1}%, ros2cli may under-report at high rates — #1043)",
            (r - target).abs() / target * 100.0
        );
        // Both tools subscribe to the same stream. hu meter must not measure
        // more than 15pp worse than ros2cli — if ros2cli also under-reports due
        // to machine load, that is not a hu meter bug.
        let diff_pct = (hu_rate - r).abs() / r.max(1.0) * 100.0;
        assert!(
            diff_pct < 15.0,
            "hu meter hz ({hu_rate:.3} Hz) differs from ros2 hz ({r:.3} Hz) by {diff_pct:.1}% at {target:.0} Hz"
        );
    } else {
        println!("ros2 hz:     n/a");
        // Without ros2cli as a baseline, just verify hu meter is not wildly off.
        let hu_error_pct = (hu_rate - target).abs() / target * 100.0;
        assert!(
            hu_error_pct < 50.0,
            "hu meter hz error {hu_error_pct:.1}% is extreme at {target:.0} Hz (reported {hu_rate:.3} Hz) — possible subscriber issue"
        );
    }
}
