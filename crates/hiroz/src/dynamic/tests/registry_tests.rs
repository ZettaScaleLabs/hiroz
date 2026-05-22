//! Tests for the schema registry.

use std::sync::Arc;

use crate::dynamic::registry::{SchemaRegistry, get_schema, has_schema, register_schema};
use crate::dynamic::schema::{FieldType, MessageSchema};

fn create_test_schema(name: &str) -> Arc<MessageSchema> {
    MessageSchema::builder(name)
        .field("x", FieldType::Float64)
        .build()
        .unwrap()
}

#[test]
fn test_registry_basic_operations() {
    let mut registry = SchemaRegistry::new();
    assert!(registry.is_empty());

    let schema = create_test_schema("test_msgs/msg/Point");
    registry.register(schema.clone());

    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
    assert!(registry.contains("test_msgs/msg/Point"));

    let retrieved = registry.get("test_msgs/msg/Point");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().type_name, "test_msgs/msg/Point");
}

#[test]
fn test_registry_not_found() {
    let registry = SchemaRegistry::new();
    assert!(registry.get("nonexistent/msg/Type").is_none());
    assert!(!registry.contains("nonexistent/msg/Type"));
}

#[test]
fn test_registry_multiple_schemas() {
    let mut registry = SchemaRegistry::new();

    registry.register(create_test_schema("pkg1/msg/A"));
    registry.register(create_test_schema("pkg2/msg/B"));
    registry.register(create_test_schema("pkg3/msg/C"));

    assert_eq!(registry.len(), 3);
    assert!(registry.contains("pkg1/msg/A"));
    assert!(registry.contains("pkg2/msg/B"));
    assert!(registry.contains("pkg3/msg/C"));
}

#[test]
fn test_registry_type_names_iteration() {
    let mut registry = SchemaRegistry::new();

    registry.register(create_test_schema("pkg1/msg/A"));
    registry.register(create_test_schema("pkg2/msg/B"));

    let names: Vec<&str> = registry.type_names().collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"pkg1/msg/A"));
    assert!(names.contains(&"pkg2/msg/B"));
}

#[test]
fn test_registry_clear() {
    let mut registry = SchemaRegistry::new();

    registry.register(create_test_schema("pkg1/msg/A"));
    registry.register(create_test_schema("pkg2/msg/B"));
    assert_eq!(registry.len(), 2);

    registry.clear();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn test_registry_replace_schema() {
    let mut registry = SchemaRegistry::new();

    let schema1 = MessageSchema::builder("test_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .build()
        .unwrap();

    let schema2 = MessageSchema::builder("test_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .build()
        .unwrap();

    registry.register(schema1);
    assert_eq!(registry.get("test_msgs/msg/Point").unwrap().fields.len(), 1);

    registry.register(schema2);
    assert_eq!(registry.get("test_msgs/msg/Point").unwrap().fields.len(), 2);
}

#[test]
fn test_global_registry_functions() {
    // Use a unique type name to avoid conflicts with other tests
    let type_name = "test_global_unique/msg/TestMessage123";

    // Register a schema
    let schema = create_test_schema(type_name);
    register_schema(schema);

    // It should be retrievable
    assert!(has_schema(type_name));
    let retrieved = get_schema(type_name);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().type_name, type_name);
}

#[test]
fn test_global_registry_returns_none_for_unknown() {
    assert!(!has_schema("completely_unknown/msg/Type"));
    assert!(get_schema("completely_unknown/msg/Type").is_none());
}

#[test]
fn test_schema_sharing() {
    let mut registry = SchemaRegistry::new();

    let schema = create_test_schema("shared/msg/Schema");
    let returned = registry.register(schema.clone());

    // The returned Arc should be the same as the input
    assert!(Arc::ptr_eq(&schema, &returned));

    // Getting from registry should return an equivalent Arc
    let retrieved = registry.get("shared/msg/Schema").unwrap();
    assert!(Arc::ptr_eq(&schema, &retrieved));
}
