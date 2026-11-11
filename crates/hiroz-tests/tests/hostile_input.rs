//! Hostile sequence lengths in the decoders a subscriber uses.
//!
//! A decoder that reserves memory from a length it has just read aborts the
//! process when the reservation fails, and the test cannot catch an abort. Each
//! case re-executes this test binary as a child process under a lowered address
//! space limit (`RLIMIT_AS`) and checks how the child ended. A cgroup memory
//! limit does not trigger the abort: the reservation is not charged until touched.
//!
//! A child that inherits a hard limit leaving under 128 MiB cannot run the case. The test then
//! prints `SKIPPED` on stderr and returns, which cargo counts as a pass. Set
//! `HIROZ_REQUIRE_HOSTILE` to make that a failure; CI sets it.

// The abort needs a reservation above the address space of a 32-bit process, and
// `setrlimit` takes a narrower type there, so the file runs on 64-bit Linux only.
#![cfg(all(target_os = "linux", target_pointer_width = "64", feature = "ros-msgs"))]

use std::process::{Command, Output};

use hiroz::dynamic::{DynamicMessage, DynamicValue, FieldType, MessageSchema};
use nix::sys::resource::{Resource, getrlimit, setrlimit};

const CHILD_ENV: &str = "HIROZ_TESTS_HOSTILE_CHILD";
const HOSTILE_LENGTH: u32 = 99_999_999;
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

/// The child ended because it could not lower its limit, so the case did not run.
///
/// By default this prints `SKIPPED` on stderr and returns `true`; the test then returns, and
/// cargo counts it as a pass. When `HIROZ_REQUIRE_HOSTILE` is set it panics, so a run that must
/// exercise the allocation failure fails when it cannot.
fn skipped(out: &Output) -> bool {
    if out.status.code() != Some(SKIP_STATUS) {
        return false;
    }
    assert!(
        std::env::var_os(REQUIRE_ENV).is_none(),
        "{REQUIRE_ENV} is set, but the inherited address-space hard limit leaves no room to run the case\n{}",
        describe(out)
    );
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
        .find_map(|l| l.split_once("address-space limit: "))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no applied limit reported\n{}", describe(&out)));
    assert!(
        applied <= inherited_kb * 1024,
        "applied {applied} above the inherited {}",
        inherited_kb * 1024
    );
}

/// Environment variable that turns a skipped hostile case into a failure.
const REQUIRE_ENV: &str = "HIROZ_REQUIRE_HOSTILE";
/// Marks the copy of the test below that runs the skip logic.
const OUTER_ENV: &str = "HIROZ_HOSTILE_SKIP_OUTER";

/// Probe child with too little room: it lowers its own hard limit to leave 64 MiB, then asks for
/// the test limit, which ends the process with `SKIP_STATUS`.
#[test]
fn limit_below_min_headroom_probe() {
    if !is_child() {
        return;
    }
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmSize in /proc/self/status");
    let hard = kb * 1024 + (64 << 20);
    setrlimit(Resource::RLIMIT_AS, hard, hard).expect("lower the hard limit");
    limit_address_space();
    unreachable!("limit_address_space must end the process below MIN_HEADROOM");
}

/// A case that cannot run is a skip by default and a failure when `REQUIRE_ENV` is set.
#[test]
fn a_skipped_case_fails_only_when_required() {
    const NAME: &str = "a_skipped_case_fails_only_when_required";
    if std::env::var_os(OUTER_ENV).is_some() {
        let probe = run_child("limit_below_min_headroom_probe");
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

/// The 4-byte CDR_LE encapsulation header and a length prefix, nothing after.
fn hostile_sample() -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&HOSTILE_LENGTH.to_le_bytes());
    v
}

/// A hostile sample followed by one unread byte. A rule that caps only an empty remainder
/// still reserves the declared count for it.
fn truncated_sample() -> Vec<u8> {
    let mut v = hostile_sample();
    v.push(0);
    v
}

/// A valid sample: `n` f64 values behind the header and a length prefix.
fn valid_f64_sample(n: u32) -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&n.to_le_bytes());
    if n > 0 {
        v.extend_from_slice(&[0; 4]); // f64 starts on an 8-byte boundary
    }
    for i in 0..n {
        v.extend_from_slice(&f64::from(i).to_le_bytes());
    }
    v
}

/// The elements of the sequence field `name`.
fn elements(msg: &DynamicMessage, name: &str) -> Vec<DynamicValue> {
    match msg.get_dynamic(name).expect("field") {
        DynamicValue::Array(v) => v,
        other => panic!("expected an array, got {other:?}"),
    }
}

/// The decoded fields of the sample the tests build: one dimension "x" of size 3, data `[1, 2, 3]`.
fn assert_valid_array(back: &hiroz_msgs::std_msgs::UInt8MultiArray) {
    assert_eq!(back.layout.dim.len(), 1, "dimensions");
    assert_eq!(back.layout.dim[0].label, "x", "label");
    assert_eq!(back.layout.dim[0].size, 3, "size");
    assert_eq!(back.layout.dim[0].stride, 3, "stride");
    assert_eq!(back.data, hiroz::ZBuf::from(vec![1u8, 2, 3]), "data");
}

fn f64_sequence_schema() -> std::sync::Arc<MessageSchema> {
    MessageSchema::builder("test_msgs/msg/F64Seq")
        .field("values", FieldType::Sequence(Box::new(FieldType::Float64)))
        .build()
        .expect("schema")
}

/// Run the case in a child; it must exit 0 and print every marker.
fn assert_child_survives(name: &str, markers: &[&str]) {
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
    for m in markers {
        assert!(
            stdout.contains(m),
            "{name}: missing {m:?}\n{}",
            describe(&out)
        );
    }
}

/// Positive control: this file's harness sees an allocation abort. The child reserves the
/// hostile amount directly, with no hiroz code involved, and must die of SIGABRT.
#[test]
fn harness_detects_an_allocation_abort_here() {
    use std::os::unix::process::ExitStatusExt;

    /// One element of 32 bytes. 99_999_999 of them need 3.2 GB.
    type Big = [u64; 4];

    if is_child() {
        limit_address_space();
        let v: Vec<Big> = Vec::with_capacity(HOSTILE_LENGTH as usize);
        println!("reserved {}", v.capacity());
        return;
    }
    let out = run_child("harness_detects_an_allocation_abort_here");
    if skipped(&out) {
        return;
    }
    assert_eq!(
        out.status.signal(),
        Some(6),
        "the harness must see the allocation abort\n{}",
        describe(&out)
    );
}

/// The dynamic decoder must consume elements before growing its output array.
#[test]
fn dynamic_decoder_survives_a_hostile_sequence_length() {
    if is_child() {
        let schema = f64_sequence_schema();
        limit_address_space();
        let err = DynamicMessage::from_cdr(&hostile_sample(), &schema)
            .expect_err("hostile length must be an error");
        println!("hostile: {err}");

        let err = DynamicMessage::from_cdr(&truncated_sample(), &schema)
            .expect_err("truncated sample must be an error");
        println!("truncated: {err}");

        // The same process decodes the next, valid sample.
        let msg = DynamicMessage::from_cdr(&valid_f64_sample(3), &schema).expect("valid sample");
        assert_eq!(
            elements(&msg, "values"),
            [0.0, 1.0, 2.0].map(DynamicValue::Float64),
            "decoded values"
        );
        println!("next valid: {} elements", elements(&msg, "values").len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_sequence_length",
        &["hostile: ", "truncated: ", "next valid: 3 elements"],
    );
}

/// The `BoundedSequence` arm has its own reservation. The decoder does not check the bound,
/// so this test reaches that reservation whatever bound the schema states.
#[test]
fn dynamic_decoder_survives_a_hostile_bounded_sequence_length() {
    if is_child() {
        let schema = MessageSchema::builder("test_msgs/msg/F64BoundedSeq")
            .field(
                "values",
                FieldType::BoundedSequence(Box::new(FieldType::Float64), 200_000_000),
            )
            .build()
            .expect("schema");
        limit_address_space();
        let err = DynamicMessage::from_cdr(&hostile_sample(), &schema)
            .expect_err("hostile length must be an error");
        println!("hostile: {err}");

        let err = DynamicMessage::from_cdr(&truncated_sample(), &schema)
            .expect_err("truncated sample must be an error");
        println!("truncated: {err}");

        // The same process decodes the next, valid sample.
        let msg = DynamicMessage::from_cdr(&valid_f64_sample(3), &schema).expect("valid sample");
        assert_eq!(
            elements(&msg, "values"),
            [0.0, 1.0, 2.0].map(DynamicValue::Float64),
            "decoded values"
        );
        println!("next valid: {} elements", elements(&msg, "values").len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_bounded_sequence_length",
        &["hostile: ", "truncated: ", "next valid: 3 elements"],
    );
}

/// A long sequence that fits its payload still decodes.
#[test]
fn dynamic_decoder_decodes_a_long_valid_sequence() {
    let schema = f64_sequence_schema();
    let msg = DynamicMessage::from_cdr(&valid_f64_sample(1000), &schema).expect("decode");
    let values = elements(&msg, "values");
    assert_eq!(values.len(), 1000);
    assert_eq!(values[999], DynamicValue::Float64(999.0));
}

/// Elements that take no bytes on the wire: a sequence of an empty message needs only its prefix.
#[test]
fn dynamic_decoder_decodes_zero_byte_elements() {
    let empty = MessageSchema::builder("test_msgs/msg/Empty")
        .build()
        .expect("schema");
    let mut sample = vec![0x00, 0x01, 0x00, 0x00];
    sample.extend_from_slice(&3u32.to_le_bytes());
    let element = Box::new(FieldType::Message(empty));
    for (name, ty) in [
        ("seq", FieldType::Sequence(element.clone())),
        ("bounded", FieldType::BoundedSequence(element, 10)),
    ] {
        let schema = MessageSchema::builder("test_msgs/msg/EmptySeq")
            .field("items", ty)
            .build()
            .expect("schema");
        let msg = DynamicMessage::from_cdr(&sample, &schema)
            .unwrap_or_else(|e| panic!("{name}: zero-byte elements must decode: {e}"));
        assert_eq!(elements(&msg, "items").len(), 3, "{name}");
    }
}

fn empty_message() -> std::sync::Arc<MessageSchema> {
    MessageSchema::builder("test_msgs/msg/Empty")
        .build()
        .expect("schema")
}

/// A sample for `Sequence(Sequence(Message(empty)))`: `outer` inner sequences, each declaring
/// the hostile count. Every inner count costs four bytes, and no element has any bytes.
fn nested_hostile_sample(outer: u32) -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&outer.to_le_bytes());
    for _ in 0..outer {
        v.extend_from_slice(&HOSTILE_LENGTH.to_le_bytes());
    }
    v
}

/// Zero-byte elements need no payload, so the declared count is the only limit on the work. The
/// decoder must stop with an error instead of growing its output by the declared count.
#[test]
fn dynamic_decoder_survives_a_hostile_zero_byte_sequence() {
    if is_child() {
        let schema = MessageSchema::builder("test_msgs/msg/EmptySeq")
            .field(
                "items",
                FieldType::Sequence(Box::new(FieldType::Message(empty_message()))),
            )
            .build()
            .expect("schema");
        limit_address_space();
        let err = DynamicMessage::from_cdr(&hostile_sample(), &schema)
            .expect_err("hostile zero-byte count must be an error");
        println!("hostile: {err}");

        let mut valid = vec![0x00, 0x01, 0x00, 0x00];
        valid.extend_from_slice(&3u32.to_le_bytes());
        let msg = DynamicMessage::from_cdr(&valid, &schema).expect("valid sample");
        println!("next valid: {} elements", elements(&msg, "items").len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_zero_byte_sequence",
        &["hostile: ", "next valid: 3 elements"],
    );
}

/// The budget is per decode call. In a sequence of sequences of zero-byte elements, each inner
/// count is below the single-sequence maximum, and only the total is hostile.
#[test]
fn dynamic_decoder_survives_nested_hostile_zero_byte_sequences() {
    if is_child() {
        let inner = FieldType::Sequence(Box::new(FieldType::Message(empty_message())));
        let schema = MessageSchema::builder("test_msgs/msg/NestedEmptySeq")
            .field("items", FieldType::Sequence(Box::new(inner)))
            .build()
            .expect("schema");
        limit_address_space();
        // 250 inner sequences fit in about 1 KB.
        let err = DynamicMessage::from_cdr(&nested_hostile_sample(250), &schema)
            .expect_err("nested hostile counts must be an error");
        println!("hostile: {err}");

        // Honest nesting still decodes: two inner sequences of three elements.
        let mut valid = vec![0x00, 0x01, 0x00, 0x00];
        valid.extend_from_slice(&2u32.to_le_bytes());
        valid.extend_from_slice(&3u32.to_le_bytes());
        valid.extend_from_slice(&3u32.to_le_bytes());
        let msg = DynamicMessage::from_cdr(&valid, &schema).expect("valid sample");
        let outer = elements(&msg, "items");
        println!(
            "next valid: {} outer, {} inner",
            outer.len(),
            outer[0].as_array().expect("inner array").len()
        );
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_nested_hostile_zero_byte_sequences",
        &["hostile: ", "next valid: 2 outer, 3 inner"],
    );
}

/// A sample for `Sequence(Sequence(Message(empty)))` with the given inner counts.
fn nested_sample(inner_counts: &[u32]) -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&(inner_counts.len() as u32).to_le_bytes());
    for count in inner_counts {
        v.extend_from_slice(&count.to_le_bytes());
    }
    v
}

/// The zero-byte budget covers the whole decode call. Each inner sequence below stays under the
/// budget of 1,048,576 on its own, so a decoder that gave every sequence a fresh budget would
/// accept the sample. Only the shared budget rejects the total.
#[test]
fn dynamic_decoder_shares_the_zero_byte_budget_across_sequences() {
    let inner = FieldType::Sequence(Box::new(FieldType::Message(empty_message())));
    let schema = MessageSchema::builder("test_msgs/msg/SharedBudget")
        .field("items", FieldType::Sequence(Box::new(inner)))
        .build()
        .expect("schema");

    // Control: three inner sequences of 300,000 elements (900,000 in all) fit the budget.
    let fits = DynamicMessage::from_cdr(&nested_sample(&[300_000; 3]), &schema)
        .expect("a total below the budget must decode");
    assert_eq!(elements(&fits, "items").len(), 3);

    // Four inner sequences of 300,000 elements (1,200,000 in all) exceed it.
    let err = DynamicMessage::from_cdr(&nested_sample(&[300_000; 4]), &schema)
        .expect_err("a total above the budget must be an error");
    assert!(
        err.to_string().contains("elements without data"),
        "the error must name the budget: {err}"
    );
}

/// A schema of `depth` levels of messages with no bytes. Each level holds two of the level below,
/// so the decoded value has `2^(depth + 1) - 1` messages and the input has no byte for any of them.
fn message_tree(depth: u32) -> std::sync::Arc<MessageSchema> {
    let mut schema = empty_message();
    for level in 0..depth {
        schema = MessageSchema::builder(&format!("test_msgs/msg/Tree{level}"))
            .field("left", FieldType::Message(schema.clone()))
            .field("right", FieldType::Message(schema))
            .build()
            .expect("schema");
    }
    schema
}

/// The number of messages in `msg`, itself included.
fn message_count(msg: &DynamicMessage) -> usize {
    let nested = (0..msg.schema().field_count()).map(|i| match msg.get_dynamic_by_index(i) {
        Ok(DynamicValue::Message(m)) => message_count(m),
        other => panic!("expected a message, got {other:?}"),
    });
    1 + nested.sum::<usize>()
}

/// Messages that hold messages and take no bytes count against the zero-byte budget, as the
/// elements of a sequence do. A tree of 33 million of them must be an error, not an allocation.
#[test]
fn dynamic_decoder_survives_a_hostile_message_tree() {
    if is_child() {
        let tree = message_tree(24);
        // A one-byte element that holds the tree: the element consumes input, its children do not.
        let wrapped = MessageSchema::builder("test_msgs/msg/WrappedTree")
            .field(
                "items",
                FieldType::Sequence(Box::new(FieldType::Message(
                    MessageSchema::builder("test_msgs/msg/Wrapper")
                        .field("tag", FieldType::Uint8)
                        .field("tree", FieldType::Message(tree.clone()))
                        .build()
                        .expect("schema"),
                ))),
            )
            .build()
            .expect("schema");
        let header = [0x00, 0x01, 0x00, 0x00];
        limit_address_space();
        let err = DynamicMessage::from_cdr(&header, &tree)
            .expect_err("a hostile message tree must be an error");
        println!("hostile: {err}");

        let mut wrapped_sample = header.to_vec();
        wrapped_sample.extend_from_slice(&1u32.to_le_bytes());
        wrapped_sample.push(7);
        let err = DynamicMessage::from_cdr(&wrapped_sample, &wrapped)
            .expect_err("a wrapped hostile message tree must be an error");
        println!("wrapped: {err}");

        // The same process decodes an honest tree.
        let msg = DynamicMessage::from_cdr(&header, &message_tree(3)).expect("valid sample");
        println!("next valid: {} messages", message_count(&msg));
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_message_tree",
        &["hostile: ", "wrapped: ", "next valid: 15 messages"],
    );
}

/// A fixed array takes its length from the schema, which a remote node supplies. A huge length
/// must be an error from the decoder, never a reservation.
#[test]
fn dynamic_decoder_survives_a_hostile_array_length() {
    if is_child() {
        let big = MessageSchema::builder("test_msgs/msg/HugeArray")
            .field(
                "flags",
                FieldType::Array(Box::new(FieldType::Bool), 1 << 40),
            )
            .build()
            .expect("schema");
        let zero_byte = MessageSchema::builder("test_msgs/msg/HugeEmptyArray")
            .field(
                "items",
                FieldType::Array(
                    Box::new(FieldType::Message(empty_message())),
                    HOSTILE_LENGTH as usize,
                ),
            )
            .build()
            .expect("schema");
        let header = [0x00, 0x01, 0x00, 0x00];
        limit_address_space();
        let err = DynamicMessage::from_cdr(&header, &big).expect_err("huge array must be an error");
        println!("hostile: {err}");
        let err = DynamicMessage::from_cdr(&header, &zero_byte)
            .expect_err("huge zero-byte array must be an error");
        println!("hostile zero-byte: {err}");

        let small = MessageSchema::builder("test_msgs/msg/SmallArray")
            .field("flags", FieldType::Array(Box::new(FieldType::Bool), 3))
            .build()
            .expect("schema");
        let mut valid = header.to_vec();
        valid.extend_from_slice(&[1, 0, 1]);
        let msg = DynamicMessage::from_cdr(&valid, &small).expect("valid sample");
        println!("next valid: {} elements", elements(&msg, "flags").len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_array_length",
        &["hostile: ", "hostile zero-byte: ", "next valid: 3 elements"],
    );
}

/// A sequence length that equals the bytes left decodes (one byte per element).
#[test]
fn dynamic_decoder_accepts_a_length_at_the_boundary() {
    let schema = MessageSchema::builder("test_msgs/msg/BoolSeq")
        .field("flags", FieldType::Sequence(Box::new(FieldType::Bool)))
        .build()
        .expect("schema");
    let mut sample = vec![0x00, 0x01, 0x00, 0x00];
    sample.extend_from_slice(&4u32.to_le_bytes());
    sample.extend_from_slice(&[1, 0, 1, 1]); // length 4, exactly 4 bytes left
    let msg = DynamicMessage::from_cdr(&sample, &schema).expect("boundary sample");
    assert_eq!(
        elements(&msg, "flags"),
        [true, false, true, true].map(DynamicValue::Bool)
    );
    sample.truncate(sample.len() - 1); // length 4, 3 bytes left
    assert!(DynamicMessage::from_cdr(&sample, &schema).is_err());
}

/// The generated decoder of a message with a `sequence<struct>` field (`UInt8MultiArray`
/// starts with `layout.dim`, a sequence of 32-byte `MultiArrayDimension`) must also
/// return an error. This is the path of a typed subscriber, on any key, because
/// every key reaches the same decode.
#[test]
fn generated_decoder_survives_a_hostile_sequence_length() {
    use hiroz::msg::ZMessage;
    use hiroz_msgs::std_msgs::{MultiArrayDimension, UInt8MultiArray};
    use zenoh_buffers::buffer::Buffer;

    if is_child() {
        // Build the valid sample before the limit applies.
        let mut valid = UInt8MultiArray::default();
        valid.layout.dim = vec![MultiArrayDimension {
            label: "x".into(),
            size: 3,
            stride: 3,
        }];
        valid.data = vec![1u8, 2, 3].into();
        let valid = valid.serialize();
        limit_address_space();

        match UInt8MultiArray::deserialize(&hostile_sample()[..]) {
            Err(err) => println!("hostile: {err}"),
            Ok(_) => panic!("hostile length must be an error"),
        }
        match UInt8MultiArray::deserialize(&truncated_sample()[..]) {
            Err(err) => println!("truncated: {err}"),
            Ok(_) => panic!("truncated sample must be an error"),
        }

        let back = UInt8MultiArray::deserialize(&valid[..]).expect("valid sample");
        assert_valid_array(&back);
        println!(
            "next valid: {} dims, {} bytes",
            back.layout.dim.len(),
            back.data.len()
        );
        return;
    }
    assert_child_survives(
        "generated_decoder_survives_a_hostile_sequence_length",
        &["hostile: ", "truncated: ", "next valid: 1 dims, 3 bytes"],
    );
}

mod common;

/// A typed subscriber must outlive a hostile sample from a raw publisher.
/// The child builds the subscriber and the publisher, limits its address space, publishes
/// the hostile sample and then a valid one. It must return an error for the first and the
/// message for the second.
#[test]
fn typed_subscriber_survives_a_hostile_sample() {
    use std::time::{Duration, Instant};

    use hiroz::{Builder, msg::ZMessage};
    use hiroz_msgs::std_msgs::{MultiArrayDimension, UInt8MultiArray};
    use zenoh::Wait;
    use zenoh_buffers::buffer::Buffer;

    const NAME: &str = "typed_subscriber_survives_a_hostile_sample";

    if is_child() {
        let router = common::TestRouter::new();
        let ctx = common::create_hiroz_context_with_endpoint(router.endpoint()).expect("context");
        let node = ctx.create_node("hostile_sub").build().expect("node");
        let sub = node
            .create_sub::<UInt8MultiArray>("/hostile_sample_topic")
            .build()
            .expect("typed subscriber");

        // A raw session is the publisher; it shares no code with the typed path.
        let key = hiroz::context::KeyExprFormat::default()
            .topic_key_expr(sub.entity())
            .expect("topic key");
        let mut config = zenoh::Config::default();
        config.insert_json5("mode", "\"client\"").expect("mode");
        config
            .insert_json5("connect/endpoints", &format!("[\"{}\"]", router.endpoint()))
            .expect("endpoint");
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .expect("multicast");
        let session = zenoh::open(config).wait().expect("raw session");
        let publisher = session
            .declare_publisher((*key).clone())
            .wait()
            .expect("raw publisher");

        let mut valid = UInt8MultiArray::default();
        valid.layout.dim = vec![MultiArrayDimension {
            label: "x".into(),
            size: 3,
            stride: 3,
        }];
        valid.data = vec![1u8, 2, 3].into();
        let valid = valid.serialize().to_vec();

        // Wait until the subscriber receives a valid sample: the route is up.
        let start = Instant::now();
        loop {
            publisher.put(valid.clone()).wait().expect("put");
            if sub.recv_timeout(Duration::from_millis(200)).is_ok() {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "route never came up"
            );
        }
        println!("route up");

        // Drain the valid samples the wait loop left queued or in flight, so that
        // neither the hostile receive nor the next-valid receive can be satisfied by one.
        // The queue is empty once a receive times out.
        let mut drained = 0;
        loop {
            match sub.recv_timeout(Duration::from_millis(500)) {
                Ok(_) => drained += 1,
                Err(err) if hiroz::error::is_timeout(&*err) => break,
                Err(err) => panic!("drain: unexpected error: {err}"),
            }
        }
        println!("drained: {drained}");

        limit_address_space();
        publisher.put(hostile_sample()).wait().expect("put hostile");
        // The error must come from the decoder. A timeout would also be an `Err`, and a
        // dead delivery path would then pass for a rejected sample.
        match sub.recv_timeout(Duration::from_secs(5)) {
            Err(err) if hiroz::error::is_timeout(&*err) => {
                panic!("hostile sample was not delivered: {err}")
            }
            Err(err) => println!("hostile: {err}"),
            Ok(_) => panic!("hostile length must be an error"),
        }
        publisher.put(valid).wait().expect("put valid");
        let back = sub
            .recv_timeout(Duration::from_secs(5))
            .expect("next valid sample");
        assert_valid_array(&back);
        println!(
            "next valid: {} dims, {} bytes",
            back.layout.dim.len(),
            back.data.len()
        );
        return;
    }
    assert_child_survives(
        NAME,
        &["route up", "hostile: ", "next valid: 1 dims, 3 bytes"],
    );
}
