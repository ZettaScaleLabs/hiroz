//! Hostile sequence lengths in the decoders a subscriber uses.
//!
//! A decoder that reserves memory from a length it has just read aborts the process when the
//! reservation fails. A test cannot catch an abort. The cases run in a child process under a
//! lowered address-space limit. See the `hostile` module for the harness and its environment.

// The file runs on 64-bit Linux only. A 32-bit process may reserve the hostile amount, and
// `setrlimit` takes a narrower type there.
#![cfg(all(target_os = "linux", target_pointer_width = "64", feature = "ros-msgs"))]

use hiroz::dynamic::{DynamicMessage, DynamicValue, FieldType, MessageSchema};

#[path = "../../hiroz-cdr/tests/support/hostile.rs"]
mod hostile;

use hostile::{HOSTILE_LENGTH, describe, is_child, limit_address_space, run_child, skipped};

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

/// The sample the typed tests build: one dimension "x" of size 3, offset 5 and data `[1, 2, 3]`.
fn valid_array() -> hiroz_msgs::std_msgs::UInt8MultiArray {
    use hiroz_msgs::std_msgs::{MultiArrayDimension, UInt8MultiArray};

    let mut valid = UInt8MultiArray::default();
    valid.layout.dim = vec![MultiArrayDimension {
        label: "x".into(),
        size: 3,
        stride: 3,
    }];
    valid.layout.data_offset = 5;
    valid.data = vec![1u8, 2, 3].into();
    valid
}

/// The decoded fields of `valid_array`.
fn assert_valid_array(back: &hiroz_msgs::std_msgs::UInt8MultiArray) {
    assert_eq!(back.layout.dim.len(), 1, "dimensions");
    assert_eq!(back.layout.dim[0].label, "x", "label");
    assert_eq!(back.layout.dim[0].size, 3, "size");
    assert_eq!(back.layout.dim[0].stride, 3, "stride");
    assert_eq!(back.layout.data_offset, 5, "data offset");
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

/// The dynamic decoder must consume elements before growing its output array. The
/// `BoundedSequence` arm decodes through the same helper as the unbounded one and does not check
/// the bound, so its case reaches the reservation whatever bound the schema states.
#[test]
fn dynamic_decoder_survives_a_hostile_sequence_length() {
    if is_child() {
        let bounded = MessageSchema::builder("test_msgs/msg/F64BoundedSeq")
            .field(
                "values",
                FieldType::BoundedSequence(Box::new(FieldType::Float64), 200_000_000),
            )
            .build()
            .expect("schema");
        let schemas = [("unbounded", f64_sequence_schema()), ("bounded", bounded)];
        limit_address_space();
        for (name, schema) in schemas {
            let err = DynamicMessage::from_cdr(&hostile_sample(), &schema)
                .expect_err("hostile length must be an error");
            println!("hostile {name}: {err}");

            let err = DynamicMessage::from_cdr(&truncated_sample(), &schema)
                .expect_err("truncated sample must be an error");
            println!("truncated {name}: {err}");

            // The same process decodes the next, valid sample.
            let msg =
                DynamicMessage::from_cdr(&valid_f64_sample(3), &schema).expect("valid sample");
            assert_eq!(
                elements(&msg, "values"),
                [0.0, 1.0, 2.0].map(DynamicValue::Float64),
                "{name}: decoded values"
            );
            println!(
                "next valid {name}: {} elements",
                elements(&msg, "values").len()
            );
        }
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_sequence_length",
        &[
            "hostile unbounded: ",
            "truncated unbounded: ",
            "next valid unbounded: 3 elements",
            "hostile bounded: ",
            "truncated bounded: ",
            "next valid bounded: 3 elements",
        ],
    );
}

/// A long sequence that fits its payload still decodes.
#[test]
fn dynamic_decoder_decodes_a_long_valid_sequence() {
    let schema = f64_sequence_schema();
    let msg = DynamicMessage::from_cdr(&valid_f64_sample(1000), &schema).expect("decode");
    let values = elements(&msg, "values");
    let expected: Vec<DynamicValue> = (0..1000)
        .map(|i| DynamicValue::Float64(f64::from(i)))
        .collect();
    assert_eq!(values, expected);
}

/// Elements that take no bytes on the wire: a sequence of them needs only its prefix.
#[test]
fn dynamic_decoder_decodes_zero_byte_elements() {
    let mut sample = vec![0x00, 0x01, 0x00, 0x00];
    sample.extend_from_slice(&3u32.to_le_bytes());
    let element = Box::new(zero_byte_element());
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

/// An element that takes no bytes: a fixed array with no members. An empty message takes one
/// byte, as in ROS 2.
fn zero_byte_element() -> FieldType {
    FieldType::Array(Box::new(FieldType::Uint8), 0)
}

/// A message that takes no bytes: its only field is a [`zero_byte_element`].
fn zero_byte_message() -> std::sync::Arc<MessageSchema> {
    MessageSchema::builder("test_msgs/msg/ZeroByte")
        .field("none", zero_byte_element())
        .build()
        .expect("schema")
}

/// A sample for `Sequence(Sequence(zero-byte element))`: `outer` inner sequences, each declaring
/// the hostile count. Every inner count costs four bytes, and no element has any bytes.
fn nested_hostile_sample(outer: u32) -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&outer.to_le_bytes());
    for _ in 0..outer {
        v.extend_from_slice(&HOSTILE_LENGTH.to_le_bytes());
    }
    v
}

/// Zero-byte elements need no payload, so only the budget bounds a declared count. The decoder
/// must stop with an error.
#[test]
fn dynamic_decoder_survives_a_hostile_zero_byte_sequence() {
    if is_child() {
        let schema = MessageSchema::builder("test_msgs/msg/EmptySeq")
            .field("items", FieldType::Sequence(Box::new(zero_byte_element())))
            .build()
            .expect("schema");
        limit_address_space();
        let err = DynamicMessage::from_cdr(&hostile_sample(), &schema)
            .expect_err("hostile zero-byte count must be an error");
        println!("hostile: {err}");

        let mut valid = vec![0x00, 0x01, 0x00, 0x00];
        valid.extend_from_slice(&3u32.to_le_bytes());
        let msg = DynamicMessage::from_cdr(&valid, &schema).expect("valid sample");
        let items = elements(&msg, "items");
        assert!(
            items.iter().all(|v| matches!(v, DynamicValue::Array(_))),
            "decoded values: {items:?}"
        );
        println!("next valid: {} elements", items.len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_zero_byte_sequence",
        &["hostile: ", "next valid: 3 elements"],
    );
}

/// The budget is per decode call. Hostile counts in nested sequences of zero-byte elements must
/// end in an error.
#[test]
fn dynamic_decoder_survives_nested_hostile_zero_byte_sequences() {
    if is_child() {
        let inner = FieldType::Sequence(Box::new(zero_byte_element()));
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
        let inner: Vec<usize> = outer
            .iter()
            .map(|v| v.as_array().expect("inner array").len())
            .collect();
        println!("next valid: {} outer, inner {inner:?}", outer.len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_nested_hostile_zero_byte_sequences",
        &["hostile: ", "next valid: 2 outer, inner [3, 3]"],
    );
}

/// A sample for `Sequence(Sequence(zero-byte element))` with the given inner counts.
fn nested_sample(inner_counts: &[u32]) -> Vec<u8> {
    let mut v = vec![0x00, 0x01, 0x00, 0x00];
    v.extend_from_slice(&(inner_counts.len() as u32).to_le_bytes());
    for count in inner_counts {
        v.extend_from_slice(&count.to_le_bytes());
    }
    v
}

/// The zero-byte budget covers the whole decode call. Each inner sequence below stays under the
/// budget of 1,048,576. A decoder that gave every sequence a fresh budget would accept the sample.
#[test]
fn dynamic_decoder_shares_the_zero_byte_budget_across_sequences() {
    let inner = FieldType::Sequence(Box::new(zero_byte_element()));
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
        err.to_string().contains("values without data"),
        "the error must name the budget: {err}"
    );
}

/// A schema of `depth` levels of messages with no bytes. Each level holds two of the level below,
/// and the lowest level holds a zero-byte element.
/// The decoded value has `2^(depth + 1) - 1` messages, and the input has no byte for any of them.
fn message_tree(depth: u32) -> std::sync::Arc<MessageSchema> {
    let mut schema = zero_byte_message();
    for level in 0..depth {
        schema = MessageSchema::builder(&format!("test_msgs/msg/Tree{level}"))
            .field("left", FieldType::Message(schema.clone()))
            .field("right", FieldType::Message(schema))
            .build()
            .expect("schema");
    }
    schema
}

/// The budget is 1,048,576 values without data: a sequence of that many zero-byte elements decodes
/// and one more is an error. The child limits memory because the accepted case holds them all.
#[test]
fn dynamic_decoder_pins_the_zero_byte_budget() {
    if is_child() {
        let schema = MessageSchema::builder("test_msgs/msg/EmptySeq")
            .field("items", FieldType::Sequence(Box::new(zero_byte_element())))
            .build()
            .expect("schema");
        let sample = |count: u32| {
            let mut v = vec![0x00, 0x01, 0x00, 0x00];
            v.extend_from_slice(&count.to_le_bytes());
            v
        };
        limit_address_space();
        let msg = DynamicMessage::from_cdr(&sample(1_048_576), &schema)
            .expect("a count at the budget must decode");
        let at_budget = match msg.get_dynamic_by_index(0) {
            Ok(DynamicValue::Array(v)) => v.len(),
            other => panic!("expected an array, got {other:?}"),
        };
        println!("at budget: {at_budget} elements");
        drop(msg);
        let err = DynamicMessage::from_cdr(&sample(1_048_577), &schema)
            .expect_err("a count above the budget must be an error");
        println!("above budget: {err}");
        return;
    }
    assert_child_survives(
        "dynamic_decoder_pins_the_zero_byte_budget",
        &["at budget: 1048576 elements", "above budget: "],
    );
}

/// The number of messages in `msg`, itself included.
fn message_count(msg: &DynamicMessage) -> usize {
    let nested = (0..msg.schema().field_count()).map(|i| match msg.get_dynamic_by_index(i) {
        Ok(DynamicValue::Message(m)) => message_count(m),
        Ok(DynamicValue::Array(_)) => 0,
        other => panic!("expected a message or an array, got {other:?}"),
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
                FieldType::Array(Box::new(zero_byte_element()), HOSTILE_LENGTH as usize),
            )
            .build()
            .expect("schema");
        let header = [0x00, 0x01, 0x00, 0x00];
        limit_address_space();
        let err = DynamicMessage::from_cdr(&header, &big).expect_err("huge array must be an error");
        println!("hostile: {err}");
        let mut one_byte = header.to_vec();
        one_byte.push(0);
        let err = DynamicMessage::from_cdr(&one_byte, &big)
            .expect_err("huge array with one unread byte must be an error");
        println!("hostile with data: {err}");
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
        assert_eq!(
            elements(&msg, "flags"),
            [true, false, true].map(DynamicValue::Bool),
            "decoded values"
        );
        println!("next valid: {} elements", elements(&msg, "flags").len());
        return;
    }
    assert_child_survives(
        "dynamic_decoder_survives_a_hostile_array_length",
        &[
            "hostile: ",
            "hostile with data: ",
            "hostile zero-byte: ",
            "next valid: 3 elements",
        ],
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
/// return an error. A typed subscriber runs this decode.
#[test]
fn generated_decoder_survives_a_hostile_sequence_length() {
    use hiroz::msg::ZMessage;
    use hiroz_msgs::std_msgs::UInt8MultiArray;
    use zenoh_buffers::buffer::Buffer;

    if is_child() {
        // Build the valid sample before the limit applies.
        let valid = valid_array().serialize();
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
