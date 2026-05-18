//! Tests for interoperability between static and dynamic messages.
//!
//! These tests verify that:
//! - Static messages serialized to CDR can be deserialized as dynamic messages
//! - Dynamic messages serialized to CDR can be deserialized as static messages

use std::sync::Arc;

use crate::dynamic::message::DynamicMessage;
use crate::dynamic::schema::{FieldType, MessageSchema};
use crate::msg::ZMessage;

use hiroz_msgs::geometry_msgs::{Point, Twist, Vector3};
use hiroz_msgs::std_msgs::String as StdString;

/// Create a Point schema matching geometry_msgs/msg/Point
fn create_point_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap()
}

/// Create a Vector3 schema matching geometry_msgs/msg/Vector3
fn create_vector3_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("geometry_msgs/msg/Vector3")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap()
}

/// Create a String schema matching std_msgs/msg/String
fn create_string_schema() -> Arc<MessageSchema> {
    MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()
        .unwrap()
}

/// Create a Twist schema matching geometry_msgs/msg/Twist
fn create_twist_schema() -> Arc<MessageSchema> {
    let vector3 = create_vector3_schema();
    MessageSchema::builder("geometry_msgs/msg/Twist")
        .field("linear", FieldType::Message(vector3.clone()))
        .field("angular", FieldType::Message(vector3))
        .build()
        .unwrap()
}

#[test]
fn test_static_point_to_dynamic() {
    // Create a static Point message
    let static_msg = Point {
        x: 1.5,
        y: 2.5,
        z: 3.5,
    };

    // Serialize to CDR bytes
    let cdr_bytes = static_msg.serialize();

    // Deserialize as dynamic message
    let schema = create_point_schema();
    let dynamic_msg = DynamicMessage::from_cdr(&cdr_bytes, &schema).unwrap();

    // Verify values match
    assert_eq!(dynamic_msg.get::<f64>("x").unwrap(), 1.5);
    assert_eq!(dynamic_msg.get::<f64>("y").unwrap(), 2.5);
    assert_eq!(dynamic_msg.get::<f64>("z").unwrap(), 3.5);
}

#[test]
fn test_dynamic_point_to_static() {
    // Create a dynamic Point message
    let schema = create_point_schema();
    let mut dynamic_msg = DynamicMessage::new(&schema);
    dynamic_msg.set("x", 10.0f64).unwrap();
    dynamic_msg.set("y", 20.0f64).unwrap();
    dynamic_msg.set("z", 30.0f64).unwrap();

    // Serialize to CDR bytes
    let cdr_bytes = dynamic_msg.to_cdr().unwrap();

    // Deserialize as static message
    let static_msg = Point::deserialize(&cdr_bytes).unwrap();

    // Verify values match
    assert_eq!(static_msg.x, 10.0);
    assert_eq!(static_msg.y, 20.0);
    assert_eq!(static_msg.z, 30.0);
}

#[test]
fn test_static_string_to_dynamic() {
    // Create a static String message
    let static_msg = StdString {
        data: "Hello from static!".to_string(),
    };

    // Serialize to CDR bytes
    let cdr_bytes = static_msg.serialize();

    // Deserialize as dynamic message
    let schema = create_string_schema();
    let dynamic_msg = DynamicMessage::from_cdr(&cdr_bytes, &schema).unwrap();

    // Verify value matches
    assert_eq!(
        dynamic_msg.get::<String>("data").unwrap(),
        "Hello from static!"
    );
}

#[test]
fn test_dynamic_string_to_static() {
    // Create a dynamic String message
    let schema = create_string_schema();
    let mut dynamic_msg = DynamicMessage::new(&schema);
    dynamic_msg.set("data", "Hello from dynamic!").unwrap();

    // Serialize to CDR bytes
    let cdr_bytes = dynamic_msg.to_cdr().unwrap();

    // Deserialize as static message
    let static_msg = StdString::deserialize(&cdr_bytes).unwrap();

    // Verify value matches
    assert_eq!(static_msg.data, "Hello from dynamic!");
}

#[test]
fn test_static_twist_to_dynamic() {
    // Create a static Twist message (nested)
    let static_msg = Twist {
        linear: Vector3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        angular: Vector3 {
            x: 0.1,
            y: 0.2,
            z: 0.3,
        },
    };

    // Serialize to CDR bytes
    let cdr_bytes = static_msg.serialize();

    // Deserialize as dynamic message
    let schema = create_twist_schema();
    let dynamic_msg = DynamicMessage::from_cdr(&cdr_bytes, &schema).unwrap();

    // Verify nested values match using dot notation
    assert_eq!(dynamic_msg.get::<f64>("linear.x").unwrap(), 1.0);
    assert_eq!(dynamic_msg.get::<f64>("linear.y").unwrap(), 2.0);
    assert_eq!(dynamic_msg.get::<f64>("linear.z").unwrap(), 3.0);
    assert_eq!(dynamic_msg.get::<f64>("angular.x").unwrap(), 0.1);
    assert_eq!(dynamic_msg.get::<f64>("angular.y").unwrap(), 0.2);
    assert_eq!(dynamic_msg.get::<f64>("angular.z").unwrap(), 0.3);
}

#[test]
fn test_dynamic_twist_to_static() {
    // Create a dynamic Twist message
    let schema = create_twist_schema();
    let mut dynamic_msg = DynamicMessage::new(&schema);
    dynamic_msg.set("linear.x", 0.5f64).unwrap();
    dynamic_msg.set("linear.y", 0.0f64).unwrap();
    dynamic_msg.set("linear.z", 0.0f64).unwrap();
    dynamic_msg.set("angular.x", 0.0f64).unwrap();
    dynamic_msg.set("angular.y", 0.0f64).unwrap();
    dynamic_msg.set("angular.z", 0.3f64).unwrap();

    // Serialize to CDR bytes
    let cdr_bytes = dynamic_msg.to_cdr().unwrap();

    // Deserialize as static message
    let static_msg = Twist::deserialize(&cdr_bytes).unwrap();

    // Verify values match
    assert_eq!(static_msg.linear.x, 0.5);
    assert_eq!(static_msg.linear.y, 0.0);
    assert_eq!(static_msg.linear.z, 0.0);
    assert_eq!(static_msg.angular.x, 0.0);
    assert_eq!(static_msg.angular.y, 0.0);
    assert_eq!(static_msg.angular.z, 0.3);
}

#[test]
fn test_roundtrip_static_dynamic_static() {
    // Static → Dynamic → Static roundtrip
    let original = Point {
        x: 1.23,
        y: 4.56,
        z: 7.89,
    };

    // Static → CDR → Dynamic
    let cdr_bytes = original.serialize();
    let schema = create_point_schema();
    let dynamic_msg = DynamicMessage::from_cdr(&cdr_bytes, &schema).unwrap();

    // Dynamic → CDR → Static
    let cdr_bytes2 = dynamic_msg.to_cdr().unwrap();
    let recovered = Point::deserialize(&cdr_bytes2).unwrap();

    // Verify original matches recovered
    assert_eq!(original.x, recovered.x);
    assert_eq!(original.y, recovered.y);
    assert_eq!(original.z, recovered.z);
}

#[test]
fn test_roundtrip_dynamic_static_dynamic() {
    // Dynamic → Static → Dynamic roundtrip
    let schema = create_point_schema();
    let mut original = DynamicMessage::new(&schema);
    original.set("x", 9.87f64).unwrap();
    original.set("y", 6.54f64).unwrap();
    original.set("z", 3.21f64).unwrap();

    // Dynamic → CDR → Static
    let cdr_bytes = original.to_cdr().unwrap();
    let static_msg = Point::deserialize(&cdr_bytes).unwrap();

    // Static → CDR → Dynamic
    let cdr_bytes2 = static_msg.serialize();
    let recovered = DynamicMessage::from_cdr(&cdr_bytes2, &schema).unwrap();

    // Verify original matches recovered
    assert_eq!(
        original.get::<f64>("x").unwrap(),
        recovered.get::<f64>("x").unwrap()
    );
    assert_eq!(
        original.get::<f64>("y").unwrap(),
        recovered.get::<f64>("y").unwrap()
    );
    assert_eq!(
        original.get::<f64>("z").unwrap(),
        recovered.get::<f64>("z").unwrap()
    );
}

#[test]
fn test_cdr_bytes_identical() {
    // Verify that static and dynamic produce identical CDR bytes
    let static_msg = Point {
        x: 1.0,
        y: 2.0,
        z: 3.0,
    };

    let schema = create_point_schema();
    let mut dynamic_msg = DynamicMessage::new(&schema);
    dynamic_msg.set("x", 1.0f64).unwrap();
    dynamic_msg.set("y", 2.0f64).unwrap();
    dynamic_msg.set("z", 3.0f64).unwrap();

    let static_bytes = static_msg.serialize();
    let dynamic_bytes = dynamic_msg.to_cdr().unwrap();

    // CDR bytes should be identical
    assert_eq!(static_bytes, dynamic_bytes);
}
