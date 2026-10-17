//! The allocator pin must not slow a 32 MiB receive on the main thread.
//!
//! The pin is `hiroz::malloc_pin::pin_large_message_thresholds`. The test runs one typed
//! `UInt8MultiArray` receive on the main thread of a helper process, five times unpinned and
//! five times pinned, on one host in one session. It fails when the pinned median of the
//! per-run p50 latency exceeds the unpinned median by more than the larger of the two
//! min-max ranges. A control arm, the unpinned receive slowed by a deliberate delay, must fail
//! the same check.
//!
//! A second check keeps the gain over the previous pin (`M_MMAP_THRESHOLD` 64 MiB, `M_TRIM_THRESHOLD`
//! 64 MiB, the arm `raw:64:64`). The pinned median must be below the old-pin median by more than
//! twice the larger min-max range of those two arms. A second set of the old pin, and the slowed
//! arm, must fail that check.
//!
//! Opt-in: the test is `#[ignore]`d and CI does not run it. Its thresholds come from one quiet
//! host (load average below 0.6). On a shared runner under load the unpinned receive drops
//! messages (47 of 60 samples were seen) and the run says nothing about the pin. Run it on a
//! quiet host with:
//!
//! ```text
//! cargo nextest run -p hiroz-tests --features ros-msgs,jazzy --profile pin-timing \
//!     --run-ignored only -E 'test(=pinned_32mib_main_thread_receive_is_not_slower_than_unpinned)'
//! ```
//!
//! The run takes about 277 s (25 receives of about 11 s each). The test prints the load average and glibc version first.
//! Check the printed load before you read the result. CI still compiles this file, and it runs
//! `operator_threshold_variable_fails_the_probe` here and the wiring test in
//! `malloc_pin_wiring.rs`.
//!
//! Population: direct loopback TCP, no router, no SHM, a typed `UInt8MultiArray` received
//! with `recv_timeout`. The test says nothing about `async_recv`, callbacks or other types.
#![cfg(all(target_os = "linux", target_env = "gnu"))]
#![cfg(feature = "ros-msgs")]

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
/// The pin before this change: `M_MMAP_THRESHOLD` 64 MiB, `M_TRIM_THRESHOLD` 64 MiB.
const OLD_PIN_ARM: &str = "raw:64:64";

/// Longest wait for the output of one receiver. One receive takes about 11 s.
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
    if arm == OLD_PIN_ARM {
        assert_eq!(field("pin="), "raw", "old-pin arm is not pinned: {line}");
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

/// `candidate` is not slower when its median stays within `tolerance` of the unpinned median.
fn not_slower(candidate: &[f64], unpinned: &[f64], tolerance: f64) -> bool {
    median(candidate) <= median(unpinned) + tolerance
}

/// The tolerance of the pinned comparison: the larger range of the two arms.
fn pinned_tolerance(pinned: &[f64], unpinned: &[f64]) -> f64 {
    range(pinned).max(range(unpinned))
}

/// The control detects when the slowed arm fails a check that is at least as loose as the pinned
/// one. The tolerance covers the pinned range too, so a pinned arm too noisy to measure makes the
/// control fail instead of hiding a regression.
fn control_detects(slowed: &[f64], unpinned: &[f64], pinned: &[f64]) -> bool {
    let tolerance = pinned_tolerance(pinned, unpinned).max(range(slowed));
    !not_slower(slowed, unpinned, tolerance)
}

/// `candidate` improves on the old pin when its median is below the old-pin median by more than
/// `margin`.
fn improves(candidate: &[f64], old: &[f64], margin: f64) -> bool {
    median(candidate) < median(old) - margin
}

/// The margin of the improvement check: twice the larger min-max range of the pinned and the
/// old-pin arm. A real gap must exceed the noise of both arms, each side counted once.
fn improvement_margin(pinned: &[f64], old: &[f64]) -> f64 {
    2.0 * range(pinned).max(range(old))
}

#[test]
fn improvement_check_separates_the_two_measured_hosts_from_the_revert() {
    // Synthetic samples with the medians and ranges measured on two hosts, in ms (n=5 per arm).
    // Host 1: glibc 2.41, old pin 32.0 and 256 MiB pin 13.3. Host 2: glibc 2.42, old pin 47.0 and
    // 256 MiB pin 18.6. The revert makes the pinned arm the old pin, so both arms come from one
    // distribution.
    let host1_old = [31.8, 31.9, 32.0, 32.0, 32.3];
    let host1_new = [13.2, 13.3, 13.3, 13.4, 13.4];
    let host2_old = [46.7, 46.8, 47.0, 47.7, 48.6];
    let host2_new = [18.2, 18.4, 18.6, 18.7, 19.8];
    for (old, new) in [(&host1_old, &host1_new), (&host2_old, &host2_new)] {
        let m = improvement_margin(new, old);
        assert!(improves(new, old, m), "the fix must pass: margin {m}");
        // revert: the pinned arm is another sample of the old pin
        let m = improvement_margin(old, old);
        assert!(!improves(old, old, m), "the revert must fail: margin {m}");
    }
    // On host 1 no pin runs at 34.0 ms, slower than the old pin, so the pinned-not-slower check
    // passes the revert there. The improvement check is what fails it.
    let host1_none = [33.2, 33.9, 34.0, 34.1, 35.3];
    assert!(not_slower(
        &host1_old,
        &host1_none,
        pinned_tolerance(&host1_old, &host1_none)
    ));
    // the check also fails for an arm that is slower than the old pin
    assert!(!improves(&[50.0; 5], &host2_old, 3.8));
}

#[test]
fn control_tolerance_includes_the_pinned_range() {
    // Pinned is 15 ms slower in median and its range is 36 ms. A control of 10 ms over unpinned
    // must not validate this gate.
    let unpinned = [40.0; 5];
    let pinned = [19.0, 55.0, 55.0, 55.0, 55.0];
    let slowed = [50.0; 5];
    assert!(!control_detects(&slowed, &unpinned, &pinned));
    let quiet_pinned = [40.0; 5];
    assert!(control_detects(&slowed, &unpinned, &quiet_pinned));
}

#[test]
#[serial]
#[ignore = "timing gate for a quiet host (load below 0.6); run with --profile pin-timing, see the module doc"]
fn pinned_32mib_main_thread_receive_is_not_slower_than_unpinned() {
    println!(
        "HOST load_average={} glibc={:?}",
        load_average(),
        glibc_version()
    );
    let delayed = format!("none+delay{CONTROL_DELAY_MS}");
    // The old pin runs twice. The second set is the A/A control of the improvement check.
    let arms = ["none", "lib", OLD_PIN_ARM, OLD_PIN_ARM, delayed.as_str()];
    let mut samples: Vec<Vec<f64>> = vec![vec![]; arms.len()];
    for rep in 0..REPEATS {
        // Rotate the order, so a drift in host load does not belong to one arm.
        for k in 0..arms.len() {
            let i = (k + rep) % arms.len();
            samples[i].push(run_once(arms[i]));
        }
    }
    let [unpinned, pinned, old, old_b, slowed] =
        <[Vec<f64>; 5]>::try_from(samples).expect("five arms");
    println!(
        "MEDIAN_P50_MS unpinned={:.3} pinned={:.3} old={:.3} old_b={:.3} slowed={:.3} ranges unpinned={:.3} pinned={:.3} old={:.3} old_b={:.3} slowed={:.3} n={REPEATS}",
        median(&unpinned),
        median(&pinned),
        median(&old),
        median(&old_b),
        median(&slowed),
        range(&unpinned),
        range(&pinned),
        range(&old),
        range(&old_b),
        range(&slowed),
    );
    // Control: the check must fire on the unpinned arm slowed on purpose. A control that passes the
    // check means the check cannot detect, so the verdict is UNKNOWN (plain assert, no marker).
    assert!(
        control_detects(&slowed, &unpinned, &pinned),
        "control failed: an unpinned receive slowed by {CONTROL_DELAY_MS} ms passed a check as loose as the pinned one"
    );
    // The improvement check keeps the gain over the previous pin (64/64), whatever no pin does on
    // this host. Its null control: a second set of the old pin must not pass it.
    let margin = improvement_margin(&pinned, &old);
    assert!(
        !improves(&old_b, &old, margin),
        "improvement control failed: a second set of the old pin (median {:.3} ms) beats the first (median {:.3} ms) by more than {margin:.3} ms",
        median(&old_b),
        median(&old)
    );
    assert!(
        !improves(&slowed, &old, margin),
        "improvement control failed: the slowed arm passed the improvement check"
    );
    assert!(
        improves(&pinned, &old, margin),
        "pinned median {:.3} ms does not beat the old-pin median {:.3} ms by more than {margin:.3} ms",
        median(&pinned),
        median(&old)
    );
    assert!(
        not_slower(&pinned, &unpinned, pinned_tolerance(&pinned, &unpinned)),
        "pinned median {:.3} ms exceeds unpinned median {:.3} ms by more than {:.3} ms",
        median(&pinned),
        median(&unpinned),
        pinned_tolerance(&pinned, &unpinned)
    );
}
