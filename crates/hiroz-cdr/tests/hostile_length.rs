//! A sequence length read from the wire must not make the decoder reserve
//! more memory than the payload can fill.
//!
//! An allocation failure aborts the process, so the test cannot catch it. Each
//! case re-executes this test binary as a child process with a lowered address
//! space limit (`RLIMIT_AS`) and checks how the child ended.
//!
//! Environment: the abort needs an address-space limit. A cgroup memory limit
//! does not trigger it, because the reservation is not charged until it is touched.

use std::process::{Command, Output};

use byteorder::LittleEndian;
use hiroz_cdr::{CdrDeserialize, CdrReader, Error, from_bytes};
use nix::sys::resource::{Resource, setrlimit};

const CHILD_ENV: &str = "HIROZ_CDR_HOSTILE_CHILD";
const HOSTILE_LENGTH: u32 = 99_999_999;

/// One element of 32 bytes on the wire and in memory. 99_999_999 of them need 3.2 GB.
type Big = [u64; 4];

/// Address space the child may add on top of what the test binary already maps.
const HEADROOM: u64 = 1 << 30;

fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// Lower RLIMIT_AS to the current virtual size plus `HEADROOM`.
fn limit_address_space() {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmSize in /proc/self/status");
    let limit = kb * 1024 + HEADROOM;
    setrlimit(Resource::RLIMIT_AS, limit, limit).expect("setrlimit RLIMIT_AS");
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
    assert!(
        aborted(&out),
        "the harness must see the allocation abort\n{}",
        describe(&out)
    );
}

// --- typed path -----------------------------------------------------------

/// `Vec<T>::cdr_deserialize`, the path generated messages use for non-plain elements.
#[test]
fn typed_vec_survives_a_hostile_length() {
    if is_child() {
        limit_address_space();
        let hostile = length_only(HOSTILE_LENGTH);
        let mut r = CdrReader::<LittleEndian>::new(&hostile);
        let err = Vec::<Big>::cdr_deserialize(&mut r).expect_err("hostile length must be an error");
        println!("hostile: {err}");

        // The same process decodes the next, valid message.
        let valid = valid_body(3);
        let mut r = CdrReader::<LittleEndian>::new(&valid);
        let v = Vec::<Big>::cdr_deserialize(&mut r).expect("valid message must decode");
        assert_eq!(v.len(), 3);
        println!("next valid: {} elements", v.len());
        return;
    }
    let out = run_child("typed_vec_survives_a_hostile_length");
    assert!(
        out.status.success(),
        "the child must survive a hostile length\n{}",
        describe(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hostile: "), "{}", describe(&out));
    assert!(
        stdout.contains("next valid: 3 elements"),
        "{}",
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
        println!("next valid: {} elements", v.len());
        return;
    }
    let out = run_child("serde_vec_survives_a_hostile_length");
    assert!(
        out.status.success(),
        "the child must survive a hostile length\n{}",
        describe(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("next valid: 3 elements"),
        "{}",
        describe(&out)
    );
}

// --- the boundary, without a child process ---------------------------------

/// Reads `len` as a prefix and returns the result and what is left.
fn read_len(len: u32, payload: usize) -> hiroz_cdr::Result<usize> {
    let mut body = length_only(len);
    body.extend(std::iter::repeat_n(0u8, payload));
    CdrReader::<LittleEndian>::new(&body).read_sequence_length()
}

/// A length that exactly matches the bytes left is valid. One more is not.
#[test]
fn sequence_length_at_the_boundary() {
    assert_eq!(read_len(0, 0).unwrap(), 0, "zero length, empty payload");
    assert_eq!(read_len(0, 10).unwrap(), 0, "zero length, trailing bytes");
    assert_eq!(read_len(8, 8).unwrap(), 8, "length equals remaining bytes");
    assert_eq!(read_len(1, 8).unwrap(), 1, "length below remaining bytes");
    assert!(
        matches!(read_len(9, 8), Err(Error::UnexpectedEof)),
        "length one above remaining bytes"
    );
    assert!(matches!(read_len(1, 0), Err(Error::UnexpectedEof)));
}

/// A long sequence that fits its payload still decodes, for the generic and the serde path.
#[test]
fn long_valid_sequence_still_decodes() {
    let n = 1000u32;
    let body = valid_body(n);

    let mut r = CdrReader::<LittleEndian>::new(&body);
    let v = Vec::<Big>::cdr_deserialize(&mut r).expect("typed decode");
    assert_eq!(v.len(), n as usize);
    assert_eq!(v[999], [3996, 3997, 3998, 3999]);

    let (v, _) = from_bytes::<Vec<Big>, LittleEndian>(&body).expect("serde decode");
    assert_eq!(v.len(), n as usize);
    assert_eq!(v[999], [3996, 3997, 3998, 3999]);
}
