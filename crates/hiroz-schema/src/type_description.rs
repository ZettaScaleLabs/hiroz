//! ROS 2 TypeDescription types
//!
//! These types match the ROS 2 `type_description_interfaces` package exactly,
//! enabling wire-format compatibility for type description services.

use serde::{Deserialize, Serialize};

/// ROS2 TypeDescription message structure
///
/// This is the top-level type description containing the main type
/// and all referenced (nested) type descriptions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TypeDescriptionMsg {
    /// Description of the main type
    pub type_description: TypeDescription,
    /// Descriptions of all referenced types (nested messages)
    pub referenced_type_descriptions: Vec<TypeDescription>,
}

/// Type description for a single message type
///
/// Matches `type_description_interfaces/msg/IndividualTypeDescription`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TypeDescription {
    /// Fully qualified type name (e.g., "std_msgs/msg/String")
    pub type_name: String,
    /// Fields in this type
    pub fields: Vec<FieldDescription>,
}

/// Field description in a type
///
/// Matches `type_description_interfaces/msg/Field`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDescription {
    /// Field name
    pub name: String,
    /// Field type information
    #[serde(rename = "type")]
    pub field_type: FieldTypeDescription,
    /// Default value (empty string for RIHS01 hash computation)
    #[serde(default)]
    pub default_value: String,
}

/// Field type description with type ID
///
/// Matches `type_description_interfaces/msg/FieldType`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FieldTypeDescription {
    /// Type ID (1-166, see `TypeId` constants)
    pub type_id: u8,
    /// Array/sequence capacity (0 for single values and unbounded sequences)
    pub capacity: u64,
    /// String capacity for bounded strings (0 otherwise)
    pub string_capacity: u64,
    /// Nested type name (empty for primitives)
    pub nested_type_name: String,
}

impl FieldTypeDescription {
    /// Create a primitive type description
    pub fn primitive(type_id: u8) -> Self {
        Self {
            type_id,
            capacity: 0,
            string_capacity: 0,
            nested_type_name: String::new(),
        }
    }

    /// Create a nested type description
    pub fn nested(type_id: u8, nested_type_name: impl Into<String>) -> Self {
        Self {
            type_id,
            capacity: 0,
            string_capacity: 0,
            nested_type_name: nested_type_name.into(),
        }
    }

    /// Create an array type description
    pub fn array(type_id: u8, capacity: u64) -> Self {
        Self {
            type_id,
            capacity,
            string_capacity: 0,
            nested_type_name: String::new(),
        }
    }

    /// Create a nested array type description
    pub fn nested_array(type_id: u8, capacity: u64, nested_type_name: impl Into<String>) -> Self {
        Self {
            type_id,
            capacity,
            string_capacity: 0,
            nested_type_name: nested_type_name.into(),
        }
    }
}

/// Field description for hash computation (excludes default_value)
#[derive(Serialize)]
pub struct FieldDescriptionForHash<'a> {
    /// Field name
    pub name: &'a str,
    /// Field type
    #[serde(rename = "type")]
    pub field_type: &'a FieldTypeDescription,
}

/// Type description for hash computation
#[derive(Serialize)]
pub struct TypeDescriptionForHash<'a> {
    /// Type name
    pub type_name: &'a str,
    /// Fields
    pub fields: Vec<FieldDescriptionForHash<'a>>,
}

/// TypeDescriptionMsg for hash computation
#[derive(Serialize)]
pub struct TypeDescriptionMsgForHash<'a> {
    /// Main type description
    pub type_description: TypeDescriptionForHash<'a>,
    /// Referenced type descriptions
    pub referenced_type_descriptions: Vec<TypeDescriptionForHash<'a>>,
}

/// Convert TypeDescriptionMsg to hash-computation version (excludes default_value)
pub fn to_hash_version(msg: &TypeDescriptionMsg) -> TypeDescriptionMsgForHash<'_> {
    TypeDescriptionMsgForHash {
        type_description: TypeDescriptionForHash {
            type_name: &msg.type_description.type_name,
            fields: msg
                .type_description
                .fields
                .iter()
                .map(|f| FieldDescriptionForHash {
                    name: &f.name,
                    field_type: &f.field_type,
                })
                .collect(),
        },
        referenced_type_descriptions: msg
            .referenced_type_descriptions
            .iter()
            .map(|td| TypeDescriptionForHash {
                type_name: &td.type_name,
                fields: td
                    .fields
                    .iter()
                    .map(|f| FieldDescriptionForHash {
                        name: &f.name,
                        field_type: &f.field_type,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_description_serialization() {
        let td = TypeDescription {
            type_name: "std_msgs/msg/String".to_string(),
            fields: vec![FieldDescription {
                name: "data".to_string(),
                field_type: FieldTypeDescription::primitive(17), // STRING
                default_value: String::new(),
            }],
        };

        let json = serde_json::to_string(&td).unwrap();
        assert!(json.contains("std_msgs/msg/String"));
        assert!(json.contains("data"));
    }

    #[test]
    fn test_field_type_description_builders() {
        let prim = FieldTypeDescription::primitive(6); // INT32
        assert_eq!(prim.type_id, 6);
        assert_eq!(prim.capacity, 0);
        assert!(prim.nested_type_name.is_empty());

        let nested = FieldTypeDescription::nested(1, "geometry_msgs/msg/Point");
        assert_eq!(nested.type_id, 1);
        assert_eq!(nested.nested_type_name, "geometry_msgs/msg/Point");

        let arr = FieldTypeDescription::array(54, 10); // INT32_ARRAY
        assert_eq!(arr.type_id, 54);
        assert_eq!(arr.capacity, 10);
    }
}
