//! The allocator pin must not slow a 32 MiB receive on the main thread.
//!
//! The pin is `hiroz::malloc_pin::pin_large_message_thresholds`. The test runs one typed
//! `UInt8MultiArray` receive on the main thread of a helper process, five times unpinned and
//! five times pinned, on one host in one session. It fails when the pinned median of the
//! per-run p50 latency exceeds the unpinned median by more than the larger of the two
//! min-max ranges. A control arm, the unpinned receive slowed by a deliberate delay, must fail
//! the same check.
//!
//! Population: direct loopback TCP, no router, no SHM, a typed `UInt8MultiArray` received
//! with `recv_timeout`. The test says nothing about `async_recv`, callbacks or other types.
#![cfg(all(target_os = "linux", feature = "ros-msgs"))]

mod common;

use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serial_test::serial;

const PROBE: &str = env!("CARGO_BIN_EXE_malloc_probe");
const SIZE: usize = 32 << 20;
const HZ: u32 = 10;
const COUNT: usize = 60;
const REPEATS: usize = 5;
const CONTROL_DELAY_MS: u64 = 10;

/// One run of the helper: p50 latency in ms of the measured messages.
fn run_once(arm: &str) -> f64 {
    let mut publisher = Command::new(PROBE)
        .args([
            "pub",
            &SIZE.to_string(),
            &HZ.to_string(),
            &COUNT.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn publisher");
    let mut receiver = Command::new(PROBE)
        .args(["recv", arm, &COUNT.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn receiver");
    let mut out = String::new();
    receiver
        .stdout
        .take()
        .expect("receiver stdout")
        .read_to_string(&mut out)
        .expect("read receiver stdout");
    let status = receiver.wait().expect("wait receiver");
    let t0 = Instant::now();
    while publisher.try_wait().expect("poll publisher").is_none() {
        if t0.elapsed() > Duration::from_secs(60) {
            let _ = publisher.kill();
            let _ = publisher.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        status.success(),
        "receiver for arm {arm} failed: {status}; stdout: {out}"
    );
    let line = out
        .lines()
        .find(|l| l.starts_with("RESULT "))
        .unwrap_or_else(|| panic!("no RESULT line for arm {arm}: {out}"));
    println!("{line}");
    let field = |k: &str| -> &str {
        line.split_whitespace()
            .find_map(|w| w.strip_prefix(k))
            .unwrap_or_else(|| panic!("no {k} in {line}"))
    };
    // A run with a missing sample is UNKNOWN, never a measurement.
    let n = field("n=");
    assert_eq!(
        n,
        format!("{COUNT}/{COUNT}"),
        "arm {arm} got {n} samples: {line}"
    );
    field("p50_ms=").parse().expect("p50")
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    s[s.len() / 2]
}

fn range(v: &[f64]) -> f64 {
    let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    hi - lo
}

/// The pinned arm is not slower when its median stays within the larger range of the unpinned median.
fn not_slower(pinned: &[f64], unpinned: &[f64]) -> bool {
    median(pinned) <= median(unpinned) + range(pinned).max(range(unpinned))
}

#[test]
#[serial]
fn pinned_32mib_main_thread_receive_is_not_slower_than_unpinned() {
    let delayed = format!("none+delay{CONTROL_DELAY_MS}");
    let (mut unpinned, mut pinned, mut slowed) = (vec![], vec![], vec![]);
    for rep in 0..REPEATS {
        // Alternate the order, so a drift in host load does not belong to one arm.
        if rep % 2 == 0 {
            unpinned.push(run_once("none"));
            pinned.push(run_once("lib"));
        } else {
            pinned.push(run_once("lib"));
            unpinned.push(run_once("none"));
        }
        slowed.push(run_once(&delayed));
    }
    println!(
        "MEDIAN_P50_MS unpinned={:.3} pinned={:.3} slowed={:.3} ranges unpinned={:.3} pinned={:.3} slowed={:.3} n={REPEATS}",
        median(&unpinned),
        median(&pinned),
        median(&slowed),
        range(&unpinned),
        range(&pinned),
        range(&slowed),
    );
    // Control: the check must fire on the unpinned arm slowed on purpose. A control that passes the
    // check means the check cannot detect, so the verdict is UNKNOWN (plain assert, no marker).
    assert!(
        !not_slower(&slowed, &unpinned),
        "control failed: an unpinned receive slowed by {CONTROL_DELAY_MS} ms passed the check"
    );
    r1_assert!(
        not_slower(&pinned, &unpinned),
        "pinned_32mib_main_thread_receive_is_not_slower_than_unpinned",
        "pinned median {:.3} ms exceeds unpinned median {:.3} ms by more than {:.3} ms",
        median(&pinned),
        median(&unpinned),
        range(&pinned).max(range(&unpinned))
    );
}
