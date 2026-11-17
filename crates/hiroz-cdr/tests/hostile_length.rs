//! A decoder caps the initial capacity of a sequence by the unread input, whatever count the
//! wire declares.
//!
//! The cases run in a child process under a lowered address-space limit. The `hostile` module
//! holds the harness, its environment and the reason for the platform gate. The tests of the
//! harness itself are at the end of this file.

#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

use std::{
    os::unix::process::ExitStatusExt,
    process::{Command, Output},
};

use byteorder::LittleEndian;
use hiroz_cdr::{CdrDeserialize, CdrReader};
use nix::sys::resource::{Resource, getrlimit, setrlimit};

#[path = "support/hostile.rs"]
mod hostile;

use hostile::{
    CHILD_ENV, HOSTILE_LENGTH, REQUIRE_ENV, SKIP_STATUS, describe, is_child, limit_address_space,
    run_child, skip, skipped, vm_size,
};

/// One element of 32 bytes on the wire and in memory. 99_999_999 of them need 3.2 GB.
#[derive(Debug)]
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

/// A long sequence that fits its payload still decodes.
#[test]
fn long_valid_sequence_still_decodes() {
    let n = 1000u32;
    let body = valid_body(n);

    let expected: Vec<[u64; 4]> = (0..u64::from(n))
        .map(|i| [i * 4, i * 4 + 1, i * 4 + 2, i * 4 + 3])
        .collect();

    let mut r = CdrReader::<LittleEndian>::new(&body);
    let v = Vec::<Big>::cdr_deserialize(&mut r).expect("typed decode");
    assert_eq!(v.iter().map(|b| b.0).collect::<Vec<_>>(), expected);
}

// ── The harness itself ────────────────────────────────────────────────────────

/// Marks the copy of `a_skipped_case_fails_only_when_required` that runs the skip logic.
const OUTER_ENV: &str = "HIROZ_HOSTILE_SKIP_OUTER";

/// The child ended because an allocation failed: SIGABRT after the allocator's message.
fn aborted(out: &Output) -> bool {
    out.status.signal() == Some(6)
        && String::from_utf8_lossy(&out.stderr).contains("memory allocation of")
}

/// Positive control: the harness detects the abort. The child reserves the hostile amount
/// directly, with no decoder involved, and must die of SIGABRT.
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

/// A child under an inherited hard limit must lower its limit and not raise it.
#[test]
fn limit_respects_an_inherited_hard_limit() {
    const NAME: &str = "limit_respects_an_inherited_hard_limit";
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
            NAME,
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

/// A case that cannot run is a skip by default and a failure when `REQUIRE_ENV` is set.
///
/// The test runs in three roles. The child lowers its hard limit to leave 64 MiB and asks for
/// the test limit, which ends it with `SKIP_STATUS`. The outer copy runs that child and reports
/// the skip. The top level runs the outer copy with and without `REQUIRE_ENV`.
#[test]
fn a_skipped_case_fails_only_when_required() {
    const NAME: &str = "a_skipped_case_fails_only_when_required";
    if is_child() {
        let (_, inherited) = getrlimit(Resource::RLIMIT_AS).expect("getrlimit RLIMIT_AS");
        let hard = (vm_size() + (64 << 20)).min(inherited);
        setrlimit(Resource::RLIMIT_AS, hard, hard).expect("lower the hard limit");
        limit_address_space();
        unreachable!("limit_address_space must end the process below MIN_HEADROOM");
    }
    if std::env::var_os(OUTER_ENV).is_some() {
        let probe = run_child(NAME);
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
        cmd.args(["--exact", NAME, "--nocapture", "--test-threads=1"])
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
