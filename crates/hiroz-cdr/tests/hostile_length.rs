//! A decoder caps the initial capacity of a sequence by the unread input, whatever count the
//! wire declares. Honest sequences still reserve their full count at once.
//!
//! An allocation failure aborts the process, so the test cannot catch it. Each
//! case re-executes this test binary as a child process with a lowered address
//! space limit (`RLIMIT_AS`) and checks how the child ended.
//!
//! Environment: the abort needs an address-space limit. A cgroup memory limit
//! does not trigger it, because the reservation is not charged until it is touched.

// The abort needs a reservation above the address space of a 32-bit process, and
// `setrlimit` takes a narrower type there, so the file runs on 64-bit Linux only.
#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

use std::process::{Command, Output};

use byteorder::LittleEndian;
use hiroz_cdr::{CdrDeserialize, CdrReader, from_bytes};
use nix::sys::resource::{Resource, getrlimit, setrlimit};

const CHILD_ENV: &str = "HIROZ_CDR_HOSTILE_CHILD";
const HOSTILE_LENGTH: u32 = 99_999_999;

/// One element of 32 bytes on the wire and in memory. 99_999_999 of them need 3.2 GB.
#[derive(Debug, serde::Deserialize)]
struct Big([u64; 4]);

impl CdrDeserialize for Big {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        reader: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(Self([
            reader.read_u64()?,
            reader.read_u64()?,
            reader.read_u64()?,
            reader.read_u64()?,
        ]))
    }
}

/// Address space the child may add on top of what the test binary already maps.
const HEADROOM: u64 = 1 << 30;

fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// Smallest address-space headroom a child accepts. Below it the child ends with `SKIP_STATUS`.
const MIN_HEADROOM: u64 = 128 << 20;
/// Exit status of a child that cannot lower its address-space limit far enough to test.
const SKIP_STATUS: i32 = 77;

/// Lower RLIMIT_AS to the current virtual size plus `HEADROOM`, and never above the inherited
/// hard limit. A child that has less than `MIN_HEADROOM` ends with `SKIP_STATUS`.
fn limit_address_space() {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmSize in /proc/self/status");
    let size = kb * 1024;
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

/// The child ended because it could not lower its limit, so the case did not run. This is
/// reported on stderr and is not a pass.
fn skipped(out: &Output) -> bool {
    if out.status.code() != Some(SKIP_STATUS) {
        return false;
    }
    eprintln!(
        "SKIPPED: the inherited address-space hard limit leaves no room to test\n{}",
        describe(out)
    );
    true
}

/// A child under an inherited hard limit must lower its limit and not raise it.
#[test]
fn limit_respects_an_inherited_hard_limit() {
    if is_child() {
        limit_address_space();
        return;
    }
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmSize in /proc/self/status");
    // `ulimit -v` sets the soft and the hard limit, in kB. The cap is below the usual request.
    let inherited_kb = kb + (256 << 10);
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -v \"$1\" && exec \"$0\" --exact limit_respects_an_inherited_hard_limit \
             --nocapture --test-threads=1",
            &std::env::current_exe()
                .expect("current_exe")
                .to_string_lossy(),
            &inherited_kb.to_string(),
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
        .find_map(|l| l.strip_prefix("address-space limit: "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no applied limit reported\n{}", describe(&out)));
    assert!(
        applied <= inherited_kb * 1024,
        "applied {applied} above the inherited {}",
        inherited_kb * 1024
    );
}

/// Re-run test `name` of this binary as a child with the hostile-length limit.
fn run_child(name: &str) -> Output {
    Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn child")
}

fn describe(out: &Output) -> String {
    use std::os::unix::process::ExitStatusExt;
    format!(
        "status={:?} signal={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        out.status.signal(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The child ended because of an abort (SIGABRT, shell status 134).
fn aborted(out: &Output) -> bool {
    use std::os::unix::process::ExitStatusExt;
    out.status.signal() == Some(6)
}

/// A message body without the encapsulation header: a length prefix and nothing after it.
fn length_only(len: u32) -> Vec<u8> {
    len.to_le_bytes().to_vec()
}

/// A valid body for `Vec<Big>`: `n` elements.
fn valid_body(n: u32) -> Vec<u8> {
    let mut v = length_only(n);
    if n > 0 {
        // `u64` elements start on an 8-byte boundary.
        v.extend_from_slice(&[0; 4]);
    }
    for i in 0..n {
        for w in 0..4u64 {
            v.extend_from_slice(&(u64::from(i) * 4 + w).to_le_bytes());
        }
    }
    v
}

// --- harness control ------------------------------------------------------

/// Positive control: the harness detects the abort. The child reserves the hostile
/// amount directly, with no hiroz-cdr code involved, and must die of SIGABRT.
#[test]
fn harness_detects_an_allocation_abort() {
    if is_child() {
        limit_address_space();
        let v: Vec<Big> = Vec::with_capacity(HOSTILE_LENGTH as usize);
        println!("reserved {}", v.capacity());
        return;
    }
    let out = run_child("harness_detects_an_allocation_abort");
    if skipped(&out) {
        return;
    }
    assert!(
        aborted(&out),
        "the harness must see the allocation abort\n{}",
        describe(&out)
    );
}

// --- typed path -----------------------------------------------------------

/// Generic `Vec<T>::cdr_deserialize` coverage. Generated non-plain sequences use their own loop.
#[test]
fn typed_vec_survives_a_hostile_length() {
    if is_child() {
        limit_address_space();
        let hostile = length_only(HOSTILE_LENGTH);
        let mut r = CdrReader::<LittleEndian>::new(&hostile);
        let err = Vec::<Big>::cdr_deserialize(&mut r).expect_err("hostile length must be an error");
        println!("hostile: {err}");

        // One unread byte: a rule that caps only an empty remainder still reserves the count.
        let mut truncated = length_only(HOSTILE_LENGTH);
        truncated.push(0);
        let mut r = CdrReader::<LittleEndian>::new(&truncated);
        let err = Vec::<Big>::cdr_deserialize(&mut r).expect_err("truncated must be an error");
        println!("truncated: {err}");

        // The same process decodes the next, valid message.
        let valid = valid_body(3);
        let mut r = CdrReader::<LittleEndian>::new(&valid);
        let v = Vec::<Big>::cdr_deserialize(&mut r).expect("valid message must decode");
        assert_eq!(v.len(), 3);
        assert_eq!(v[2].0, [8, 9, 10, 11], "decoded values");
        println!("next valid: {} elements", v.len());
        return;
    }
    let name = "typed_vec_survives_a_hostile_length";
    let out = run_child(name);
    if skipped(&out) {
        return;
    }
    assert!(
        out.status.success(),
        "{name}: the child must survive a hostile length\n{}",
        describe(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hostile: "), "{name}: {}", describe(&out));
    assert!(stdout.contains("truncated: "), "{name}: {}", describe(&out));
    assert!(
        stdout.contains("next valid: 3 elements"),
        "{name}: {}",
        describe(&out)
    );
}

// --- serde path -----------------------------------------------------------

/// The serde deserializer reaches the same length check through `deserialize_seq`.
#[test]
fn serde_vec_survives_a_hostile_length() {
    if is_child() {
        limit_address_space();
        let hostile = length_only(HOSTILE_LENGTH);
        let res = from_bytes::<Vec<Big>, LittleEndian>(&hostile);
        assert!(res.is_err(), "hostile length must be an error");
        println!("hostile: {}", res.unwrap_err());

        let valid = valid_body(3);
        let (v, _) = from_bytes::<Vec<Big>, LittleEndian>(&valid).expect("valid message");
        assert_eq!(v.len(), 3);
        assert_eq!(v[2].0, [8, 9, 10, 11], "decoded values");
        println!("next valid: {} elements", v.len());
        return;
    }
    let name = "serde_vec_survives_a_hostile_length";
    let out = run_child(name);
    if skipped(&out) {
        return;
    }
    assert!(
        out.status.success(),
        "{name}: the child must survive a hostile length\n{}",
        describe(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("next valid: 3 elements"),
        "{name}: {}",
        describe(&out)
    );
}

/// A long sequence that fits its payload still decodes, for the generic and the serde path.
#[test]
fn long_valid_sequence_still_decodes() {
    let n = 1000u32;
    let body = valid_body(n);

    let mut r = CdrReader::<LittleEndian>::new(&body);
    let v = Vec::<Big>::cdr_deserialize(&mut r).expect("typed decode");
    assert_eq!(v.len(), n as usize);
    assert_eq!(v[999].0, [3996, 3997, 3998, 3999]);

    let (v, _) = from_bytes::<Vec<Big>, LittleEndian>(&body).expect("serde decode");
    assert_eq!(v.len(), n as usize);
    assert_eq!(v[999].0, [3996, 3997, 3998, 3999]);
}

/// Unit elements occupy no bytes on the wire, so their sequence needs only a length prefix.
#[test]
fn zero_size_elements_need_no_payload_bytes() {
    let decode = |len: u32, payload: usize| {
        let mut body = length_only(len);
        body.extend(std::iter::repeat_n(0u8, payload));
        from_bytes::<Vec<()>, LittleEndian>(&body).map(|(v, consumed)| (v.len(), consumed))
    };
    // Controls: the decoder handles empty and non-empty sequences. Elements take no bytes, so
    // every decode consumes the four bytes of the length prefix and leaves the payload unread.
    assert_eq!(decode(0, 0).unwrap(), (0, 4), "empty sequence");
    assert_eq!(decode(3, 3).unwrap(), (3, 4), "payload bytes remain unread");
    assert_eq!(decode(3, 0).unwrap(), (3, 4), "zero-byte unit elements");
    assert_eq!(
        decode(3, 2).unwrap(),
        (3, 4),
        "trailing bytes remain unread"
    );
}
