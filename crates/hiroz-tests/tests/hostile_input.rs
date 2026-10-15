//! Hostile sequence lengths in the decoders a subscriber uses.
//!
//! A decoder that reserves memory from a length it has just read aborts the
//! process when the reservation fails, and the test cannot catch an abort. Each
//! case re-executes this test binary as a child process under a lowered address
//! space limit (`RLIMIT_AS`) and checks how the child ended. A cgroup memory
//! limit does not trigger the abort: the reservation is not charged until touched.

use std::process::{Command, Output};

use hiroz::dynamic::{DynamicMessage, DynamicValue, FieldType, MessageSchema};
use nix::sys::resource::{Resource, setrlimit};

const CHILD_ENV: &str = "HIROZ_TESTS_HOSTILE_CHILD";
const HOSTILE_LENGTH: u32 = 99_999_999;
/// Address space the child may add on top of what the test binary already maps.
const HEADROOM: u64 = 1 << 30;

fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

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

/// F19: the 4-byte CDR_LE encapsulation header and a length prefix, nothing after.
fn hostile_sample() -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&HOSTILE_LENGTH.to_le_bytes());
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

fn f64_sequence_schema() -> std::sync::Arc<MessageSchema> {
    MessageSchema::builder("test_msgs/msg/F64Seq")
        .field("values", FieldType::Sequence(Box::new(FieldType::Float64)))
        .build()
        .expect("schema")
}

/// Run the case in a child; it must exit 0 and print every marker.
fn assert_child_survives(name: &str, markers: &[&str]) {
    let out = run_child(name);
    assert!(
        out.status.success(),
        "the child must survive a hostile length\n{}",
        describe(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for m in markers {
        assert!(stdout.contains(m), "missing {m:?}\n{}", describe(&out));
    }
}

/// The dynamic decoder reserves `Vec::with_capacity(len)` for non-byte sequences.
#[test]
fn dynamic_decoder_survives_a_hostile_sequence_length() {
    if is_child() {
        let schema = f64_sequence_schema();
        limit_address_space();
        let err = DynamicMessage::from_cdr(&hostile_sample(), &schema)
            .expect_err("hostile length must be an error");
        println!("hostile: {err}");

        // The same process decodes the next, valid sample.
        let msg = DynamicMessage::from_cdr(&valid_f64_sample(3), &schema).expect("valid sample");
        println!("next valid: {} elements", elements(&msg, "values").len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_sequence_length",
        &["hostile: ", "next valid: 3 elements"],
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
/// return an error. This is the path of a typed subscriber, on the base key and on
/// the group key alike, because both reach the same decode.
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

        let err = UInt8MultiArray::deserialize(&hostile_sample()[..])
            .err()
            .expect("hostile length must be an error");
        println!("hostile: {err}");

        let back = UInt8MultiArray::deserialize(&valid[..]).expect("valid sample");
        println!(
            "next valid: {} dims, {} bytes",
            back.layout.dim.len(),
            back.data.len()
        );
        return;
    }
    assert_child_survives(
        "generated_decoder_survives_a_hostile_sequence_length",
        &["hostile: ", "next valid: 1 dims, 3 bytes"],
    );
}
