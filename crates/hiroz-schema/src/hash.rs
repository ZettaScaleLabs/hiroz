//! RIHS01 Type Hash Calculation
//!
//! This module implements the ROS2 RIHS01 (ROS IDL Hash Standard 01) type hashing algorithm.

use serde::Serialize;
use sha2::Digest;

use crate::TypeHash;
use crate::type_description::{TypeDescriptionMsg, to_hash_version};

/// Calculate RIHS01 hash for a TypeDescriptionMsg
///
/// This computes the SHA-256 hash of the JSON-serialized type description,
/// following the ROS 2 RIHS01 specification.
pub fn calculate_hash(msg: &TypeDescriptionMsg) -> TypeHash {
    // Exclude default_value from hash computation (ROS2's emit_field() doesn't include it)
    let hash_version = to_hash_version(msg);
    let json = to_ros2_json(&hash_version).expect("JSON serialization should not fail");

    let mut hasher = sha2::Sha256::new();
    hasher.update(json.as_bytes());
    let hash_bytes: [u8; 32] = hasher.finalize().into();

    TypeHash(hash_bytes)
}

/// Serialize to ROS2-compatible JSON format
///
/// ROS2 uses JSON with spaces after colons and commas: `{"key": "value", "key2": 2}`
/// This matches Python's default `json.dumps()` output.
pub fn to_ros2_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    // Serialize to compact JSON
    let compact = serde_json::to_string(value)?;

    // Add spaces after : and , (matching Python's default json.dumps())
    let mut result = String::with_capacity(compact.len() + 100);
    let chars: Vec<char> = compact.chars().collect();

    for i in 0..chars.len() {
        result.push(chars[i]);

        // Add space after : or , if not already followed by a space
        if (chars[i] == ':' || chars[i] == ',') && i + 1 < chars.len() && chars[i + 1] != ' ' {
            result.push(' ');
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldDescription, FieldTypeDescription, TypeDescription, TypeId};

    #[test]
    fn test_to_ros2_json_spacing() {
        #[derive(Serialize)]
        struct Test {
            a: i32,
            b: String,
        }

        let t = Test {
            a: 42,
            b: "hello".to_string(),
        };

        let json = to_ros2_json(&t).unwrap();
        assert!(json.contains(": ")); // Space after colon
        assert!(json.contains(", ")); // Space after comma
        assert_eq!(json, r#"{"a": 42, "b": "hello"}"#);
    }

    #[test]
    fn test_calculate_hash_simple() {
        let msg = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "std_msgs/msg/String".to_string(),
                fields: vec![FieldDescription {
                    name: "data".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::STRING),
                    default_value: String::new(),
                }],
            },
            referenced_type_descriptions: vec![],
        };

        let hash = calculate_hash(&msg);
        let rihs = hash.to_rihs_string();

        // Verify format
        assert!(rihs.starts_with("RIHS01_"));
        assert_eq!(rihs.len(), 7 + 64); // "RIHS01_" + 64 hex chars

        // Hash should be deterministic
        let hash2 = calculate_hash(&msg);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_calculate_hash_with_nested() {
        let header_desc = TypeDescription {
            type_name: "std_msgs/msg/Header".to_string(),
            fields: vec![
                FieldDescription {
                    name: "stamp".to_string(),
                    field_type: FieldTypeDescription::nested(
                        TypeId::NESTED_TYPE,
                        "builtin_interfaces/msg/Time",
                    ),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "frame_id".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::STRING),
                    default_value: String::new(),
                },
            ],
        };

        let time_desc = TypeDescription {
            type_name: "builtin_interfaces/msg/Time".to_string(),
            fields: vec![
                FieldDescription {
                    name: "sec".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::INT32),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "nanosec".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::UINT32),
                    default_value: String::new(),
                },
            ],
        };

        let msg = TypeDescriptionMsg {
            type_description: header_desc,
            referenced_type_descriptions: vec![time_desc],
        };

        let hash = calculate_hash(&msg);
        let rihs = hash.to_rihs_string();

        assert!(rihs.starts_with("RIHS01_"));
    }

    #[test]
    fn test_hash_excludes_default_value() {
        // Two messages that differ only in default_value should have same hash
        let msg1 = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "test/msg/Test".to_string(),
                fields: vec![FieldDescription {
                    name: "value".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::INT32),
                    default_value: String::new(),
                }],
            },
            referenced_type_descriptions: vec![],
        };

        let msg2 = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "test/msg/Test".to_string(),
                fields: vec![FieldDescription {
                    name: "value".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::INT32),
                    default_value: "42".to_string(), // Different default
                }],
            },
            referenced_type_descriptions: vec![],
        };

        let hash1 = calculate_hash(&msg1);
        let hash2 = calculate_hash(&msg2);

        assert_eq!(hash1, hash2, "Hash should not include default_value");
    }

    #[test]
    fn test_hash_std_msgs_string_deterministic() {
        // std_msgs/msg/String - verify hash is deterministic and well-formed
        // Note: To verify against ROS 2, run: ros2 interface show std_msgs/msg/String --show-hash
        let msg = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "std_msgs/msg/String".to_string(),
                fields: vec![FieldDescription {
                    name: "data".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::STRING),
                    default_value: String::new(),
                }],
            },
            referenced_type_descriptions: vec![],
        };

        let hash1 = calculate_hash(&msg);
        let hash2 = calculate_hash(&msg);

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
    fn test_hash_builtin_time_deterministic() {
        // builtin_interfaces/msg/Time
        // Note: To verify against ROS 2, run: ros2 interface show builtin_interfaces/msg/Time --show-hash
        let msg = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "builtin_interfaces/msg/Time".to_string(),
                fields: vec![
                    FieldDescription {
                        name: "sec".to_string(),
                        field_type: FieldTypeDescription::primitive(TypeId::INT32),
                        default_value: String::new(),
                    },
                    FieldDescription {
                        name: "nanosec".to_string(),
                        field_type: FieldTypeDescription::primitive(TypeId::UINT32),
                        default_value: String::new(),
                    },
                ],
            },
            referenced_type_descriptions: vec![],
        };

        let hash1 = calculate_hash(&msg);
        let hash2 = calculate_hash(&msg);

        // Hash should be deterministic
        assert_eq!(hash1, hash2);

        // Hash should be valid RIHS01 format
        let rihs = hash1.to_rihs_string();
        assert!(rihs.starts_with("RIHS01_"));
        assert_eq!(rihs.len(), 7 + 64);

        // Verify our computed hash (for regression testing)
        assert_eq!(
            rihs,
            "RIHS01_b106235e25a4c5ed35098aa0a61a3ee9c9b18d197f398b0e4206cea9acf9c197"
        );
    }

    #[test]
    fn test_hash_empty_message() {
        // Empty message (like std_srvs/srv/Empty request/response)
        let msg = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "test_msgs/msg/Empty".to_string(),
                fields: vec![],
            },
            referenced_type_descriptions: vec![],
        };

        let hash = calculate_hash(&msg);
        let rihs = hash.to_rihs_string();

        // Empty messages should still produce valid hash
        assert!(rihs.starts_with("RIHS01_"));
        assert_eq!(rihs.len(), 7 + 64);
    }

    #[test]
    fn test_hash_with_arrays() {
        // Message with fixed array
        let msg = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "test_msgs/msg/Arrays".to_string(),
                fields: vec![
                    FieldDescription {
                        name: "float_array".to_string(),
                        field_type: FieldTypeDescription::array(TypeId::FLOAT64_ARRAY, 3),
                        default_value: String::new(),
                    },
                    FieldDescription {
                        name: "int_sequence".to_string(),
                        field_type: FieldTypeDescription::primitive(
                            TypeId::INT32_UNBOUNDED_SEQUENCE,
                        ),
                        default_value: String::new(),
                    },
                    FieldDescription {
                        name: "bounded_seq".to_string(),
                        field_type: FieldTypeDescription::array(TypeId::UINT8_BOUNDED_SEQUENCE, 10),
                        default_value: String::new(),
                    },
                ],
            },
            referenced_type_descriptions: vec![],
        };

        let hash = calculate_hash(&msg);
        let rihs = hash.to_rihs_string();

        assert!(rihs.starts_with("RIHS01_"));

        // Hash should be deterministic
        let hash2 = calculate_hash(&msg);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_hash_with_bounded_string() {
        let msg = TypeDescriptionMsg {
            type_description: TypeDescription {
                type_name: "test_msgs/msg/BoundedString".to_string(),
                fields: vec![FieldDescription {
                    name: "bounded_data".to_string(),
                    field_type: FieldTypeDescription {
                        type_id: TypeId::STRING,
                        capacity: 0,
                        string_capacity: 256,
                        nested_type_name: String::new(),
                    },
                    default_value: String::new(),
                }],
            },
            referenced_type_descriptions: vec![],
        };

        let hash = calculate_hash(&msg);
        let rihs = hash.to_rihs_string();

        assert!(rihs.starts_with("RIHS01_"));
    }

    #[test]
    fn test_hash_referenced_types_order_matters() {
        // Referenced types must be sorted alphabetically for consistent hashing
        let type_a = TypeDescription {
            type_name: "pkg/msg/TypeA".to_string(),
            fields: vec![FieldDescription {
                name: "value".to_string(),
                field_type: FieldTypeDescription::primitive(TypeId::INT32),
                default_value: String::new(),
            }],
        };

        let type_b = TypeDescription {
            type_name: "pkg/msg/TypeB".to_string(),
            fields: vec![FieldDescription {
                name: "value".to_string(),
                field_type: FieldTypeDescription::primitive(TypeId::FLOAT64),
                default_value: String::new(),
            }],
        };

        let main_type = TypeDescription {
            type_name: "pkg/msg/Main".to_string(),
            fields: vec![
                FieldDescription {
                    name: "a".to_string(),
                    field_type: FieldTypeDescription::nested(TypeId::NESTED_TYPE, "pkg/msg/TypeA"),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "b".to_string(),
                    field_type: FieldTypeDescription::nested(TypeId::NESTED_TYPE, "pkg/msg/TypeB"),
                    default_value: String::new(),
                },
            ],
        };

        // Order A, B
        let msg1 = TypeDescriptionMsg {
            type_description: main_type.clone(),
            referenced_type_descriptions: vec![type_a.clone(), type_b.clone()],
        };

        // Order B, A (should produce same hash after sorting)
        let msg2 = TypeDescriptionMsg {
            type_description: main_type,
            referenced_type_descriptions: vec![type_b, type_a],
        };

        let hash1 = calculate_hash(&msg1);
        let hash2 = calculate_hash(&msg2);

        // Note: The caller is responsible for sorting referenced_type_descriptions
        // If we want consistent hashing regardless of input order, we'd need to sort here
        // This test documents the current behavior
        assert_ne!(
            hash1, hash2,
            "Different order produces different hash (caller must sort)"
        );
    }

    #[test]
    fn test_hash_deeply_nested() {
        // Three-level nesting: Main -> Middle -> Inner
        let inner = TypeDescription {
            type_name: "pkg/msg/Inner".to_string(),
            fields: vec![FieldDescription {
                name: "value".to_string(),
                field_type: FieldTypeDescription::primitive(TypeId::FLOAT64),
                default_value: String::new(),
            }],
        };

        let middle = TypeDescription {
            type_name: "pkg/msg/Middle".to_string(),
            fields: vec![FieldDescription {
                name: "inner".to_string(),
                field_type: FieldTypeDescription::nested(TypeId::NESTED_TYPE, "pkg/msg/Inner"),
                default_value: String::new(),
            }],
        };

        let main = TypeDescription {
            type_name: "pkg/msg/Main".to_string(),
            fields: vec![FieldDescription {
                name: "middle".to_string(),
                field_type: FieldTypeDescription::nested(TypeId::NESTED_TYPE, "pkg/msg/Middle"),
                default_value: String::new(),
            }],
        };

        let msg = TypeDescriptionMsg {
            type_description: main,
            // Referenced types sorted alphabetically
            referenced_type_descriptions: vec![inner, middle],
        };

        let hash = calculate_hash(&msg);
        let rihs = hash.to_rihs_string();

        assert!(rihs.starts_with("RIHS01_"));

        // Should be deterministic
        let hash2 = calculate_hash(&msg);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_json_field_order() {
        // Verify JSON field order matches ROS 2 expectations
        let td = TypeDescription {
            type_name: "test/msg/Test".to_string(),
            fields: vec![FieldDescription {
                name: "data".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 6,
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: String::new(),
                },
                default_value: String::new(),
            }],
        };

        let msg = TypeDescriptionMsg {
            type_description: td,
            referenced_type_descriptions: vec![],
        };

        let hash_version = to_hash_version(&msg);
        let json = to_ros2_json(&hash_version).unwrap();

        // Verify structure matches ROS 2 format
        assert!(json.contains("\"type_description\""));
        assert!(json.contains("\"referenced_type_descriptions\""));
        assert!(json.contains("\"type_name\""));
        assert!(json.contains("\"fields\""));
        assert!(json.contains("\"type\""));
        assert!(json.contains("\"type_id\""));
    }
}
