//! Tests for CDR serialization of dynamic messages.

use std::sync::Arc;

use crate::dynamic::message::DynamicMessage;
use crate::dynamic::schema::{FieldType, MessageSchema};
use crate::dynamic::serialization::{CDR_HEADER_LE, deserialize_cdr, serialize_cdr};
use crate::dynamic::value::DynamicValue;

fn create_point_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap()
}

fn create_string_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()
        .unwrap()
}

#[test]
fn test_cdr_header() {
    let schema = create_point_schema();
    let msg = DynamicMessage::new(&schema);
    let bytes = serialize_cdr(&msg).unwrap();

    // Check CDR header (little-endian)
    assert_eq!(&bytes[0..4], &CDR_HEADER_LE);
}

#[test]
fn test_cdr_roundtrip_point() {
    let schema = create_point_schema();
    let mut msg = DynamicMessage::new(&schema);
    msg.set("x", 1.0f64).unwrap();
    msg.set("y", 2.0f64).unwrap();
    msg.set("z", 3.0f64).unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    assert_eq!(decoded.get::<f64>("x").unwrap(), 1.0);
    assert_eq!(decoded.get::<f64>("y").unwrap(), 2.0);
    assert_eq!(decoded.get::<f64>("z").unwrap(), 3.0);
}

#[test]
fn test_cdr_roundtrip_string() {
    let schema = create_string_schema();
    let mut msg = DynamicMessage::new(&schema);
    msg.set("data", "Hello, hiroz!").unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    assert_eq!(
        decoded.get::<String>("data").unwrap(),
        "Hello, hiroz!".to_string()
    );
}

#[test]
fn test_cdr_roundtrip_empty_string() {
    let schema = create_string_schema();
    let mut msg = DynamicMessage::new(&schema);
    msg.set("data", "").unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    assert_eq!(decoded.get::<String>("data").unwrap(), "".to_string());
}

#[test]
fn test_cdr_roundtrip_sequence() {
    let schema = MessageSchema::builder("test_msgs/msg/IntArray")
        .field("data", FieldType::Sequence(Box::new(FieldType::Int32)))
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&schema);
    msg.set("data", vec![1i32, 2, 3, 4, 5]).unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    let data = decoded.get_dynamic("data").unwrap();
    if let DynamicValue::Array(arr) = data {
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0], DynamicValue::Int32(1));
        assert_eq!(arr[4], DynamicValue::Int32(5));
    } else {
        panic!("Expected Array");
    }
}

#[test]
fn test_cdr_roundtrip_empty_sequence() {
    let schema = MessageSchema::builder("test_msgs/msg/IntArray")
        .field("data", FieldType::Sequence(Box::new(FieldType::Int32)))
        .build()
        .unwrap();

    let msg = DynamicMessage::new(&schema);

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    let data = decoded.get_dynamic("data").unwrap();
    if let DynamicValue::Array(arr) = data {
        assert_eq!(arr.len(), 0);
    } else {
        panic!("Expected Array");
    }
}

#[test]
fn test_cdr_roundtrip_fixed_array() {
    let schema = MessageSchema::builder("test_msgs/msg/FixedArray")
        .field("data", FieldType::Array(Box::new(FieldType::Float64), 3))
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&schema);
    msg.set_dynamic(
        "data",
        DynamicValue::Array(vec![
            DynamicValue::Float64(1.0),
            DynamicValue::Float64(2.0),
            DynamicValue::Float64(3.0),
        ]),
    )
    .unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    let data = decoded.get_dynamic("data").unwrap();
    if let DynamicValue::Array(arr) = data {
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], DynamicValue::Float64(1.0));
        assert_eq!(arr[1], DynamicValue::Float64(2.0));
        assert_eq!(arr[2], DynamicValue::Float64(3.0));
    } else {
        panic!("Expected Array");
    }
}

#[test]
fn test_cdr_roundtrip_nested_message() {
    let vector3 = MessageSchema::builder("geometry_msgs/msg/Vector3")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap();

    let twist = MessageSchema::builder("geometry_msgs/msg/Twist")
        .field("linear", FieldType::Message(vector3.clone()))
        .field("angular", FieldType::Message(vector3))
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&twist);
    msg.set("linear.x", 1.0f64).unwrap();
    msg.set("linear.y", 2.0f64).unwrap();
    msg.set("linear.z", 3.0f64).unwrap();
    msg.set("angular.x", 0.1f64).unwrap();
    msg.set("angular.y", 0.2f64).unwrap();
    msg.set("angular.z", 0.5f64).unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &twist).unwrap();

    assert_eq!(decoded.get::<f64>("linear.x").unwrap(), 1.0);
    assert_eq!(decoded.get::<f64>("linear.y").unwrap(), 2.0);
    assert_eq!(decoded.get::<f64>("linear.z").unwrap(), 3.0);
    assert_eq!(decoded.get::<f64>("angular.x").unwrap(), 0.1);
    assert_eq!(decoded.get::<f64>("angular.y").unwrap(), 0.2);
    assert_eq!(decoded.get::<f64>("angular.z").unwrap(), 0.5);
}

#[test]
fn test_cdr_roundtrip_all_primitives() {
    let schema = MessageSchema::builder("test_msgs/msg/AllPrimitives")
        .field("bool_field", FieldType::Bool)
        .field("int8_field", FieldType::Int8)
        .field("int16_field", FieldType::Int16)
        .field("int32_field", FieldType::Int32)
        .field("int64_field", FieldType::Int64)
        .field("uint8_field", FieldType::Uint8)
        .field("uint16_field", FieldType::Uint16)
        .field("uint32_field", FieldType::Uint32)
        .field("uint64_field", FieldType::Uint64)
        .field("float32_field", FieldType::Float32)
        .field("float64_field", FieldType::Float64)
        .field("string_field", FieldType::String)
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&schema);
    msg.set("bool_field", true).unwrap();
    msg.set("int8_field", -42i8).unwrap();
    msg.set("int16_field", -1000i16).unwrap();
    msg.set("int32_field", -100000i32).unwrap();
    msg.set("int64_field", -10000000000i64).unwrap();
    msg.set("uint8_field", 200u8).unwrap();
    msg.set("uint16_field", 50000u16).unwrap();
    msg.set("uint32_field", 3000000000u32).unwrap();
    msg.set("uint64_field", 10000000000u64).unwrap();
    msg.set("float32_field", 1.5f32).unwrap();
    msg.set("float64_field", 2.5f64).unwrap();
    msg.set("string_field", "test string").unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    assert!(decoded.get::<bool>("bool_field").unwrap());
    assert_eq!(decoded.get::<i8>("int8_field").unwrap(), -42);
    assert_eq!(decoded.get::<i16>("int16_field").unwrap(), -1000);
    assert_eq!(decoded.get::<i32>("int32_field").unwrap(), -100000);
    assert_eq!(decoded.get::<i64>("int64_field").unwrap(), -10000000000);
    assert_eq!(decoded.get::<u8>("uint8_field").unwrap(), 200);
    assert_eq!(decoded.get::<u16>("uint16_field").unwrap(), 50000);
    assert_eq!(decoded.get::<u32>("uint32_field").unwrap(), 3000000000);
    assert_eq!(decoded.get::<u64>("uint64_field").unwrap(), 10000000000);
    assert!((decoded.get::<f32>("float32_field").unwrap() - 1.5).abs() < 0.001);
    assert!((decoded.get::<f64>("float64_field").unwrap() - 2.5).abs() < 0.0000001);
    assert_eq!(
        decoded.get::<String>("string_field").unwrap(),
        "test string"
    );
}

#[test]
fn test_cdr_byte_array_optimization() {
    let schema = MessageSchema::builder("test_msgs/msg/ByteArray")
        .field("data", FieldType::Sequence(Box::new(FieldType::Uint8)))
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&schema);
    let bytes_data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    msg.set("data", bytes_data.clone()).unwrap();

    let bytes = serialize_cdr(&msg).unwrap();
    let decoded = deserialize_cdr(&bytes, &schema).unwrap();

    let data = decoded.get_dynamic("data").unwrap();
    if let DynamicValue::Bytes(arr) = data {
        assert_eq!(arr, bytes_data);
    } else {
        panic!("Expected Bytes");
    }
}

#[test]
fn test_message_convenience_methods() {
    let schema = create_point_schema();
    let mut msg = DynamicMessage::new(&schema);
    msg.set("x", 1.0f64).unwrap();
    msg.set("y", 2.0f64).unwrap();
    msg.set("z", 3.0f64).unwrap();

    // Test to_cdr() convenience method
    let bytes = msg.to_cdr().unwrap();

    // Test from_cdr() convenience method
    let decoded = DynamicMessage::from_cdr(&bytes, &schema).unwrap();

    assert_eq!(decoded.get::<f64>("x").unwrap(), 1.0);
    assert_eq!(decoded.get::<f64>("y").unwrap(), 2.0);
    assert_eq!(decoded.get::<f64>("z").unwrap(), 3.0);
}

#[test]
fn test_zbuf_serialization() {
    let schema = create_point_schema();
    let mut msg = DynamicMessage::new(&schema);
    msg.set("x", 1.0f64).unwrap();
    msg.set("y", 2.0f64).unwrap();
    msg.set("z", 3.0f64).unwrap();

    // Test to_cdr_zbuf() convenience method
    let zbuf = msg.to_cdr_zbuf().unwrap();

    // Convert ZBuf to bytes for verification
    use zenoh_buffers::buffer::SplitBuffer;
    let bytes = zbuf.contiguous();

    // Check CDR header
    assert_eq!(&bytes[0..4], &CDR_HEADER_LE);
}
