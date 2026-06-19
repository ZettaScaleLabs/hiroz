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

    let error_pct = (hu_rate - target).abs() / target * 100.0;
    assert!(
        error_pct < 10.0,
        "hu meter hz error {error_pct:.1}% exceeds 10% at {target} Hz (reported {hu_rate:.3} Hz)"
    );
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

    let error_pct = (hu_rate - target).abs() / target * 100.0;
    assert!(
        error_pct < 10.0,
        "hu meter hz error {error_pct:.1}% exceeds 10% at {target:.0} Hz (reported {hu_rate:.3} Hz)"
    );
}
