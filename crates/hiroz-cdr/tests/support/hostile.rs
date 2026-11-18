//! Harness for tests that need an allocation failure. The failure aborts the process, so a test
//! cannot catch it. Each case re-executes the test binary as a child process under a lowered
//! address-space limit (`RLIMIT_AS`) and checks how the child ended.
//!
//! The `hiroz-cdr` and `hiroz-tests` test binaries both include this file as a module. The tests of
//! the harness itself are in `hostile_length.rs`, so they run once.
//!
//! The including files run on 64-bit Linux only. A 32-bit process may reserve the hostile amount,
//! and `setrlimit` takes a narrower type there.
//!
//! A cgroup memory limit does not trigger the abort, because the reservation is not charged
//! until it is touched.
//!
//! A child that inherits a hard limit leaving under 128 MiB cannot run the case. The test then
//! prints `SKIPPED` on stderr and returns, which cargo counts as a pass. Set
//! `HIROZ_REQUIRE_HOSTILE` to make that a failure; CI sets it.

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Output};

use nix::sys::resource::{Resource, getrlimit, setrlimit};

pub const CHILD_ENV: &str = "HIROZ_HOSTILE_CHILD";
/// Environment variable that turns a skipped hostile case into a failure.
pub const REQUIRE_ENV: &str = "HIROZ_REQUIRE_HOSTILE";
pub const HOSTILE_LENGTH: u32 = 99_999_999;
/// Address space the child may add on top of what the test binary already maps.
const HEADROOM: u64 = 1 << 30;
/// Smallest address-space headroom a child accepts. Below it the child ends with `SKIP_STATUS`.
const MIN_HEADROOM: u64 = 128 << 20;
/// Exit status of a child that cannot lower its address-space limit far enough to test.
pub const SKIP_STATUS: i32 = 77;

pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// The virtual size of this process, in bytes.
pub fn vm_size() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmSize in /proc/self/status");
    kb * 1024
}

/// Lower RLIMIT_AS to the current virtual size plus `HEADROOM`, and never above the inherited
/// hard limit. A child that has less than `MIN_HEADROOM` ends with `SKIP_STATUS`.
pub fn limit_address_space() {
    let size = vm_size();
    let (_, hard) = getrlimit(Resource::RLIMIT_AS).expect("getrlimit RLIMIT_AS");
    // RLIM_INFINITY is the largest value, so `min` leaves an unlimited hard limit alone.
    let limit = (size + HEADROOM).min(hard);
    if limit < size + MIN_HEADROOM {
        println!("address-space limit unavailable: inherited hard limit {hard}, VmSize {size}");
        std::process::exit(SKIP_STATUS);
    }
    setrlimit(Resource::RLIMIT_AS, limit, limit).expect("setrlimit RLIMIT_AS");
    println!("address-space limit: {limit}");
    // A decoder that regresses aborts in the child; a core dump would be large and slow.
    setrlimit(Resource::RLIMIT_CORE, 0, 0).expect("setrlimit RLIMIT_CORE");
}

/// The child ended because it could not lower its limit, so the case did not run.
///
/// By default this prints `SKIPPED` on stderr and returns `true`; the test then returns, and
/// cargo counts it as a pass. When `HIROZ_REQUIRE_HOSTILE` is set it panics, so a run that must
/// exercise the allocation failure fails when it cannot.
pub fn skipped(out: &Output) -> bool {
    if out.status.code() != Some(SKIP_STATUS) {
        return false;
    }
    skip(&format!(
        "the inherited address-space hard limit leaves no room to test\n{}",
        describe(out)
    ));
    true
}

/// A case that cannot run: print `SKIPPED` on stderr, or panic when `HIROZ_REQUIRE_HOSTILE` is set.
pub fn skip(why: &str) {
    assert!(
        std::env::var_os(REQUIRE_ENV).is_none(),
        "{REQUIRE_ENV} is set, but {why}"
    );
    eprintln!("SKIPPED: {why}");
}

/// Re-run the test `name` of this binary in child mode. The test sets its own limit.
pub fn run_child(name: &str) -> Output {
    Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn child")
}

pub fn describe(out: &Output) -> String {
    format!(
        "status={:?} signal={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        out.status.signal(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
