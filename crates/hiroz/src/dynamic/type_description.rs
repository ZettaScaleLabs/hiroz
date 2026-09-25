//! Conversion between MessageSchema and ROS 2 TypeDescription.
//!
//! This module provides bidirectional conversion between the runtime
//! `MessageSchema` type used for dynamic messages and the wire-format
//! `TypeDescription` types defined in `hiroz-schema`.
//!
//! # Usage
//!
//! ```rust,ignore
//! use hiroz::dynamic::{MessageSchema, FieldType};
//! use hiroz_schema::{TypeDescriptionMsg, calculate_hash};
//!
//! // Create a schema programmatically
//! let schema = MessageSchema::builder("geometry_msgs/msg/Point")
//!     .field("x", FieldType::Float64)
//!     .field("y", FieldType::Float64)
//!     .field("z", FieldType::Float64)
//!     .build()?;
//!
//! // Convert to TypeDescription for wire format or hashing
//! let type_desc_msg = schema.to_type_description_msg()?;
//!
//! // Calculate type hash
//! let hash = calculate_hash(&type_desc_msg);
//! println!("Type hash: {}", hash.to_rihs_string());
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use hiroz_schema::{
    FieldDescription, FieldTypeDescription, TypeDescription, TypeDescriptionMsg, TypeHash, TypeId,
    calculate_hash,
};

use super::error::DynamicError;
use super::schema::{FieldSchema, FieldType, MessageSchema};

/// Extension trait for MessageSchema to support TypeDescription conversion.
pub trait MessageSchemaTypeDescription {
    /// Convert this schema to a TypeDescriptionMsg (includes referenced types).
    fn to_type_description_msg(&self) -> Result<TypeDescriptionMsg, DynamicError>;

    /// Convert this schema to a single TypeDescription (without referenced types).
    fn to_type_description(&self) -> Result<TypeDescription, DynamicError>;

    /// Calculate the RIHS01 type hash for this schema.
    fn compute_type_hash(&self) -> Result<TypeHash, DynamicError>;
}

impl MessageSchemaTypeDescription for MessageSchema {
    fn to_type_description_msg(&self) -> Result<TypeDescriptionMsg, DynamicError> {
        let mut referenced = Vec::new();
        let mut visited = HashMap::new();

        // First, collect all referenced type descriptions
        collect_referenced_types(self, &mut referenced, &mut visited)?;

        // Sort referenced types alphabetically by type name (required for hash consistency)
        referenced.sort_by(|a, b| a.type_name.cmp(&b.type_name));

        Ok(TypeDescriptionMsg {
            type_description: self.to_type_description()?,
            referenced_type_descriptions: referenced,
        })
    }

    fn to_type_description(&self) -> Result<TypeDescription, DynamicError> {
        let fields = self
            .fields
            .iter()
            .map(field_schema_to_description)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(TypeDescription {
            type_name: self.type_name.clone(),
            fields,
        })
    }

    fn compute_type_hash(&self) -> Result<TypeHash, DynamicError> {
        let msg = self.to_type_description_msg()?;
        Ok(calculate_hash(&msg))
    }
}

/// Collect all referenced (nested) type descriptions recursively.
fn collect_referenced_types(
    schema: &MessageSchema,
    referenced: &mut Vec<TypeDescription>,
    visited: &mut HashMap<String, bool>,
) -> Result<(), DynamicError> {
    for field in &schema.fields {
        collect_field_type_references(&field.field_type, referenced, visited)?;
    }
    Ok(())
}

/// Recursively collect references from a field type.
fn collect_field_type_references(
    field_type: &FieldType,
    referenced: &mut Vec<TypeDescription>,
    visited: &mut HashMap<String, bool>,
) -> Result<(), DynamicError> {
    match field_type {
        FieldType::Message(nested_schema) if !visited.contains_key(&nested_schema.type_name) => {
            visited.insert(nested_schema.type_name.clone(), true);

            // First collect this type's nested references
            collect_referenced_types(nested_schema, referenced, visited)?;

            // Then add this type's description
            let td = MessageSchemaTypeDescription::to_type_description(nested_schema.as_ref())?;
            referenced.push(td);
        }
        FieldType::Message(_) => {}
        FieldType::Array(inner, _)
        | FieldType::Sequence(inner)
        | FieldType::BoundedSequence(inner, _) => {
            collect_field_type_references(inner, referenced, visited)?;
        }
        _ => {} // Primitives have no references
    }
    Ok(())
}

/// Convert a FieldSchema to a FieldDescription.
fn field_schema_to_description(field: &FieldSchema) -> Result<FieldDescription, DynamicError> {
    Ok(FieldDescription {
        name: field.name.clone(),
        field_type: field_type_to_description(&field.field_type)?,
        default_value: String::new(), // Default values not included in hash
    })
}

/// Convert a dynamic FieldType to a FieldTypeDescription.
fn field_type_to_description(field_type: &FieldType) -> Result<FieldTypeDescription, DynamicError> {
    match field_type {
        // Primitives
        FieldType::Bool => Ok(FieldTypeDescription::primitive(TypeId::BOOL)),
        FieldType::Int8 => Ok(FieldTypeDescription::primitive(TypeId::INT8)),
        FieldType::Int16 => Ok(FieldTypeDescription::primitive(TypeId::INT16)),
        FieldType::Int32 => Ok(FieldTypeDescription::primitive(TypeId::INT32)),
        FieldType::Int64 => Ok(FieldTypeDescription::primitive(TypeId::INT64)),
        FieldType::Uint8 => Ok(FieldTypeDescription::primitive(TypeId::UINT8)),
        FieldType::Char => Ok(FieldTypeDescription::primitive(TypeId::CHAR)),
        FieldType::WChar => Ok(FieldTypeDescription::primitive(TypeId::WCHAR)),
        FieldType::Byte => Ok(FieldTypeDescription::primitive(TypeId::BYTE)),
        FieldType::Uint16 => Ok(FieldTypeDescription::primitive(TypeId::UINT16)),
        FieldType::Uint32 => Ok(FieldTypeDescription::primitive(TypeId::UINT32)),
        FieldType::Uint64 => Ok(FieldTypeDescription::primitive(TypeId::UINT64)),
        FieldType::Float32 => Ok(FieldTypeDescription::primitive(TypeId::FLOAT32)),
        FieldType::Float64 => Ok(FieldTypeDescription::primitive(TypeId::FLOAT64)),
        FieldType::String => Ok(FieldTypeDescription::primitive(TypeId::STRING)),
        FieldType::WString => Ok(FieldTypeDescription::primitive(TypeId::WSTRING)),

        // Bounded string
        FieldType::BoundedString(capacity) => Ok(FieldTypeDescription {
            type_id: TypeId::BOUNDED_STRING,
            capacity: 0,
            string_capacity: *capacity as u64,
            nested_type_name: String::new(),
        }),
        FieldType::BoundedWString(capacity) => Ok(FieldTypeDescription {
            type_id: TypeId::BOUNDED_WSTRING,
            capacity: 0,
            string_capacity: *capacity as u64,
            nested_type_name: String::new(),
        }),

        // Nested message (single)
        FieldType::Message(schema) => Ok(FieldTypeDescription::nested(
            TypeId::NESTED_TYPE,
            &schema.type_name,
        )),

        // Fixed-size array
        FieldType::Array(inner, size) => {
            let base_type_id = get_base_type_id(inner)?;
            let array_type_id = base_type_id + TypeId::ARRAY_OFFSET;

            let mut description = if let FieldType::Message(schema) = inner.as_ref() {
                FieldTypeDescription::nested_array(array_type_id, *size as u64, &schema.type_name)
            } else {
                FieldTypeDescription::array(array_type_id, *size as u64)
            };
            description.string_capacity = bounded_string_capacity(inner);
            Ok(description)
        }

        // Unbounded sequence
        FieldType::Sequence(inner) => {
            let base_type_id = get_base_type_id(inner)?;
            let seq_type_id = base_type_id + TypeId::UNBOUNDED_SEQUENCE_OFFSET;

            if let FieldType::Message(schema) = inner.as_ref() {
                Ok(FieldTypeDescription::nested(seq_type_id, &schema.type_name))
            } else {
                let mut description = FieldTypeDescription::primitive(seq_type_id);
                description.string_capacity = bounded_string_capacity(inner);
                Ok(description)
            }
        }

        // Bounded sequence
        FieldType::BoundedSequence(inner, capacity) => {
            let base_type_id = get_base_type_id(inner)?;
            let seq_type_id = base_type_id + TypeId::BOUNDED_SEQUENCE_OFFSET;

            let mut description = if let FieldType::Message(schema) = inner.as_ref() {
                FieldTypeDescription::nested_array(seq_type_id, *capacity as u64, &schema.type_name)
            } else {
                FieldTypeDescription::array(seq_type_id, *capacity as u64)
            };
            description.string_capacity = bounded_string_capacity(inner);
            Ok(description)
        }
    }
}

/// Get the base type ID for a field type (without array/sequence modifiers).
fn get_base_type_id(field_type: &FieldType) -> Result<u8, DynamicError> {
    match field_type {
        FieldType::Bool => Ok(TypeId::BOOL),
        FieldType::Int8 => Ok(TypeId::INT8),
        FieldType::Int16 => Ok(TypeId::INT16),
        FieldType::Int32 => Ok(TypeId::INT32),
        FieldType::Int64 => Ok(TypeId::INT64),
        FieldType::Uint8 => Ok(TypeId::UINT8),
        FieldType::Char => Ok(TypeId::CHAR),
        FieldType::WChar => Ok(TypeId::WCHAR),
        FieldType::Byte => Ok(TypeId::BYTE),
        FieldType::Uint16 => Ok(TypeId::UINT16),
        FieldType::Uint32 => Ok(TypeId::UINT32),
        FieldType::Uint64 => Ok(TypeId::UINT64),
        FieldType::Float32 => Ok(TypeId::FLOAT32),
        FieldType::Float64 => Ok(TypeId::FLOAT64),
        FieldType::String => Ok(TypeId::STRING),
        FieldType::WString => Ok(TypeId::WSTRING),
        FieldType::BoundedString(_) => Ok(TypeId::BOUNDED_STRING),
        FieldType::BoundedWString(_) => Ok(TypeId::BOUNDED_WSTRING),
        FieldType::Message(_) => Ok(TypeId::NESTED_TYPE),
        // For nested arrays/sequences, we get the innermost type
        FieldType::Array(inner, _)
        | FieldType::Sequence(inner)
        | FieldType::BoundedSequence(inner, _) => get_base_type_id(inner),
    }
}

fn bounded_string_capacity(field_type: &FieldType) -> u64 {
    match field_type {
        FieldType::BoundedString(capacity) | FieldType::BoundedWString(capacity) => {
            *capacity as u64
        }
        _ => 0,
    }
}

/// Convert a TypeDescriptionMsg back to a MessageSchema.
pub fn type_description_msg_to_schema(
    msg: &TypeDescriptionMsg,
) -> Result<Arc<MessageSchema>, DynamicError> {
    // Build a map of all referenced types
    let mut type_map: HashMap<String, Arc<MessageSchema>> = HashMap::new();

    // First pass: create all schemas without resolving nested types
    for ref_td in &msg.referenced_type_descriptions {
        let schema = type_description_to_schema_partial(ref_td)?;
        type_map.insert(ref_td.type_name.clone(), Arc::new(schema));
    }

    // Second pass: resolve nested types in referenced schemas
    // Process in dependency order: iterate until all are resolved
    let mut resolved_map: HashMap<String, Arc<MessageSchema>> = HashMap::new();
    let mut remaining: Vec<&TypeDescription> = msg.referenced_type_descriptions.iter().collect();

    while !remaining.is_empty() {
        let mut made_progress = false;
        remaining.retain(|ref_td| {
            // Try to build with current resolved_map
            match type_description_to_schema_full(ref_td, &resolved_map) {
                Ok(schema) => {
                    resolved_map.insert(ref_td.type_name.clone(), Arc::new(schema));
                    made_progress = true;
                    false // Remove from remaining
                }
                Err(_) => true, // Keep in remaining, dependencies not ready yet
            }
        });

        if !made_progress && !remaining.is_empty() {
            // Circular dependency or missing type
            return Err(DynamicError::SerializationError(format!(
                "Cannot resolve dependencies for types: {}",
                remaining
                    .iter()
                    .map(|t| t.type_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }

    // Build the main schema with resolved references
    type_description_to_schema_full(&msg.type_description, &resolved_map).map(Arc::new)
}

/// Create a partial MessageSchema without resolving nested types.
fn type_description_to_schema_partial(td: &TypeDescription) -> Result<MessageSchema, DynamicError> {
    let parts: Vec<&str> = td.type_name.split('/').collect();
    if parts.len() != 3 {
        return Err(DynamicError::InvalidTypeName(td.type_name.clone()));
    }

    Ok(MessageSchema {
        type_name: td.type_name.clone(),
        package: parts[0].to_string(),
        name: parts[2].to_string(),
        fields: Vec::new(), // Will be filled later
        type_hash: None,
    })
}

/// Create a full MessageSchema with resolved nested types.
fn type_description_to_schema_full(
    td: &TypeDescription,
    type_map: &HashMap<String, Arc<MessageSchema>>,
) -> Result<MessageSchema, DynamicError> {
    let parts: Vec<&str> = td.type_name.split('/').collect();
    if parts.len() != 3 {
        return Err(DynamicError::InvalidTypeName(td.type_name.clone()));
    }

    let fields = td
        .fields
        .iter()
        .map(|fd| field_description_to_schema(fd, type_map))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(MessageSchema {
        type_name: td.type_name.clone(),
        package: parts[0].to_string(),
        name: parts[2].to_string(),
        fields,
        type_hash: None,
    })
}

/// Convert a FieldDescription back to a FieldSchema.
fn field_description_to_schema(
    fd: &FieldDescription,
    type_map: &HashMap<String, Arc<MessageSchema>>,
) -> Result<FieldSchema, DynamicError> {
    let field_type = field_type_description_to_type(&fd.field_type, type_map)?;
    Ok(FieldSchema {
        name: fd.name.clone(),
        field_type,
        default_value: None,
    })
}

/// Convert a FieldTypeDescription back to a FieldType.
fn field_type_description_to_type(
    ftd: &FieldTypeDescription,
    type_map: &HashMap<String, Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    let base_type_id = TypeId::base_type(ftd.type_id);
    let is_array = TypeId::is_array(ftd.type_id);
    let is_bounded_seq = TypeId::is_bounded_sequence(ftd.type_id);
    let is_unbounded_seq = TypeId::is_unbounded_sequence(ftd.type_id);

    // Get the base field type
    let base_type = match base_type_id {
        TypeId::BOOL => FieldType::Bool,
        TypeId::INT8 => FieldType::Int8,
        TypeId::INT16 => FieldType::Int16,
        TypeId::INT32 => FieldType::Int32,
        TypeId::INT64 => FieldType::Int64,
        TypeId::UINT8 => FieldType::Uint8,
        TypeId::CHAR => FieldType::Char,
        TypeId::WCHAR => FieldType::WChar,
        TypeId::BYTE => FieldType::Byte,
        TypeId::UINT16 => FieldType::Uint16,
        TypeId::UINT32 => FieldType::Uint32,
        TypeId::UINT64 => FieldType::Uint64,
        TypeId::FLOAT32 => FieldType::Float32,
        TypeId::FLOAT64 => FieldType::Float64,
        TypeId::STRING => FieldType::String,
        TypeId::WSTRING => FieldType::WString,
        TypeId::BOUNDED_STRING => FieldType::BoundedString(ftd.string_capacity as usize),
        TypeId::BOUNDED_WSTRING => FieldType::BoundedWString(ftd.string_capacity as usize),
        TypeId::NESTED_TYPE => {
            let schema = type_map.get(&ftd.nested_type_name).ok_or_else(|| {
                DynamicError::FieldNotFound(format!(
                    "Referenced type not found: {}",
                    ftd.nested_type_name
                ))
            })?;
            FieldType::Message(schema.clone())
        }
        _ => {
            return Err(DynamicError::SerializationError(format!(
                "Unknown type ID: {}",
                base_type_id
            )));
        }
    };

    // Apply array/sequence modifiers
    if is_array {
        Ok(FieldType::Array(Box::new(base_type), ftd.capacity as usize))
    } else if is_bounded_seq {
        Ok(FieldType::BoundedSequence(
            Box::new(base_type),
            ftd.capacity as usize,
        ))
    } else if is_unbounded_seq {
        Ok(FieldType::Sequence(Box::new(base_type)))
    } else {
        Ok(base_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_type_to_description() {
        let schema = MessageSchema::builder("std_msgs/msg/Int32")
            .field("data", FieldType::Int32)
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(td.type_name, "std_msgs/msg/Int32");
        assert_eq!(td.fields.len(), 1);
        assert_eq!(td.fields[0].name, "data");
        assert_eq!(td.fields[0].field_type.type_id, TypeId::INT32);
    }

    #[test]
    fn test_string_type_to_description() {
        let schema = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(td.fields[0].field_type.type_id, TypeId::STRING);
    }

    #[test]
    fn test_array_type_to_description() {
        let schema = MessageSchema::builder("test_msgs/msg/ArrayTest")
            .field("data", FieldType::Array(Box::new(FieldType::Float64), 3))
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(td.fields[0].field_type.type_id, TypeId::FLOAT64_ARRAY);
        assert_eq!(td.fields[0].field_type.capacity, 3);
    }

    #[test]
    fn test_sequence_type_to_description() {
        let schema = MessageSchema::builder("test_msgs/msg/SeqTest")
            .field("data", FieldType::Sequence(Box::new(FieldType::Int32)))
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(
            td.fields[0].field_type.type_id,
            TypeId::INT32_UNBOUNDED_SEQUENCE
        );
    }

    #[test]
    fn test_nested_message_to_description() {
        let vector3_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let twist_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
            .field("linear", FieldType::Message(vector3_schema.clone()))
            .field("angular", FieldType::Message(vector3_schema))
            .build()
            .unwrap();

        let msg = twist_schema.to_type_description_msg().unwrap();
        assert_eq!(msg.type_description.type_name, "geometry_msgs/msg/Twist");
        assert_eq!(msg.referenced_type_descriptions.len(), 1); // Only Vector3
        assert_eq!(
            msg.referenced_type_descriptions[0].type_name,
            "geometry_msgs/msg/Vector3"
        );

        // Verify nested type fields
        assert_eq!(msg.type_description.fields[0].name, "linear");
        assert_eq!(
            msg.type_description.fields[0].field_type.type_id,
            TypeId::NESTED_TYPE
        );
        assert_eq!(
            msg.type_description.fields[0].field_type.nested_type_name,
            "geometry_msgs/msg/Vector3"
        );
    }

    #[test]
    fn test_type_hash_computation() {
        let schema = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let hash = schema.compute_type_hash().unwrap();
        let rihs = hash.to_rihs_string();
        assert!(rihs.starts_with("RIHS01_"));
        assert_eq!(rihs.len(), 7 + 64); // RIHS01_ + 64 hex chars
    }

    #[test]
    fn test_roundtrip_conversion() {
        let original = MessageSchema::builder("geometry_msgs/msg/Point")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let msg = original.to_type_description_msg().unwrap();
        let restored = type_description_msg_to_schema(&msg).unwrap();

        assert_eq!(original.type_name, restored.type_name);
        assert_eq!(original.fields.len(), restored.fields.len());

        for (orig_field, restored_field) in original.fields.iter().zip(restored.fields.iter()) {
            assert_eq!(orig_field.name, restored_field.name);
        }
    }

    #[test]
    fn test_nested_roundtrip() {
        let vector3_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let twist_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
            .field("linear", FieldType::Message(vector3_schema.clone()))
            .field("angular", FieldType::Message(vector3_schema))
            .build()
            .unwrap();

        let msg = twist_schema.to_type_description_msg().unwrap();
        let restored = type_description_msg_to_schema(&msg).unwrap();

        assert_eq!(twist_schema.type_name, restored.type_name);
        assert_eq!(twist_schema.fields.len(), restored.fields.len());

        // Verify nested fields
        if let FieldType::Message(nested) = &restored.fields[0].field_type {
            assert_eq!(nested.type_name, "geometry_msgs/msg/Vector3");
            assert_eq!(nested.fields.len(), 3);
        } else {
            panic!("Expected Message type for linear field");
        }
    }

    #[test]
    fn test_all_primitive_types() {
        let schema = MessageSchema::builder("test_msgs/msg/AllPrimitives")
            .field("bool_val", FieldType::Bool)
            .field("int8_val", FieldType::Int8)
            .field("int16_val", FieldType::Int16)
            .field("int32_val", FieldType::Int32)
            .field("int64_val", FieldType::Int64)
            .field("uint8_val", FieldType::Uint8)
            .field("uint16_val", FieldType::Uint16)
            .field("uint32_val", FieldType::Uint32)
            .field("uint64_val", FieldType::Uint64)
            .field("float32_val", FieldType::Float32)
            .field("float64_val", FieldType::Float64)
            .field("string_val", FieldType::String)
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(td.fields.len(), 12);

        // Verify type IDs
        assert_eq!(td.fields[0].field_type.type_id, TypeId::BOOL);
        assert_eq!(td.fields[1].field_type.type_id, TypeId::INT8);
        assert_eq!(td.fields[2].field_type.type_id, TypeId::INT16);
        assert_eq!(td.fields[3].field_type.type_id, TypeId::INT32);
        assert_eq!(td.fields[4].field_type.type_id, TypeId::INT64);
        assert_eq!(td.fields[5].field_type.type_id, TypeId::UINT8);
        assert_eq!(td.fields[6].field_type.type_id, TypeId::UINT16);
        assert_eq!(td.fields[7].field_type.type_id, TypeId::UINT32);
        assert_eq!(td.fields[8].field_type.type_id, TypeId::UINT64);
        assert_eq!(td.fields[9].field_type.type_id, TypeId::FLOAT32);
        assert_eq!(td.fields[10].field_type.type_id, TypeId::FLOAT64);
        assert_eq!(td.fields[11].field_type.type_id, TypeId::STRING);
    }

    #[test]
    fn test_bounded_string() {
        let schema = MessageSchema::builder("test_msgs/msg/BoundedString")
            .field("bounded", FieldType::BoundedString(256))
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(td.fields[0].field_type.type_id, TypeId::BOUNDED_STRING);
        assert_eq!(td.fields[0].field_type.string_capacity, 256);
    }

    #[test]
    fn test_byte_and_bounded_string_collection_descriptions_roundtrip() {
        let schema = MessageSchema::builder("test_msgs/msg/ScalarAliases")
            .field("byte_value", FieldType::Byte)
            .field("native_char", FieldType::Char)
            .field("bytes", FieldType::Array(Box::new(FieldType::Byte), 3))
            .field("bounded", FieldType::BoundedString(12))
            .field(
                "bounded_values",
                FieldType::BoundedSequence(Box::new(FieldType::BoundedString(12)), 4),
            )
            .build()
            .unwrap();

        let description = schema.to_type_description().unwrap();
        assert_eq!(description.fields[0].field_type.type_id, TypeId::BYTE);
        assert_eq!(description.fields[1].field_type.type_id, TypeId::CHAR);
        assert_eq!(description.fields[2].field_type.type_id, TypeId::BYTE_ARRAY);
        assert_eq!(
            description.fields[3].field_type.type_id,
            TypeId::BOUNDED_STRING
        );
        assert_eq!(description.fields[3].field_type.string_capacity, 12);
        assert_eq!(
            description.fields[4].field_type.type_id,
            TypeId::BOUNDED_STRING + TypeId::BOUNDED_SEQUENCE_OFFSET
        );
        assert_eq!(description.fields[4].field_type.string_capacity, 12);

        let msg = schema.to_type_description_msg().unwrap();
        let restored = type_description_msg_to_schema(&msg).unwrap();
        assert_eq!(restored.fields[0].field_type, FieldType::Byte);
        assert_eq!(restored.fields[1].field_type, FieldType::Char);
        assert_eq!(restored.fields[3].field_type, FieldType::BoundedString(12));
        assert_eq!(
            restored.fields[4].field_type,
            FieldType::BoundedSequence(Box::new(FieldType::BoundedString(12)), 4)
        );
    }

    #[test]
    fn test_byte_hash_matches_ros_fixture() {
        let schema = MessageSchema::builder("example_interfaces/msg/Byte")
            .field("data", FieldType::Byte)
            .build()
            .unwrap();

        assert_eq!(
            schema.compute_type_hash().unwrap().to_rihs_string(),
            "RIHS01_f014e0424be54b8ba7c35490aea4198be92df1de4e88f4e19a2fbbce2e020bb9"
        );
    }

    #[test]
    fn test_wide_string_descriptions_and_collections_roundtrip() {
        let schema = MessageSchema::builder("test_msgs/msg/WideTypes")
            .field("native_wchar", FieldType::WChar)
            .field("wide", FieldType::WString)
            .field("bounded_wide", FieldType::BoundedWString(6))
            .field(
                "wide_array",
                FieldType::Array(Box::new(FieldType::WString), 3),
            )
            .field(
                "wide_values",
                FieldType::Sequence(Box::new(FieldType::WString)),
            )
            .field(
                "bounded_wide_values",
                FieldType::BoundedSequence(Box::new(FieldType::BoundedWString(6)), 4),
            )
            .build()
            .unwrap();

        let description = schema.to_type_description().unwrap();
        assert_eq!(description.fields[0].field_type.type_id, TypeId::WCHAR);
        assert_eq!(description.fields[1].field_type.type_id, TypeId::WSTRING);
        assert_eq!(
            description.fields[2].field_type.type_id,
            TypeId::BOUNDED_WSTRING
        );
        assert_eq!(description.fields[2].field_type.string_capacity, 6);
        assert_eq!(
            description.fields[3].field_type.type_id,
            TypeId::WSTRING_ARRAY
        );
        assert_eq!(description.fields[3].field_type.capacity, 3);
        assert_eq!(
            description.fields[4].field_type.type_id,
            TypeId::WSTRING_UNBOUNDED_SEQUENCE
        );
        assert_eq!(
            description.fields[5].field_type.type_id,
            TypeId::BOUNDED_WSTRING_BOUNDED_SEQUENCE
        );
        assert_eq!(description.fields[5].field_type.capacity, 4);
        assert_eq!(description.fields[5].field_type.string_capacity, 6);

        let restored =
            type_description_msg_to_schema(&schema.to_type_description_msg().unwrap()).unwrap();
        assert_eq!(restored.fields.len(), schema.fields.len());
        for (restored, original) in restored.fields.iter().zip(&schema.fields) {
            assert_eq!(restored.name, original.name);
            assert_eq!(restored.field_type, original.field_type);
        }
    }

    #[test]
    fn test_wstring_hash_matches_ros_fixture() {
        let schema = MessageSchema::builder("example_interfaces/msg/WString")
            .field("data", FieldType::WString)
            .build()
            .unwrap();

        assert_eq!(
            schema.compute_type_hash().unwrap().to_rihs_string(),
            "RIHS01_32033e06d9dfe5468c5d6e1dc8b7a23c8910bad071cfd4e151a951d580e68dd8"
        );
    }

    #[test]
    fn test_standard_byte_and_legacy_char_hashes() {
        let byte = MessageSchema::builder("std_msgs/msg/Byte")
            .field("data", FieldType::Byte)
            .build()
            .unwrap();
        assert_eq!(
            byte.compute_type_hash().unwrap().to_rihs_string(),
            "RIHS01_41e1a3345f73fe93ede006da826a6ee274af23dd4653976ff249b0f44e3e798f"
        );

        let legacy_char = MessageSchema::builder("std_msgs/msg/Char")
            .field("data", FieldType::Uint8)
            .build()
            .unwrap();
        assert_eq!(
            legacy_char.compute_type_hash().unwrap().to_rihs_string(),
            "RIHS01_3ad2d04dd29ba19d04b16659afa3ccaedd691914b02a64e82e252f2fa6a586a9"
        );
    }

    #[test]
    fn test_bounded_sequence() {
        let schema = MessageSchema::builder("test_msgs/msg/BoundedSeq")
            .field(
                "data",
                FieldType::BoundedSequence(Box::new(FieldType::Uint8), 100),
            )
            .build()
            .unwrap();

        let td = schema.to_type_description().unwrap();
        assert_eq!(
            td.fields[0].field_type.type_id,
            TypeId::UINT8_BOUNDED_SEQUENCE
        );
        assert_eq!(td.fields[0].field_type.capacity, 100);
    }

    #[test]
    fn test_nested_array() {
        let point_schema = MessageSchema::builder("geometry_msgs/msg/Point")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let schema = MessageSchema::builder("test_msgs/msg/PointArray")
            .field(
                "points",
                FieldType::Array(Box::new(FieldType::Message(point_schema)), 10),
            )
            .build()
            .unwrap();

        let msg = schema.to_type_description_msg().unwrap();
        assert_eq!(
            msg.type_description.fields[0].field_type.type_id,
            TypeId::NESTED_TYPE_ARRAY
        );
        assert_eq!(msg.type_description.fields[0].field_type.capacity, 10);
        assert_eq!(
            msg.type_description.fields[0].field_type.nested_type_name,
            "geometry_msgs/msg/Point"
        );
        assert_eq!(msg.referenced_type_descriptions.len(), 1);
    }

    #[test]
    fn test_nested_unbounded_sequence() {
        let point_schema = MessageSchema::builder("geometry_msgs/msg/Point")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let schema = MessageSchema::builder("test_msgs/msg/PointList")
            .field(
                "points",
                FieldType::Sequence(Box::new(FieldType::Message(point_schema))),
            )
            .build()
            .unwrap();

        let msg = schema.to_type_description_msg().unwrap();
        assert_eq!(
            msg.type_description.fields[0].field_type.type_id,
            TypeId::NESTED_TYPE_UNBOUNDED_SEQUENCE
        );
        assert_eq!(
            msg.type_description.fields[0].field_type.nested_type_name,
            "geometry_msgs/msg/Point"
        );
    }

    #[test]
    fn test_deeply_nested_three_levels() {
        // Inner -> Middle -> Outer
        let inner_schema = MessageSchema::builder("test_msgs/msg/Inner")
            .field("value", FieldType::Float64)
            .build()
            .unwrap();

        let middle_schema = MessageSchema::builder("test_msgs/msg/Middle")
            .field("inner", FieldType::Message(inner_schema))
            .build()
            .unwrap();

        let outer_schema = MessageSchema::builder("test_msgs/msg/Outer")
            .field("middle", FieldType::Message(middle_schema))
            .build()
            .unwrap();

        let msg = outer_schema.to_type_description_msg().unwrap();

        // Should have 2 referenced types: Inner and Middle
        assert_eq!(msg.referenced_type_descriptions.len(), 2);

        // Referenced types should be sorted alphabetically
        let ref_names: Vec<&str> = msg
            .referenced_type_descriptions
            .iter()
            .map(|td| td.type_name.as_str())
            .collect();
        assert!(ref_names.contains(&"test_msgs/msg/Inner"));
        assert!(ref_names.contains(&"test_msgs/msg/Middle"));
    }

    #[test]
    fn test_hash_deterministic_and_valid() {
        // std_msgs/msg/String - verify hash is deterministic and well-formed
        // Note: To verify against ROS 2, run: ros2 interface show std_msgs/msg/String --show-hash
        let schema = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let hash1 = schema.compute_type_hash().unwrap();
        let hash2 = schema.compute_type_hash().unwrap();

        // Hash should be deterministic
        assert_eq!(hash1, hash2);

        // Hash should be valid RIHS01 format
        let rihs = hash1.to_rihs_string();
        assert!(rihs.starts_with("RIHS01_"));
        assert_eq!(rihs.len(), 7 + 64);

        // Verify our computed hash (for regression testing)
        assert_eq!(
            rihs,
            "RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18"
        );
    }

    #[test]
    fn test_roundtrip_with_arrays() {
        let original = MessageSchema::builder("test_msgs/msg/Arrays")
            .field("fixed", FieldType::Array(Box::new(FieldType::Int32), 5))
            .field(
                "unbounded",
                FieldType::Sequence(Box::new(FieldType::Float64)),
            )
            .field(
                "bounded",
                FieldType::BoundedSequence(Box::new(FieldType::Uint8), 100),
            )
            .build()
            .unwrap();

        let msg = original.to_type_description_msg().unwrap();
        let restored = type_description_msg_to_schema(&msg).unwrap();

        assert_eq!(original.type_name, restored.type_name);
        assert_eq!(original.fields.len(), restored.fields.len());

        // Verify array types
        if let FieldType::Array(inner, size) = &restored.fields[0].field_type {
            assert!(matches!(inner.as_ref(), FieldType::Int32));
            assert_eq!(*size, 5);
        } else {
            panic!("Expected Array type");
        }

        if let FieldType::Sequence(inner) = &restored.fields[1].field_type {
            assert!(matches!(inner.as_ref(), FieldType::Float64));
        } else {
            panic!("Expected Sequence type");
        }

        if let FieldType::BoundedSequence(inner, cap) = &restored.fields[2].field_type {
            assert!(matches!(inner.as_ref(), FieldType::Uint8));
            assert_eq!(*cap, 100);
        } else {
            panic!("Expected BoundedSequence type");
        }
    }

    #[test]
    fn test_referenced_types_sorted() {
        // Create schema with multiple nested types to verify sorting
        let type_c = MessageSchema::builder("pkg/msg/TypeC")
            .field("c", FieldType::Int32)
            .build()
            .unwrap();

        let type_a = MessageSchema::builder("pkg/msg/TypeA")
            .field("a", FieldType::Int32)
            .build()
            .unwrap();

        let type_b = MessageSchema::builder("pkg/msg/TypeB")
            .field("b", FieldType::Int32)
            .build()
            .unwrap();

        // Add in non-alphabetical order
        let main = MessageSchema::builder("pkg/msg/Main")
            .field("c", FieldType::Message(type_c))
            .field("a", FieldType::Message(type_a))
            .field("b", FieldType::Message(type_b))
            .build()
            .unwrap();

        let msg = main.to_type_description_msg().unwrap();

        // Referenced types should be sorted alphabetically
        let ref_names: Vec<&str> = msg
            .referenced_type_descriptions
            .iter()
            .map(|td| td.type_name.as_str())
            .collect();

        assert_eq!(ref_names[0], "pkg/msg/TypeA");
        assert_eq!(ref_names[1], "pkg/msg/TypeB");
        assert_eq!(ref_names[2], "pkg/msg/TypeC");
    }

    #[test]
    fn test_duplicate_nested_types_deduplicated() {
        // Same nested type used multiple times should only appear once
        let point_schema = MessageSchema::builder("geometry_msgs/msg/Point")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let schema = MessageSchema::builder("test_msgs/msg/Triangle")
            .field("p1", FieldType::Message(point_schema.clone()))
            .field("p2", FieldType::Message(point_schema.clone()))
            .field("p3", FieldType::Message(point_schema))
            .build()
            .unwrap();

        let msg = schema.to_type_description_msg().unwrap();

        // Point should only appear once in referenced types
        assert_eq!(msg.referenced_type_descriptions.len(), 1);
        assert_eq!(
            msg.referenced_type_descriptions[0].type_name,
            "geometry_msgs/msg/Point"
        );
    }
}
