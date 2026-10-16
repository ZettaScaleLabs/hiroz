//! Hostile sequence lengths in the decoders a subscriber uses.
//!
//! A decoder that reserves memory from a length it has just read aborts the
//! process when the reservation fails, and the test cannot catch an abort. Each
//! case re-executes this test binary as a child process under a lowered address
//! space limit (`RLIMIT_AS`) and checks how the child ended. A cgroup memory
//! limit does not trigger the abort: the reservation is not charged until touched.

#![cfg(target_os = "linux")]

use std::process::{Command, Output};

use hiroz::dynamic::{DynamicMessage, DynamicValue, FieldType, MessageSchema};
use nix::sys::resource::{Resource, setrlimit};

/// Failure marker, `NAME: message`: a false condition panics with `R1ASSERT <name>: <message>`.
/// A red run is read only through this text.
macro_rules! r1_assert {
    ($cond:expr, $name:expr, $($msg:tt)+) => {
        if !$cond {
            panic!("R1ASSERT {}: {}", $name, format!($($msg)+));
        }
    };
}

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

/// The 4-byte CDR_LE encapsulation header and a length prefix, nothing after.
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
    r1_assert!(
        out.status.success(),
        name,
        "the child must survive a hostile length\n{}",
        describe(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for m in markers {
        r1_assert!(
            stdout.contains(m),
            name,
            "missing {m:?}\n{}",
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
    assert_eq!(
        out.status.signal(),
        Some(6),
        "the harness must see the allocation abort\n{}",
        describe(&out)
    );
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

        match UInt8MultiArray::deserialize(&hostile_sample()[..]) {
            Err(err) => println!("hostile: {err}"),
            Ok(_) => panic!("hostile length must be an error"),
        }

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

mod common;

/// A typed subscriber on its base key must outlive a hostile sample from a raw publisher.
/// The child builds the subscriber and the publisher, limits its address space, publishes
/// the hostile sample and then a valid one. It must return an error for the first and the
/// message for the second.
#[test]
fn typed_subscriber_survives_a_hostile_sample_on_the_base_key() {
    use std::time::{Duration, Instant};

    use hiroz::{Builder, msg::ZMessage};
    use hiroz_msgs::std_msgs::{MultiArrayDimension, UInt8MultiArray};
    use zenoh::Wait;
    use zenoh_buffers::buffer::Buffer;

    const NAME: &str = "typed_subscriber_survives_a_hostile_sample_on_the_base_key";

    if is_child() {
        let router = common::TestRouter::new();
        let ctx = common::create_hiroz_context_with_endpoint(router.endpoint()).expect("context");
        let node = ctx.create_node("hostile_sub").build().expect("node");
        let sub = node
            .create_sub::<UInt8MultiArray>("/hostile_base_key")
            .build()
            .expect("typed subscriber");

        // A raw session is the publisher; it shares no code with the typed path.
        let key = hiroz_protocol::KeyExprFormat::default()
            .topic_key_expr(sub.entity())
            .expect("base key");
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
