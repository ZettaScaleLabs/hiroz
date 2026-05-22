//! RIHS01 Type Hash Calculation for ROS2
//!
//! This module implements the ROS2 RIHS01 (ROS IDL Hash Standard 01) type hashing algorithm.
//!
//! Type descriptions and hash computation are provided by `hiroz-schema`.
//! This module provides codegen-specific functions for building TypeDescription
//! from parsed message definitions.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::types::{ArrayType, FieldType, ParsedMessage};

// Re-export types from hiroz-schema for backwards compatibility
pub use hiroz_schema::{
    FieldDescription, FieldTypeDescription, TypeDescription, TypeDescriptionMsg, TypeHash, TypeId,
    calculate_hash, to_hash_version, to_ros2_json,
};

/// Calculate RIHS01 hash for a message
pub fn calculate_type_hash(
    msg: &ParsedMessage,
    resolved_deps: &BTreeMap<String, TypeDescription>,
) -> Result<TypeHash> {
    let type_desc_msg = build_type_description_msg(msg, resolved_deps)?;
    Ok(calculate_hash(&type_desc_msg))
}

/// Calculate RIHS01 hash for a service (Request + Response + Event)
/// Based on ROS2 RIHS01 specification
pub fn calculate_service_type_hash(
    package: &str,
    service_name: &str,
    request_desc: &TypeDescription,
    response_desc: &TypeDescription,
    service_event_info_desc: &TypeDescription,
    resolved_deps: &BTreeMap<String, TypeDescription>,
) -> Result<TypeHash> {
    // ROS2 uses slash format, not :: format
    // Detect if this is an action service (contains SendGoal/GetResult/CancelGoal)
    let is_action = service_name.contains("SendGoal")
        || service_name.contains("GetResult")
        || service_name.contains("CancelGoal");
    let path = if is_action { "action" } else { "srv" };

    // Action services use /action/ path, regular services use /srv/ path
    let service_type_name = format!("{}/{}/{}", package, path, service_name);
    let request_type_name = format!("{}/{}/{}_Request", package, path, service_name);
    let response_type_name = format!("{}/{}/{}_Response", package, path, service_name);
    let event_type_name = format!("{}/{}/{}_Event", package, path, service_name);

    // Create the Event type with three fields: info, request[], response[]
    let event_desc = TypeDescription {
        type_name: event_type_name.clone(),
        fields: vec![
            FieldDescription {
                name: "info".to_string(),
                field_type: FieldTypeDescription::nested(
                    TypeId::NESTED_TYPE,
                    service_event_info_desc.type_name.clone(),
                ),
                default_value: String::new(),
            },
            FieldDescription {
                name: "request".to_string(),
                field_type: FieldTypeDescription::nested_array(
                    TypeId::NESTED_TYPE_BOUNDED_SEQUENCE,
                    1,
                    request_type_name.clone(),
                ),
                default_value: String::new(),
            },
            FieldDescription {
                name: "response".to_string(),
                field_type: FieldTypeDescription::nested_array(
                    TypeId::NESTED_TYPE_BOUNDED_SEQUENCE,
                    1,
                    response_type_name.clone(),
                ),
                default_value: String::new(),
            },
        ],
    };

    // Main service type description with three fields in definition order (NOT alphabetical)
    let service_desc = TypeDescription {
        type_name: service_type_name,
        fields: vec![
            FieldDescription {
                name: "request_message".to_string(),
                field_type: FieldTypeDescription::nested(
                    TypeId::NESTED_TYPE,
                    request_type_name.clone(),
                ),
                default_value: String::new(),
            },
            FieldDescription {
                name: "response_message".to_string(),
                field_type: FieldTypeDescription::nested(
                    TypeId::NESTED_TYPE,
                    response_type_name.clone(),
                ),
                default_value: String::new(),
            },
            FieldDescription {
                name: "event_message".to_string(),
                field_type: FieldTypeDescription::nested(TypeId::NESTED_TYPE, event_type_name),
                default_value: String::new(),
            },
        ],
    };

    // Build referenced type descriptions
    // Need to include: Request, Response, Event, ServiceEventInfo, and any deps of ServiceEventInfo
    let mut request_desc_corrected = request_desc.clone();
    request_desc_corrected.type_name = request_type_name;

    let mut response_desc_corrected = response_desc.clone();
    response_desc_corrected.type_name = response_type_name;

    let mut referenced = BTreeMap::new();

    // Add ServiceEventInfo and its dependencies (like Time)
    for desc in resolved_deps.values() {
        referenced.insert(desc.type_name.clone(), desc.clone());
    }

    // Add the three service-specific types
    referenced.insert(event_desc.type_name.clone(), event_desc);
    referenced.insert(
        request_desc_corrected.type_name.clone(),
        request_desc_corrected,
    );
    referenced.insert(
        response_desc_corrected.type_name.clone(),
        response_desc_corrected,
    );

    // Sort referenced types alphabetically by type_name
    let mut referenced_vec: Vec<TypeDescription> = referenced.into_values().collect();
    referenced_vec.sort_by(|a, b| a.type_name.cmp(&b.type_name));

    let type_desc_msg = TypeDescriptionMsg {
        type_description: service_desc,
        referenced_type_descriptions: referenced_vec,
    };

    Ok(calculate_hash(&type_desc_msg))
}

/// Build TypeDescription message from parsed message
pub fn build_type_description_msg(
    msg: &ParsedMessage,
    resolved_deps: &BTreeMap<String, TypeDescription>,
) -> Result<TypeDescriptionMsg> {
    let type_name = format!("{}/msg/{}", msg.package, msg.name);

    let mut fields = Vec::new();
    let mut referenced_types = BTreeMap::new();

    // ROS2 treats empty messages as having a single uint8 field
    if msg.fields.is_empty() {
        fields.push(FieldDescription {
            name: "structure_needs_at_least_one_member".to_string(),
            field_type: FieldTypeDescription::primitive(TypeId::UINT8),
            default_value: String::new(),
        });
    } else {
        for field in &msg.fields {
            let type_id = get_type_id(&field.field_type)?;
            let capacity = get_array_capacity(&field.field_type);
            let string_capacity = field.field_type.string_bound.unwrap_or(0) as u64;

            // For nested types, get the fully qualified name with package context
            let nested_type_name = if is_primitive_type(&field.field_type.base_type) {
                String::new()
            } else {
                get_nested_type_name_with_context(&field.field_type, &msg.package)
            };

            let field_type_desc = if nested_type_name.is_empty() {
                if capacity > 0 {
                    FieldTypeDescription::array(type_id, capacity)
                } else if string_capacity > 0 {
                    // Bounded string
                    FieldTypeDescription {
                        type_id,
                        capacity: 0,
                        string_capacity,
                        nested_type_name: String::new(),
                    }
                } else {
                    FieldTypeDescription::primitive(type_id)
                }
            } else if capacity > 0 {
                FieldTypeDescription::nested_array(type_id, capacity, nested_type_name)
            } else {
                FieldTypeDescription::nested(type_id, nested_type_name)
            };

            fields.push(FieldDescription {
                name: field.name.clone(),
                field_type: field_type_desc,
                default_value: String::new(),
            });

            // Collect nested type references
            if !is_primitive_type(&field.field_type.base_type) {
                // Determine the package - if None, it's the same package as the message
                let pkg = field.field_type.package.as_deref().unwrap_or(&msg.package);

                let dep_key = format!("{}/{}", pkg, field.field_type.base_type);

                if let Some(dep_desc) = resolved_deps.get(&dep_key) {
                    collect_referenced_types(dep_desc, resolved_deps, &mut referenced_types);
                    referenced_types.insert(dep_desc.type_name.clone(), dep_desc.clone());
                }
            }
        }
    }

    Ok(TypeDescriptionMsg {
        type_description: TypeDescription { type_name, fields },
        referenced_type_descriptions: referenced_types.into_values().collect(),
    })
}

/// Convert a fully-qualified nested type name to the key format used in resolved_deps.
///
/// The resolver stores type descriptions keyed as `"pkg/TypeName"`, but
/// `nested_type_name` in field descriptors uses `"pkg/subdir/TypeName"` (e.g.
/// `"geometry_msgs/msg/Pose"`). This strips the middle path component so lookups
/// succeed regardless of whether the subdir is `msg`, `srv`, `action`, or any
/// future category.
fn nested_type_name_to_key(name: &str) -> String {
    let parts: Vec<&str> = name.splitn(3, '/').collect();
    if parts.len() == 3 {
        format!("{}/{}", parts[0], parts[2])
    } else {
        name.to_string()
    }
}

/// Recursively collect all referenced types
fn collect_referenced_types(
    type_desc: &TypeDescription,
    all_deps: &BTreeMap<String, TypeDescription>,
    collected: &mut BTreeMap<String, TypeDescription>,
) {
    for field in &type_desc.fields {
        if !field.field_type.nested_type_name.is_empty() {
            let key = nested_type_name_to_key(&field.field_type.nested_type_name);

            if let Some(dep) = all_deps.get(&key)
                && !collected.contains_key(&dep.type_name)
            {
                collect_referenced_types(dep, all_deps, collected);
                collected.insert(dep.type_name.clone(), dep.clone());
            }
        }
    }
}

/// Get ROS2 type ID for a field type (1-166)
pub fn get_type_id(field_type: &FieldType) -> Result<u8> {
    let base_type = field_type.base_type.as_str();
    let array = &field_type.array;
    let has_package = field_type.package.is_some();
    let is_bounded_string = field_type.string_bound.is_some();

    // Nested types (either explicitly packaged OR not a primitive)
    // package=None means same-package reference for custom types
    if has_package || !is_primitive_type(base_type) {
        return Ok(match array {
            ArrayType::Single => TypeId::NESTED_TYPE,
            ArrayType::Fixed(_) => TypeId::NESTED_TYPE_ARRAY,
            ArrayType::Bounded(_) => TypeId::NESTED_TYPE_BOUNDED_SEQUENCE,
            ArrayType::Unbounded => TypeId::NESTED_TYPE_UNBOUNDED_SEQUENCE,
        });
    }

    // Handle bounded strings specially
    if is_bounded_string && base_type == "string" {
        return Ok(TypeId::BOUNDED_STRING);
    }

    // Primitive types
    // Note: In ROS2:
    //   'char'  is uint8 (type_id 3)
    //   'byte'  is its own distinct type (type_id 16) — NOT the same as uint8
    match (base_type, array) {
        // Single primitives
        ("int8", ArrayType::Single) => Ok(TypeId::INT8),
        ("uint8" | "char", ArrayType::Single) => Ok(TypeId::UINT8),
        ("byte", ArrayType::Single) => Ok(TypeId::BYTE),
        ("int16", ArrayType::Single) => Ok(TypeId::INT16),
        ("uint16", ArrayType::Single) => Ok(TypeId::UINT16),
        ("int32", ArrayType::Single) => Ok(TypeId::INT32),
        ("uint32", ArrayType::Single) => Ok(TypeId::UINT32),
        ("int64", ArrayType::Single) => Ok(TypeId::INT64),
        ("uint64", ArrayType::Single) => Ok(TypeId::UINT64),
        ("float32", ArrayType::Single) => Ok(TypeId::FLOAT32),
        ("float64", ArrayType::Single) => Ok(TypeId::FLOAT64),
        ("bool", ArrayType::Single) => Ok(TypeId::BOOL),
        ("string", ArrayType::Single) => Ok(TypeId::STRING),

        // Fixed arrays
        ("int8", ArrayType::Fixed(_)) => Ok(TypeId::INT8_ARRAY),
        ("uint8" | "char", ArrayType::Fixed(_)) => Ok(TypeId::UINT8_ARRAY),
        ("byte", ArrayType::Fixed(_)) => Ok(TypeId::BYTE + TypeId::ARRAY_OFFSET),
        ("int16", ArrayType::Fixed(_)) => Ok(TypeId::INT16_ARRAY),
        ("uint16", ArrayType::Fixed(_)) => Ok(TypeId::UINT16_ARRAY),
        ("int32", ArrayType::Fixed(_)) => Ok(TypeId::INT32_ARRAY),
        ("uint32", ArrayType::Fixed(_)) => Ok(TypeId::UINT32_ARRAY),
        ("int64", ArrayType::Fixed(_)) => Ok(TypeId::INT64_ARRAY),
        ("uint64", ArrayType::Fixed(_)) => Ok(TypeId::UINT64_ARRAY),
        ("float32", ArrayType::Fixed(_)) => Ok(TypeId::FLOAT32_ARRAY),
        ("float64", ArrayType::Fixed(_)) => Ok(TypeId::FLOAT64_ARRAY),
        ("bool", ArrayType::Fixed(_)) => Ok(TypeId::BOOL_ARRAY),
        ("string", ArrayType::Fixed(_)) => Ok(TypeId::STRING_ARRAY),

        // Bounded sequences
        ("int8", ArrayType::Bounded(_)) => Ok(TypeId::INT8_BOUNDED_SEQUENCE),
        ("uint8" | "char", ArrayType::Bounded(_)) => Ok(TypeId::UINT8_BOUNDED_SEQUENCE),
        ("byte", ArrayType::Bounded(_)) => Ok(TypeId::BYTE + TypeId::BOUNDED_SEQUENCE_OFFSET),
        ("int16", ArrayType::Bounded(_)) => Ok(TypeId::INT16_BOUNDED_SEQUENCE),
        ("uint16", ArrayType::Bounded(_)) => Ok(TypeId::UINT16_BOUNDED_SEQUENCE),
        ("int32", ArrayType::Bounded(_)) => Ok(TypeId::INT32_BOUNDED_SEQUENCE),
        ("uint32", ArrayType::Bounded(_)) => Ok(TypeId::UINT32_BOUNDED_SEQUENCE),
        ("int64", ArrayType::Bounded(_)) => Ok(TypeId::INT64_BOUNDED_SEQUENCE),
        ("uint64", ArrayType::Bounded(_)) => Ok(TypeId::UINT64_BOUNDED_SEQUENCE),
        ("float32", ArrayType::Bounded(_)) => Ok(TypeId::FLOAT32_BOUNDED_SEQUENCE),
        ("float64", ArrayType::Bounded(_)) => Ok(TypeId::FLOAT64_BOUNDED_SEQUENCE),
        ("bool", ArrayType::Bounded(_)) => Ok(TypeId::BOOL_BOUNDED_SEQUENCE),
        ("string", ArrayType::Bounded(_)) => Ok(TypeId::STRING_BOUNDED_SEQUENCE),

        // Unbounded sequences
        ("int8", ArrayType::Unbounded) => Ok(TypeId::INT8_UNBOUNDED_SEQUENCE),
        ("uint8" | "char", ArrayType::Unbounded) => Ok(TypeId::UINT8_UNBOUNDED_SEQUENCE),
        ("byte", ArrayType::Unbounded) => Ok(TypeId::BYTE + TypeId::UNBOUNDED_SEQUENCE_OFFSET),
        ("int16", ArrayType::Unbounded) => Ok(TypeId::INT16_UNBOUNDED_SEQUENCE),
        ("uint16", ArrayType::Unbounded) => Ok(TypeId::UINT16_UNBOUNDED_SEQUENCE),
        ("int32", ArrayType::Unbounded) => Ok(TypeId::INT32_UNBOUNDED_SEQUENCE),
        ("uint32", ArrayType::Unbounded) => Ok(TypeId::UINT32_UNBOUNDED_SEQUENCE),
        ("int64", ArrayType::Unbounded) => Ok(TypeId::INT64_UNBOUNDED_SEQUENCE),
        ("uint64", ArrayType::Unbounded) => Ok(TypeId::UINT64_UNBOUNDED_SEQUENCE),
        ("float32", ArrayType::Unbounded) => Ok(TypeId::FLOAT32_UNBOUNDED_SEQUENCE),
        ("float64", ArrayType::Unbounded) => Ok(TypeId::FLOAT64_UNBOUNDED_SEQUENCE),
        ("bool", ArrayType::Unbounded) => Ok(TypeId::BOOL_UNBOUNDED_SEQUENCE),
        ("string", ArrayType::Unbounded) => Ok(TypeId::STRING_UNBOUNDED_SEQUENCE),

        _ => bail!(
            "Unsupported field type: {} with array {:?}",
            base_type,
            array
        ),
    }
}

/// Get array capacity for field type
fn get_array_capacity(field_type: &FieldType) -> u64 {
    match &field_type.array {
        ArrayType::Fixed(n) | ArrayType::Bounded(n) => *n as u64,
        _ => 0,
    }
}

/// Get nested type name with package context for same-package references
fn get_nested_type_name_with_context(field_type: &FieldType, source_package: &str) -> String {
    let pkg = field_type.package.as_deref().unwrap_or(source_package);
    format!("{}/msg/{}", pkg, field_type.base_type)
}

/// Check if a base type name is a ROS primitive
pub fn is_primitive_type(base_type: &str) -> bool {
    matches!(
        base_type,
        "bool"
            | "byte"
            | "char"
            | "uint8"
            | "int8"
            | "uint16"
            | "int16"
            | "uint32"
            | "int32"
            | "uint64"
            | "int64"
            | "float32"
            | "float64"
            | "string"
            | "wstring"
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::{ArrayType, Field, FieldType as CodegenFieldType};

    /// Helper to create a TypeDescription with primitive fields only
    fn primitive_type_desc(type_name: &str, fields: Vec<(&str, u8)>) -> TypeDescription {
        TypeDescription {
            type_name: type_name.to_string(),
            fields: fields
                .into_iter()
                .map(|(name, type_id)| FieldDescription {
                    name: name.to_string(),
                    field_type: FieldTypeDescription::primitive(type_id),
                    default_value: String::new(),
                })
                .collect(),
        }
    }

    /// Helper to create a TypeDescription with a nested field referencing another type
    fn nested_type_desc(
        type_name: &str,
        nested_field_name: &str,
        nested_type_name: &str,
    ) -> TypeDescription {
        TypeDescription {
            type_name: type_name.to_string(),
            fields: vec![FieldDescription {
                name: nested_field_name.to_string(),
                field_type: FieldTypeDescription::nested(
                    TypeId::NESTED_TYPE,
                    nested_type_name.to_string(),
                ),
                default_value: String::new(),
            }],
        }
    }

    #[test]
    fn test_collect_referenced_types_with_msg_prefix() {
        let parent = nested_type_desc("test_msgs/msg/Parent", "child", "test_msgs/msg/Child");
        let child = primitive_type_desc("test_msgs/msg/Child", vec![("value", TypeId::INT32)]);

        let mut all_deps = BTreeMap::new();
        all_deps.insert("test_msgs/Child".to_string(), child.clone());

        let mut collected = BTreeMap::new();
        collect_referenced_types(&parent, &all_deps, &mut collected);

        assert_eq!(collected.len(), 1);
        assert!(collected.contains_key("test_msgs/msg/Child"));
    }

    #[test]
    fn test_collect_referenced_types_without_prefix() {
        let parent = nested_type_desc("test_msgs/msg/Parent", "child", "test_msgs/Child");
        let child = primitive_type_desc("test_msgs/msg/Child", vec![("value", TypeId::INT32)]);

        let mut all_deps = BTreeMap::new();
        all_deps.insert("test_msgs/Child".to_string(), child.clone());

        let mut collected = BTreeMap::new();
        collect_referenced_types(&parent, &all_deps, &mut collected);

        assert_eq!(collected.len(), 1);
        assert!(collected.contains_key("test_msgs/msg/Child"));
    }

    #[test]
    fn test_collect_referenced_types_deeply_nested() {
        let grandparent = nested_type_desc("pkg/msg/Grandparent", "parent", "pkg/msg/Parent");
        let parent = nested_type_desc("pkg/msg/Parent", "child", "other_pkg/msg/Child");
        let child = primitive_type_desc("other_pkg/msg/Child", vec![("x", TypeId::FLOAT64)]);

        let mut all_deps = BTreeMap::new();
        all_deps.insert("pkg/Parent".to_string(), parent.clone());
        all_deps.insert("other_pkg/Child".to_string(), child.clone());

        let mut collected = BTreeMap::new();
        collect_referenced_types(&grandparent, &all_deps, &mut collected);

        assert_eq!(collected.len(), 2);
        assert!(collected.contains_key("pkg/msg/Parent"));
        assert!(collected.contains_key("other_pkg/msg/Child"));
    }

    #[test]
    fn test_collect_referenced_types_no_duplicates() {
        let parent = TypeDescription {
            type_name: "pkg/msg/Parent".to_string(),
            fields: vec![
                FieldDescription {
                    name: "a".to_string(),
                    field_type: FieldTypeDescription::nested(
                        TypeId::NESTED_TYPE,
                        "pkg/msg/Shared".to_string(),
                    ),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "b".to_string(),
                    field_type: FieldTypeDescription::nested(
                        TypeId::NESTED_TYPE,
                        "pkg/msg/Shared".to_string(),
                    ),
                    default_value: String::new(),
                },
            ],
        };
        let shared = primitive_type_desc("pkg/msg/Shared", vec![("val", TypeId::INT32)]);

        let mut all_deps = BTreeMap::new();
        all_deps.insert("pkg/Shared".to_string(), shared.clone());

        let mut collected = BTreeMap::new();
        collect_referenced_types(&parent, &all_deps, &mut collected);

        assert_eq!(collected.len(), 1);
    }

    #[test]
    fn test_collect_referenced_types_with_action_prefix() {
        // action_msgs/action/GoalInfo references unique_identifier_msgs/action/UUID
        // The /action/ subdir must be stripped the same way as /msg/ and /srv/
        let parent = nested_type_desc(
            "action_msgs/action/GoalStatus",
            "goal_info",
            "action_msgs/action/GoalInfo",
        );
        let goal_info = primitive_type_desc(
            "action_msgs/action/GoalInfo",
            vec![("stamp", TypeId::INT32)],
        );

        let mut all_deps = BTreeMap::new();
        all_deps.insert("action_msgs/GoalInfo".to_string(), goal_info.clone());

        let mut collected = BTreeMap::new();
        collect_referenced_types(&parent, &all_deps, &mut collected);

        assert_eq!(collected.len(), 1);
        assert!(collected.contains_key("action_msgs/action/GoalInfo"));
    }

    // --- Tests for nested_type_name_to_key ---

    #[test]
    fn test_nested_type_name_to_key_msg() {
        assert_eq!(
            nested_type_name_to_key("geometry_msgs/msg/Pose"),
            "geometry_msgs/Pose"
        );
    }

    #[test]
    fn test_nested_type_name_to_key_srv() {
        assert_eq!(
            nested_type_name_to_key("example_interfaces/srv/AddTwoInts"),
            "example_interfaces/AddTwoInts"
        );
    }

    #[test]
    fn test_nested_type_name_to_key_action() {
        assert_eq!(
            nested_type_name_to_key("action_msgs/action/GoalInfo"),
            "action_msgs/GoalInfo"
        );
    }

    #[test]
    fn test_nested_type_name_to_key_no_subdir() {
        // Two-part names (already in pkg/Type format) pass through unchanged
        assert_eq!(
            nested_type_name_to_key("geometry_msgs/Pose"),
            "geometry_msgs/Pose"
        );
    }

    #[test]
    fn test_nested_type_name_to_key_bare_name() {
        // Unqualified names pass through unchanged
        assert_eq!(nested_type_name_to_key("Pose"), "Pose");
    }

    #[test]
    fn test_build_type_description_msg_with_nested_dep() {
        let child = primitive_type_desc("other_pkg/msg/Child", vec![("data", TypeId::STRING)]);

        let mut resolved_deps = BTreeMap::new();
        resolved_deps.insert("other_pkg/Child".to_string(), child);

        let msg = ParsedMessage {
            name: "Parent".to_string(),
            package: "test_msgs".to_string(),
            fields: vec![
                Field {
                    name: "child".to_string(),
                    field_type: CodegenFieldType {
                        base_type: "Child".to_string(),
                        package: Some("other_pkg".to_string()),
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
                Field {
                    name: "value".to_string(),
                    field_type: CodegenFieldType {
                        base_type: "int32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
            ],
            constants: vec![],
            source: "other_pkg/Child child\nint32 value".to_string(),
            path: PathBuf::new(),
        };

        let result = build_type_description_msg(&msg, &resolved_deps).unwrap();

        assert_eq!(result.type_description.type_name, "test_msgs/msg/Parent");
        assert_eq!(result.type_description.fields.len(), 2);
        assert_eq!(result.referenced_type_descriptions.len(), 1);
        assert_eq!(
            result.referenced_type_descriptions[0].type_name,
            "other_pkg/msg/Child"
        );
    }
}
