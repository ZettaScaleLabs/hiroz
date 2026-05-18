//! Tests for schema types (FieldType, FieldSchema, MessageSchema).

use crate::dynamic::schema::{FieldType, MessageSchema};

#[test]
fn test_field_type_fixed_size() {
    assert_eq!(FieldType::Bool.fixed_size(), Some(1));
    assert_eq!(FieldType::Int8.fixed_size(), Some(1));
    assert_eq!(FieldType::Int16.fixed_size(), Some(2));
    assert_eq!(FieldType::Int32.fixed_size(), Some(4));
    assert_eq!(FieldType::Int64.fixed_size(), Some(8));
    assert_eq!(FieldType::Uint8.fixed_size(), Some(1));
    assert_eq!(FieldType::Uint16.fixed_size(), Some(2));
    assert_eq!(FieldType::Uint32.fixed_size(), Some(4));
    assert_eq!(FieldType::Uint64.fixed_size(), Some(8));
    assert_eq!(FieldType::Float32.fixed_size(), Some(4));
    assert_eq!(FieldType::Float64.fixed_size(), Some(8));
    assert_eq!(FieldType::String.fixed_size(), None);

    let arr = FieldType::Array(Box::new(FieldType::Float64), 3);
    assert_eq!(arr.fixed_size(), Some(24));

    let seq = FieldType::Sequence(Box::new(FieldType::Int32));
    assert_eq!(seq.fixed_size(), None);
}

#[test]
fn test_field_type_alignment() {
    assert_eq!(FieldType::Bool.alignment(), 1);
    assert_eq!(FieldType::Int8.alignment(), 1);
    assert_eq!(FieldType::Uint8.alignment(), 1);
    assert_eq!(FieldType::Int16.alignment(), 2);
    assert_eq!(FieldType::Uint16.alignment(), 2);
    assert_eq!(FieldType::Int32.alignment(), 4);
    assert_eq!(FieldType::Uint32.alignment(), 4);
    assert_eq!(FieldType::Float32.alignment(), 4);
    assert_eq!(FieldType::Int64.alignment(), 8);
    assert_eq!(FieldType::Uint64.alignment(), 8);
    assert_eq!(FieldType::Float64.alignment(), 8);
    assert_eq!(FieldType::String.alignment(), 4);
}

#[test]
fn test_field_type_is_primitive() {
    assert!(FieldType::Bool.is_primitive());
    assert!(FieldType::Int32.is_primitive());
    assert!(FieldType::Float64.is_primitive());
    assert!(FieldType::String.is_primitive());
    assert!(FieldType::BoundedString(100).is_primitive());

    assert!(!FieldType::Sequence(Box::new(FieldType::Int32)).is_primitive());
    assert!(!FieldType::Array(Box::new(FieldType::Int32), 10).is_primitive());
}

#[test]
fn test_field_type_is_numeric() {
    assert!(FieldType::Int8.is_numeric());
    assert!(FieldType::Int32.is_numeric());
    assert!(FieldType::Float64.is_numeric());

    assert!(!FieldType::Bool.is_numeric());
    assert!(!FieldType::String.is_numeric());
}

#[test]
fn test_message_schema_builder() {
    let schema = MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap();

    assert_eq!(schema.type_name, "geometry_msgs/msg/Point");
    assert_eq!(schema.package, "geometry_msgs");
    assert_eq!(schema.name, "Point");
    assert_eq!(schema.fields.len(), 3);
    assert_eq!(schema.field("x").unwrap().name, "x");
    assert_eq!(schema.field_count(), 3);
}

#[test]
fn test_invalid_type_name() {
    // Missing "/msg/" part
    let result = MessageSchema::builder("invalid_name").build();
    assert!(result.is_err());

    // Wrong separator
    let result = MessageSchema::builder("pkg/srv/Name").build();
    assert!(result.is_err());
}

#[test]
fn test_field_path_indices() {
    let point = MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()
        .unwrap();

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

    // Simple path
    let indices = point.field_path_indices("x").unwrap();
    assert_eq!(indices, vec![0]);

    let indices = point.field_path_indices("z").unwrap();
    assert_eq!(indices, vec![2]);

    // Nested path
    let indices = twist.field_path_indices("linear.x").unwrap();
    assert_eq!(indices, vec![0, 0]);

    let indices = twist.field_path_indices("angular.z").unwrap();
    assert_eq!(indices, vec![1, 2]);

    // Field not found
    let result = point.field_path_indices("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_message_schema_field_names() {
    let schema = MessageSchema::builder("test_msgs/msg/Test")
        .field("a", FieldType::Int32)
        .field("b", FieldType::String)
        .field("c", FieldType::Float64)
        .build()
        .unwrap();

    let names: Vec<&str> = schema.field_names().collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn test_message_schema_with_type_hash() {
    let schema = MessageSchema::builder("test_msgs/msg/Test")
        .field("x", FieldType::Int32)
        .type_hash("RIHS01_abcd1234")
        .build()
        .unwrap();

    assert_eq!(schema.type_hash, Some("RIHS01_abcd1234".to_string()));
}

#[test]
fn test_schema_equality() {
    let schema1 = MessageSchema::builder("test_msgs/msg/Test")
        .field("x", FieldType::Int32)
        .build()
        .unwrap();

    let schema2 = MessageSchema::builder("test_msgs/msg/Test")
        .field("x", FieldType::Int32)
        .build()
        .unwrap();

    let schema3 = MessageSchema::builder("test_msgs/msg/Other")
        .field("x", FieldType::Int32)
        .build()
        .unwrap();

    assert_eq!(*schema1, *schema2);
    assert_ne!(*schema1, *schema3);
}
