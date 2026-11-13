//! Harness for tests that need an allocation failure. The failure aborts the process, so a test
//! cannot catch it. Each case re-executes the test binary as a child process under a lowered
//! address-space limit (`RLIMIT_AS`) and checks how the child ended.
//!
//! The `hiroz-cdr` and `hiroz-tests` test binaries both include this file as a module.
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
/// Marks the copy of the test below that runs the skip logic.
const OUTER_ENV: &str = "HIROZ_HOSTILE_SKIP_OUTER";
pub const HOSTILE_LENGTH: u32 = 99_999_999;
/// Address space the child may add on top of what the test binary already maps.
const HEADROOM: u64 = 1 << 30;
/// Smallest address-space headroom a child accepts. Below it the child ends with `SKIP_STATUS`.
const MIN_HEADROOM: u64 = 128 << 20;
/// Exit status of a child that cannot lower its address-space limit far enough to test.
const SKIP_STATUS: i32 = 77;

pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// The virtual size of this process, in bytes.
fn vm_size() -> u64 {
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
    // The child aborts on purpose; a core dump would be large and slow.
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
fn skip(why: &str) {
    assert!(
        std::env::var_os(REQUIRE_ENV).is_none(),
        "{REQUIRE_ENV} is set, but {why}"
    );
    eprintln!("SKIPPED: {why}");
}

/// The name `--exact` needs for a test of this module: the path below the crate root.
fn own(test: &str) -> String {
    let (_, module) = module_path!().split_once("::").expect("module path");
    format!("{module}::{test}")
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

/// The child ended because an allocation failed: SIGABRT after the allocator's message.
fn aborted(out: &Output) -> bool {
    out.status.signal() == Some(6)
        && String::from_utf8_lossy(&out.stderr).contains("memory allocation of")
}

/// Positive control: the harness detects the abort. The child reserves the hostile amount
/// directly, with no decoder involved, and must die of SIGABRT.
#[test]
fn harness_detects_an_allocation_abort() {
    /// One element of 32 bytes. 99_999_999 of them need 3.2 GB.
    type Big = [u64; 4];

    if is_child() {
        limit_address_space();
        let v: Vec<Big> = Vec::with_capacity(HOSTILE_LENGTH as usize);
        println!("reserved {}", v.capacity());
        return;
    }
    let out = run_child(&own("harness_detects_an_allocation_abort"));
    if skipped(&out) {
        return;
    }
    assert!(
        aborted(&out),
        "the harness must see the allocation abort\n{}",
        describe(&out)
    );
}

/// A child under an inherited hard limit must lower its limit and not raise it.
#[test]
fn limit_respects_an_inherited_hard_limit() {
    if is_child() {
        limit_address_space();
        return;
    }
    // `ulimit -v` sets the soft and the hard limit, in kB. The cap is below the usual request.
    // It cannot raise a lower hard limit that this process inherited.
    let inherited_kb = vm_size() / 1024 + (256 << 10);
    let (_, hard) = getrlimit(Resource::RLIMIT_AS).expect("getrlimit RLIMIT_AS");
    if hard < inherited_kb * 1024 {
        skip("the inherited address-space hard limit is below the cap this case sets");
        return;
    }
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -v \"$1\" && exec \"$0\" --exact \"$2\" --nocapture --test-threads=1",
            &std::env::current_exe()
                .expect("current_exe")
                .to_string_lossy(),
            &inherited_kb.to_string(),
            &own("limit_respects_an_inherited_hard_limit"),
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn child");
    assert!(
        out.status.success(),
        "the child must work under an inherited hard limit\n{}",
        describe(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let applied: u64 = stdout
        .lines()
        .find_map(|l| l.split_once("address-space limit: "))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no applied limit reported\n{}", describe(&out)));
    assert!(
        applied <= inherited_kb * 1024,
        "applied {applied} above the inherited {}",
        inherited_kb * 1024
    );
}

/// Probe child with too little room. It lowers its hard limit to leave 64 MiB, then asks for the
/// test limit, which ends the process with `SKIP_STATUS`.
#[test]
fn limit_below_min_headroom_probe() {
    if !is_child() {
        return;
    }
    let (_, inherited) = getrlimit(Resource::RLIMIT_AS).expect("getrlimit RLIMIT_AS");
    let hard = (vm_size() + (64 << 20)).min(inherited);
    setrlimit(Resource::RLIMIT_AS, hard, hard).expect("lower the hard limit");
    limit_address_space();
    unreachable!("limit_address_space must end the process below MIN_HEADROOM");
}

/// A case that cannot run is a skip by default and a failure when `REQUIRE_ENV` is set.
#[test]
fn a_skipped_case_fails_only_when_required() {
    let name = own("a_skipped_case_fails_only_when_required");
    if std::env::var_os(OUTER_ENV).is_some() {
        let probe = run_child(&own("limit_below_min_headroom_probe"));
        assert_eq!(
            probe.status.code(),
            Some(SKIP_STATUS),
            "the probe must end as a skip\n{}",
            describe(&probe)
        );
        println!("outer: skipped={}", skipped(&probe));
        return;
    }
    let run_outer = |require: bool| {
        let mut cmd = Command::new(std::env::current_exe().expect("current_exe"));
        cmd.args(["--exact", &name, "--nocapture", "--test-threads=1"])
            .env(OUTER_ENV, "1")
            .env_remove(CHILD_ENV)
            .env_remove(REQUIRE_ENV);
        if require {
            cmd.env(REQUIRE_ENV, "1");
        }
        cmd.output().expect("spawn outer")
    };
    let relaxed = run_outer(false);
    assert!(
        relaxed.status.success(),
        "without {REQUIRE_ENV} a skip is not a failure\n{}",
        describe(&relaxed)
    );
    assert!(
        String::from_utf8_lossy(&relaxed.stdout).contains("outer: skipped=true"),
        "the skip must be reported\n{}",
        describe(&relaxed)
    );
    let required = run_outer(true);
    assert!(
        !required.status.success(),
        "with {REQUIRE_ENV} a skip must fail\n{}",
        describe(&required)
    );
    assert!(
        String::from_utf8_lossy(&required.stderr).contains(REQUIRE_ENV),
        "the failure must name {REQUIRE_ENV}\n{}",
        describe(&required)
    );
}
