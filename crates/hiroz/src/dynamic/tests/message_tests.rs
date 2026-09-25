//! Tests for DynamicMessage.

use std::sync::Arc;

use crate::dynamic::error::DynamicError;
use crate::dynamic::message::DynamicMessage;
use crate::dynamic::schema::{FieldType, MessageSchema};
use crate::dynamic::value::DynamicValue;

fn create_point_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap()
}

fn create_vector3_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("geometry_msgs/msg/Vector3")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap()
}

fn create_twist_schema() -> Arc<MessageSchema> {
    let vector3 = create_vector3_schema();
    MessageSchema::builder("geometry_msgs/msg/Twist")
        .field("linear", FieldType::Message(vector3.clone()))
        .field("angular", FieldType::Message(vector3))
        .build()
        .unwrap()
}

#[test]
fn test_new_message_with_defaults() {
    let schema = create_point_schema();
    let msg = DynamicMessage::new(&schema);

    assert_eq!(msg.field_count(), 3);
    assert_eq!(msg.get::<f64>("x").unwrap(), 0.0);
    assert_eq!(msg.get::<f64>("y").unwrap(), 0.0);
    assert_eq!(msg.get::<f64>("z").unwrap(), 0.0);
}

#[test]
fn test_set_and_get() {
    let schema = create_point_schema();
    let mut msg = DynamicMessage::new(&schema);

    msg.set("x", 1.0f64).unwrap();
    msg.set("y", 2.0f64).unwrap();
    msg.set("z", 3.0f64).unwrap();

    assert_eq!(msg.get::<f64>("x").unwrap(), 1.0);
    assert_eq!(msg.get::<f64>("y").unwrap(), 2.0);
    assert_eq!(msg.get::<f64>("z").unwrap(), 3.0);
}

#[test]
fn test_nested_field_access() {
    let schema = create_twist_schema();
    let mut msg = DynamicMessage::new(&schema);

    // Set nested fields using dot notation
    msg.set("linear.x", 1.0f64).unwrap();
    msg.set("linear.y", 2.0f64).unwrap();
    msg.set("linear.z", 3.0f64).unwrap();
    msg.set("angular.x", 0.1f64).unwrap();
    msg.set("angular.y", 0.2f64).unwrap();
    msg.set("angular.z", 0.5f64).unwrap();

    // Get nested fields
    assert_eq!(msg.get::<f64>("linear.x").unwrap(), 1.0);
    assert_eq!(msg.get::<f64>("linear.y").unwrap(), 2.0);
    assert_eq!(msg.get::<f64>("linear.z").unwrap(), 3.0);
    assert_eq!(msg.get::<f64>("angular.x").unwrap(), 0.1);
    assert_eq!(msg.get::<f64>("angular.y").unwrap(), 0.2);
    assert_eq!(msg.get::<f64>("angular.z").unwrap(), 0.5);
}

#[test]
fn test_builder_pattern() {
    let schema = create_point_schema();
    let msg = DynamicMessage::builder(&schema)
        .set("x", 1.0f64)
        .unwrap()
        .set("y", 2.0f64)
        .unwrap()
        .build();

    assert_eq!(msg.get::<f64>("x").unwrap(), 1.0);
    assert_eq!(msg.get::<f64>("y").unwrap(), 2.0);
    assert_eq!(msg.get::<f64>("z").unwrap(), 0.0); // Default
}

#[test]
fn test_by_index_access() {
    let schema = create_point_schema();
    let mut msg = DynamicMessage::new(&schema);

    msg.set_by_index(0, 1.0f64).unwrap();
    msg.set_by_index(1, 2.0f64).unwrap();
    msg.set_by_index(2, 3.0f64).unwrap();

    assert_eq!(msg.get_by_index::<f64>(0).unwrap(), 1.0);
    assert_eq!(msg.get_by_index::<f64>(1).unwrap(), 2.0);
    assert_eq!(msg.get_by_index::<f64>(2).unwrap(), 3.0);
}

#[test]
fn test_field_not_found() {
    let schema = create_point_schema();
    let msg = DynamicMessage::new(&schema);

    let result = msg.get::<f64>("nonexistent");
    assert!(matches!(result, Err(DynamicError::FieldNotFound(_))));
}

#[test]
fn test_type_mismatch() {
    let schema = create_point_schema();
    let msg = DynamicMessage::new(&schema);

    // x is f64, trying to get as String
    let result = msg.get::<String>("x");
    assert!(matches!(result, Err(DynamicError::TypeMismatch { .. })));
}

#[test]
fn test_index_out_of_bounds() {
    let schema = create_point_schema();
    let msg = DynamicMessage::new(&schema);

    let result = msg.get_by_index::<f64>(10);
    assert!(matches!(result, Err(DynamicError::IndexOutOfBounds(_))));
}

#[test]
fn test_iter() {
    let schema = create_point_schema();
    let mut msg = DynamicMessage::new(&schema);
    msg.set("x", 1.0f64).unwrap();
    msg.set("y", 2.0f64).unwrap();
    msg.set("z", 3.0f64).unwrap();

    let fields: Vec<(&str, &DynamicValue)> = msg.iter().collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0].0, "x");
    assert_eq!(fields[1].0, "y");
    assert_eq!(fields[2].0, "z");
}

#[test]
fn test_message_with_string_field() {
    let schema = MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&schema);
    msg.set("data", "Hello, hiroz!").unwrap();

    assert_eq!(msg.get::<String>("data").unwrap(), "Hello, hiroz!");
}

#[test]
fn test_message_with_array_field() {
    let schema = MessageSchema::builder("test_msgs/msg/IntArray")
        .field("data", FieldType::Sequence(Box::new(FieldType::Int32)))
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&schema);
    msg.set("data", vec![1i32, 2, 3, 4, 5]).unwrap();

    let data = msg.get_dynamic("data").unwrap();
    if let DynamicValue::Array(arr) = data {
        assert_eq!(arr.len(), 5);
    } else {
        panic!("Expected Array");
    }
}

#[test]
fn test_message_with_fixed_array() {
    let schema = MessageSchema::builder("test_msgs/msg/FixedArray")
        .field("data", FieldType::Array(Box::new(FieldType::Float64), 3))
        .build()
        .unwrap();

    let msg = DynamicMessage::new(&schema);

    // Fixed arrays should be initialized with default values
    let data = msg.get_dynamic("data").unwrap();
    if let DynamicValue::Array(arr) = data {
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], DynamicValue::Float64(0.0));
    } else {
        panic!("Expected Array");
    }
}

#[test]
fn test_message_equality() {
    let schema = create_point_schema();

    let mut msg1 = DynamicMessage::new(&schema);
    msg1.set("x", 1.0f64).unwrap();

    let mut msg2 = DynamicMessage::new(&schema);
    msg2.set("x", 1.0f64).unwrap();

    let mut msg3 = DynamicMessage::new(&schema);
    msg3.set("x", 2.0f64).unwrap();

    assert_eq!(msg1, msg2);
    assert_ne!(msg1, msg3);
}

#[test]
fn assignment_rejects_wrong_shapes_bounds_and_value_kinds() {
    let child = MessageSchema::builder("test_msgs/msg/Child")
        .field("value", FieldType::Int32)
        .build()
        .unwrap();
    let schema = MessageSchema::builder("test_msgs/msg/Validated")
        .field("fixed", FieldType::Array(Box::new(FieldType::Int32), 2))
        .field(
            "bounded",
            FieldType::BoundedSequence(Box::new(FieldType::Int32), 2),
        )
        .field("name", FieldType::BoundedString(4))
        .field("child", FieldType::Message(child.clone()))
        .build()
        .unwrap();
    let mut message = DynamicMessage::new(&schema);

    for values in [
        vec![DynamicValue::Int32(1)],
        vec![
            DynamicValue::Int32(1),
            DynamicValue::Int32(2),
            DynamicValue::Int32(3),
        ],
    ] {
        assert!(matches!(
            message.set_dynamic("fixed", DynamicValue::Array(values)),
            Err(DynamicError::WrongArrayLength { path, .. }) if path == "fixed"
        ));
    }

    message.set("bounded", vec![1_i32, 2]).unwrap();
    assert!(matches!(
        message.set("bounded", vec![1_i32, 2, 3]),
        Err(DynamicError::FieldBoundExceeded { path, .. }) if path == "bounded"
    ));
    message.set("name", "éé").unwrap();
    assert!(matches!(
        message.set("name", "ééé"),
        Err(DynamicError::FieldBoundExceeded {
            path,
            max: 4,
            actual: 6
        }) if path == "name"
    ));
    assert!(matches!(
        message.set("name", "a\0b"),
        Err(DynamicError::InvalidString { path, .. }) if path == "name"
    ));
    assert!(matches!(
        message.set("fixed", "not an array"),
        Err(DynamicError::TypeMismatch { path, .. }) if path == "fixed"
    ));

    let wrong_child_schema = MessageSchema::builder("test_msgs/msg/Child")
        .field("value", FieldType::Uint32)
        .build()
        .unwrap();
    assert!(matches!(
        message.set_dynamic(
            "child",
            DynamicValue::Message(Box::new(DynamicMessage::new(&wrong_child_schema)))
        ),
        Err(DynamicError::TypeMismatch { path, .. }) if path == "child"
    ));

    let mut invalid_child = DynamicMessage::new(&child);
    invalid_child.values_mut()[0] = DynamicValue::String("wrong".into());
    assert!(matches!(
        message.set_dynamic("child", DynamicValue::Message(Box::new(invalid_child))),
        Err(DynamicError::TypeMismatch { path, .. }) if path == "child.value"
    ));

    let mut short_child = DynamicMessage::new(&child);
    short_child.values_mut().clear();
    assert!(matches!(
        message.set_dynamic("child", DynamicValue::Message(Box::new(short_child))),
        Err(DynamicError::WrongArrayLength { path, .. }) if path == "child"
    ));
}

#[test]
fn malformed_value_cardinality_returns_errors_instead_of_panicking() {
    let schema = create_point_schema();
    let mut short = DynamicMessage::new(&schema);
    short.values_mut().clear();
    assert!(matches!(
        short.set("x", 1.0_f64),
        Err(DynamicError::WrongArrayLength {
            expected: 3,
            actual: 0,
            ..
        })
    ));
    assert!(matches!(
        short.set_by_index(0, 1.0_f64),
        Err(DynamicError::WrongArrayLength { .. })
    ));
    assert!(matches!(
        short.get::<f64>("x"),
        Err(DynamicError::WrongArrayLength { .. })
    ));
    assert!(matches!(
        short.get_by_index::<f64>(0),
        Err(DynamicError::WrongArrayLength { .. })
    ));

    let mut long = DynamicMessage::new(&schema);
    long.values_mut().push(DynamicValue::Float64(4.0));
    assert!(matches!(
        long.set_by_index(3, 5.0_f64),
        Err(DynamicError::WrongArrayLength {
            expected: 3,
            actual: 4,
            ..
        })
    ));

    let twist_schema = create_twist_schema();
    let mut twist = DynamicMessage::new(&twist_schema);
    twist.values_mut()[0]
        .as_message_mut()
        .unwrap()
        .values_mut()
        .clear();
    assert!(matches!(
        twist.set("linear.x", 1.0_f64),
        Err(DynamicError::WrongArrayLength { path, .. }) if path == "linear"
    ));
}

#[test]
fn dotted_field_paths_have_a_depth_limit() {
    let schema = create_point_schema();
    let mut message = DynamicMessage::new(&schema);
    let path = std::iter::repeat_n("x", 130).collect::<Vec<_>>().join(".");
    assert!(matches!(
        message.set(&path, 1.0_f64),
        Err(DynamicError::ResourceLimitExceeded(_))
    ));
    assert!(matches!(
        message.get::<f64>(&path),
        Err(DynamicError::ResourceLimitExceeded(_))
    ));
}

#[test]
fn test_schema_access() {
    let schema = create_point_schema();
    let msg = DynamicMessage::new(&schema);

    assert_eq!(msg.schema().type_name, "geometry_msgs/msg/Point");
    assert!(Arc::ptr_eq(&msg.schema_arc(), &schema));
}

#[test]
fn test_deeply_nested_message() {
    let point = MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap();

    let pose = MessageSchema::builder("geometry_msgs/msg/Pose")
        .field("position", FieldType::Message(point.clone()))
        .build()
        .unwrap();

    let pose_stamped = MessageSchema::builder("geometry_msgs/msg/PoseStamped")
        .field("pose", FieldType::Message(pose))
        .build()
        .unwrap();

    let mut msg = DynamicMessage::new(&pose_stamped);
    msg.set("pose.position.x", 1.0f64).unwrap();
    msg.set("pose.position.y", 2.0f64).unwrap();
    msg.set("pose.position.z", 3.0f64).unwrap();

    assert_eq!(msg.get::<f64>("pose.position.x").unwrap(), 1.0);
    assert_eq!(msg.get::<f64>("pose.position.y").unwrap(), 2.0);
    assert_eq!(msg.get::<f64>("pose.position.z").unwrap(), 3.0);
}
