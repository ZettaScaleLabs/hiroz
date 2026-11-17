//! The allocator pin must not slow a 32 MiB receive on the main thread.
//!
//! The pin is `hiroz::malloc_pin::pin_large_message_thresholds`. The test runs one typed
//! `UInt8MultiArray` receive on the main thread of a helper process, five times unpinned and
//! five times pinned, on one host in one session. It fails when the pinned median of the
//! per-run p50 latency exceeds the unpinned median by more than the larger of the two
//! min-max ranges. A control arm, the unpinned receive slowed by a deliberate delay, must fail
//! the same check.
//!
//! A second check keeps the gain over the earlier pin of 64 MiB for both thresholds (the arm
//! `raw:64:64`). The pinned median must be below the old-pin median by more than twice the
//! larger min-max range of those two arms. A second set of the old pin, and the old pin slowed
//! by a deliberate delay, must fail that check.
//!
//! Opt-in: the test is `#[ignore]`d and CI does not run it. It needs a quiet host (load average
//! below 0.6); under load the receiver drops messages and the run says nothing about the pin.
//! The test prints the load average and the glibc version first. Run it with:
//!
//! ```text
//! cargo nextest run -p hiroz-tests --features ros-msgs,jazzy --profile pin-timing \
//!     --run-ignored only -E 'test(=pinned_32mib_main_thread_receive_is_not_slower_than_unpinned)'
//! ```
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
/// The earlier pin: `M_MMAP_THRESHOLD` 64 MiB and `M_TRIM_THRESHOLD` 64 MiB.
const OLD_PIN_ARM: &str = "raw:64:64";

/// Longest wait for the output of one receiver.
const RECEIVER_LIMIT: Duration = Duration::from_secs(120);

/// A child process that is killed and reaped when the guard drops, also on a panic.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A helper command with the operator threshold variables, `GLIBC_TUNABLES` and the Zenoh
/// configuration overrides removed. The helper exits when a threshold variable is set, and the
/// overrides can redirect the receiver away from the publisher of the test. A test that needs
/// one of them sets it after this call.
fn probe() -> Command {
    let mut c = Command::new(PROBE);
    c.env_remove("MALLOC_TRIM_THRESHOLD_")
        .env_remove("MALLOC_MMAP_THRESHOLD_")
        .env_remove("GLIBC_TUNABLES")
        .env_remove("ZENOH_CONFIG_OVERRIDE")
        .env_remove("ZENOH_SESSION_CONFIG_URI");
    c
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
        probe()
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
        probe()
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
    // The receiver already stopped the run on a lost or repeated id (`IDGATE`). A run with fewer
    // samples than asked for is UNKNOWN, never a measurement.
    let n = field("n=");
    assert_eq!(
        n,
        format!("{COUNT}/{COUNT}"),
        "arm {arm} got {n} samples: {line}"
    );
    field("p50_ms=").parse().expect("p50")
}

/// The one self-test of the probe: a receiver must stop with exit status 5 and the marker
/// `IDGATE` when the publisher skips an id, so a lost message cannot enter the latency sample.
/// The 64 KiB payload keeps the check to a few seconds.
#[test]
#[serial]
fn receiver_fails_when_the_publisher_skips_an_id() {
    const IDS: usize = 20;
    let endpoint = free_endpoint();
    let _publisher = Guard(
        probe()
            .args([
                "pub",
                &(64usize << 10).to_string(),
                "50",
                &IDS.to_string(),
                "skip15",
            ])
            .env("MALLOC_PROBE_ENDPOINT", &endpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn publisher"),
    );
    let out = probe()
        .args(["recv", "none", &IDS.to_string()])
        .env("MALLOC_PROBE_ENDPOINT", &endpoint)
        .output()
        .expect("run receiver");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "stdout: {}, stderr: {stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(stderr.contains("IDGATE"), "stderr: {stderr}");
}

/// The production function, called alone, must leave the allocator unchanged when the operator
/// set a threshold variable, and must act without one.
#[test]
#[serial]
fn production_pin_reports_operator_set_under_each_variable() {
    for var in ["MALLOC_TRIM_THRESHOLD_", "MALLOC_MMAP_THRESHOLD_"] {
        let out = probe()
            .arg("pin")
            .env(var, "1048576")
            .output()
            .expect("run pin");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("PIN outcome=OperatorSet"),
            "{var}: status {}, stdout: {stdout}",
            out.status
        );
    }
    // Control: without a variable the same call acts, so the check above can tell the two apart.
    // A glibc that rejects the mmap threshold reports `Rejected`, which also is not `OperatorSet`.
    let out = probe().arg("pin").output().expect("run pin");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success()
            && (stdout.contains("PIN outcome=Pinned") || stdout.contains("PIN outcome=Rejected")),
        "status {}, stdout: {stdout}",
        out.status
    );
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

/// The negative control of the improvement check: the old pin slowed on purpose must not pass it.
/// It slows the arm that the check compares with. The verdict then does not depend on the
/// distance between the old pin and no pin on the host.
fn slowed_old_pin_is_rejected(old_slowed: &[f64], old: &[f64], margin: f64) -> bool {
    !improves(old_slowed, old, margin)
}

/// The margin of the improvement check: twice the larger min-max range of the pinned and the
/// old-pin arm. A real gap must exceed the noise of both arms, each side counted once.
fn improvement_margin(pinned: &[f64], old: &[f64]) -> f64 {
    2.0 * range(pinned).max(range(old))
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
    let old_delayed = format!("{OLD_PIN_ARM}+delay{CONTROL_DELAY_MS}");
    // The old pin runs twice. The second set is the A/A control of the improvement check. The
    // delayed old pin is its negative control, so the control holds on a host where the old pin
    // is much slower than no pin.
    let arms = [
        "none",
        "lib",
        OLD_PIN_ARM,
        OLD_PIN_ARM,
        delayed.as_str(),
        old_delayed.as_str(),
    ];
    let mut samples: Vec<Vec<f64>> = vec![vec![]; arms.len()];
    for rep in 0..REPEATS {
        // Rotate the order, so a drift in host load does not belong to one arm.
        for k in 0..arms.len() {
            let i = (k + rep) % arms.len();
            samples[i].push(run_once(arms[i]));
        }
    }
    let [unpinned, pinned, old, old_b, slowed, old_slowed] =
        <[Vec<f64>; 6]>::try_from(samples).expect("six arms");
    println!(
        "MEDIAN_P50_MS unpinned={:.3} pinned={:.3} old={:.3} old_b={:.3} slowed={:.3} old_slowed={:.3} ranges unpinned={:.3} pinned={:.3} old={:.3} old_b={:.3} slowed={:.3} old_slowed={:.3} n={REPEATS}",
        median(&unpinned),
        median(&pinned),
        median(&old),
        median(&old_b),
        median(&slowed),
        median(&old_slowed),
        range(&unpinned),
        range(&pinned),
        range(&old),
        range(&old_b),
        range(&slowed),
        range(&old_slowed),
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
        slowed_old_pin_is_rejected(&old_slowed, &old, margin),
        "improvement control failed: the old pin slowed by {CONTROL_DELAY_MS} ms passed the improvement check"
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
