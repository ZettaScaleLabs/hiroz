//! Wire format of empty messages and bounded sequences, as ROS 2 writes and reads them.
//!
//! ROS 2 gives an empty message one `uint8` member, so it takes one byte on the wire. A bounded
//! sequence declares a maximum, and a longer count is an error.

#![cfg(feature = "ros-msgs")]

use hiroz::{
    dynamic::{DynamicMessage, DynamicValue, FieldType, MessageSchema},
    msg::ZMessage,
};
use hiroz_msgs::{
    rcl_interfaces::{IntegerRange, ParameterDescriptor},
    std_msgs::Empty,
};

/// The CDR_LE encapsulation header.
const HEADER: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

fn sample(payload: &[u8]) -> Vec<u8> {
    [&HEADER[..], payload].concat()
}

fn empty_schema() -> std::sync::Arc<MessageSchema> {
    MessageSchema::builder("test_msgs/msg/Empty")
        .build()
        .expect("schema")
}

#[test]
fn typed_empty_message_writes_one_byte() {
    assert_eq!(Empty::default().serialize(), sample(&[0]));
}

#[test]
fn typed_empty_message_reads_one_byte() {
    Empty::deserialize(&sample(&[0])[..]).expect("an empty message is one byte");
    let err = Empty::deserialize(&HEADER[..]).expect_err("no byte is an error");
    println!("header only: {err}");
}

#[test]
fn dynamic_empty_message_writes_one_byte() {
    let schema = empty_schema();
    let msg = DynamicMessage::new(&schema);
    assert_eq!(msg.to_cdr().expect("serialize"), sample(&[0]));
}

#[test]
fn dynamic_empty_message_reads_one_byte() {
    let schema = empty_schema();
    DynamicMessage::from_cdr(&sample(&[0]), &schema).expect("an empty message is one byte");
    DynamicMessage::from_cdr(&HEADER, &schema).expect_err("no byte is an error");
}

/// Each element of a sequence of empty messages takes one byte, and a field after an empty
/// message starts after that byte.
#[test]
fn dynamic_nested_empty_messages_take_one_byte_each() {
    let empty = empty_schema();
    let seq = MessageSchema::builder("test_msgs/msg/EmptySeq")
        .field(
            "items",
            FieldType::Sequence(Box::new(FieldType::Message(empty.clone()))),
        )
        .build()
        .expect("schema");
    let three = sample(&[3, 0, 0, 0, 0, 0, 0]);
    let msg = DynamicMessage::from_cdr(&three, &seq).expect("three elements, three bytes");
    match msg.get_dynamic("items").expect("field") {
        DynamicValue::Array(v) => assert_eq!(v.len(), 3),
        other => panic!("expected an array, got {other:?}"),
    }
    let short = sample(&[3, 0, 0, 0, 0, 0]);
    DynamicMessage::from_cdr(&short, &seq).expect_err("three elements, two bytes");

    let pair = MessageSchema::builder("test_msgs/msg/EmptyThenByte")
        .field("e", FieldType::Message(empty))
        .field("b", FieldType::Uint8)
        .build()
        .expect("schema");
    let msg = DynamicMessage::from_cdr(&sample(&[0, 7]), &pair).expect("valid sample");
    assert_eq!(msg.get_dynamic("b").expect("field"), DynamicValue::Uint8(7));
}

fn descriptor_with(ranges: usize) -> ParameterDescriptor {
    let mut d = ParameterDescriptor::default();
    d.integer_range = vec![IntegerRange::default(); ranges];
    d
}

#[test]
fn typed_bounded_sequence_rejects_a_count_over_the_maximum() {
    let at_max = descriptor_with(1).serialize();
    ParameterDescriptor::deserialize(&at_max[..]).expect("a count at the maximum decodes");
    let over = descriptor_with(2).serialize();
    let err = ParameterDescriptor::deserialize(&over[..])
        .expect_err("a count over the maximum is an error");
    println!("over maximum: {err}");
}

#[test]
fn dynamic_bounded_sequence_rejects_a_count_over_the_maximum() {
    let floats = MessageSchema::builder("test_msgs/msg/F64Bounded")
        .field(
            "values",
            FieldType::BoundedSequence(Box::new(FieldType::Float64), 3),
        )
        .build()
        .expect("schema");
    let f64s = |n: u32| {
        let mut v = n.to_le_bytes().to_vec();
        if n > 0 {
            v.extend_from_slice(&[0; 4]); // f64 starts on an 8-byte boundary
        }
        v.extend(std::iter::repeat(0u8).take(8 * n as usize));
        sample(&v)
    };
    DynamicMessage::from_cdr(&f64s(3), &floats).expect("a count at the maximum decodes");
    DynamicMessage::from_cdr(&f64s(4), &floats).expect_err("a count over the maximum");

    let bytes = MessageSchema::builder("test_msgs/msg/U8Bounded")
        .field(
            "data",
            FieldType::BoundedSequence(Box::new(FieldType::Uint8), 3),
        )
        .build()
        .expect("schema");
    DynamicMessage::from_cdr(&sample(&[3, 0, 0, 0, 1, 2, 3]), &bytes)
        .expect("a byte count at the maximum decodes");
    DynamicMessage::from_cdr(&sample(&[4, 0, 0, 0, 1, 2, 3, 4]), &bytes)
        .expect_err("a byte count over the maximum");
}
