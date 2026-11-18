//! Wire format of bounded sequences, as ROS 2 reads them.
//!
//! A bounded sequence declares a maximum, and a longer count is an error.

#![cfg(feature = "ros-msgs")]

use hiroz::{
    dynamic::{DynamicMessage, FieldType, MessageSchema},
    msg::{SerdeCdrSerdes, ZDeserializer, ZMessage, ZSerializer},
};
use hiroz_msgs::rcl_interfaces::{IntegerRange, ParameterDescriptor};

/// The CDR_LE encapsulation header.
const HEADER: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

fn sample(payload: &[u8]) -> Vec<u8> {
    [&HEADER[..], payload].concat()
}

fn descriptor_with(ranges: usize) -> ParameterDescriptor {
    ParameterDescriptor {
        integer_range: vec![IntegerRange::default(); ranges],
        ..Default::default()
    }
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
        v.extend(std::iter::repeat_n(0u8, 8 * n as usize));
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

#[test]
fn serde_bounded_sequence_rejects_a_count_over_the_maximum() {
    let at_max = SerdeCdrSerdes::<ParameterDescriptor>::serialize(&descriptor_with(1));
    SerdeCdrSerdes::<ParameterDescriptor>::deserialize(&at_max[..])
        .expect("a count at the maximum decodes");
    let over = SerdeCdrSerdes::<ParameterDescriptor>::serialize(&descriptor_with(2));
    SerdeCdrSerdes::<ParameterDescriptor>::deserialize(&over[..])
        .expect_err("a count over the maximum is an error");
}
