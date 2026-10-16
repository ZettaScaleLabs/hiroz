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

use std::{
    io::Read,
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serial_test::serial;

const PROBE: &str = env!("CARGO_BIN_EXE_malloc_probe");
const SIZE: usize = 32 << 20;
const HZ: u32 = 10;
const COUNT: usize = 60;
const REPEATS: usize = 5;
const CONTROL_DELAY_MS: u64 = 10;

/// Longest wait for the output of one receiver. A run takes about 16 s.
const RECEIVER_LIMIT: Duration = Duration::from_secs(120);

/// A child process that is killed and reaped when the guard drops, also on a panic.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A loopback endpoint on a port that is free now. The port can be taken before the helper
/// binds it. The window is short, and a taken port fails the run instead of skewing it.
fn free_endpoint() -> String {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = l.local_addr().expect("local addr").port();
    format!("tcp/127.0.0.1:{port}")
}

fn glibc_version() -> String {
    Command::new("ldd")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// One run of the helper: p50 latency in ms of the measured messages.
fn run_once(arm: &str) -> f64 {
    let endpoint = free_endpoint();
    let _publisher = Guard(
        Command::new(PROBE)
            .args([
                "pub",
                &SIZE.to_string(),
                &HZ.to_string(),
                &COUNT.to_string(),
            ])
            .env("MALLOC_PROBE_ENDPOINT", &endpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn publisher"),
    );
    let mut receiver = Guard(
        Command::new(PROBE)
            .args(["recv", arm, &COUNT.to_string()])
            .env("MALLOC_PROBE_ENDPOINT", &endpoint)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn receiver"),
    );
    let mut stdout = receiver.0.stdout.take().expect("receiver stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let r = stdout.read_to_string(&mut out).map(|_| out);
        let _ = tx.send(r);
    });
    let out = match rx.recv_timeout(RECEIVER_LIMIT) {
        Ok(r) => r.expect("read receiver stdout"),
        // The guard kills the receiver, which ends the reader thread.
        Err(_) => panic!("receiver for arm {arm} gave no output in {RECEIVER_LIMIT:?}"),
    };
    let status = receiver.0.wait().expect("wait receiver");
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
    // The pinned arm must report that the pin took effect, so two unpinned arms cannot pass.
    if arm == "lib" {
        assert_eq!(field("pin="), "Pinned", "lib arm is not pinned: {line}");
    }
    // A run with a missing sample is UNKNOWN, never a measurement.
    let n = field("n=");
    assert_eq!(
        n,
        format!("{COUNT}/{COUNT}"),
        "arm {arm} got {n} samples: {line}"
    );
    field("p50_ms=").parse().expect("p50")
}

/// An operator threshold in the environment must stop the receiver, not pass the gate.
#[test]
#[serial]
fn operator_threshold_variable_fails_the_probe() {
    for (var, arm) in [
        ("MALLOC_TRIM_THRESHOLD_", "lib"),
        ("MALLOC_MMAP_THRESHOLD_", "lib"),
        ("MALLOC_TRIM_THRESHOLD_", "none"),
    ] {
        let out = Command::new(PROBE)
            .args(["recv", arm, "1"])
            .env("MALLOC_PROBE_ENDPOINT", free_endpoint())
            .env(var, "1048576")
            .output()
            .expect("run receiver");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("PINGATE"),
            "{var} with arm {arm}: status {}, stderr: {stderr}",
            out.status
        );
    }
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
    println!(
        "HOST load_average={} glibc={:?}",
        load_average(),
        glibc_version()
    );
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
    assert!(
        not_slower(&pinned, &unpinned),
        "pinned median {:.3} ms exceeds unpinned median {:.3} ms by more than {:.3} ms",
        median(&pinned),
        median(&unpinned),
        range(&pinned).max(range(&unpinned))
    );
}
