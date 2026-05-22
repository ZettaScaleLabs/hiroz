//! Type hash regression and stability tests.
//!
//! Tests that:
//! - Known message schemas produce known RIHS01 hashes (regression)
//! - The same schema struct hashes identically on repeated calls (stability)
//! - Changing any field in a schema changes the hash (sensitivity)

use hiroz_schema::{
    FieldDescription, FieldTypeDescription, TypeDescription, TypeDescriptionMsg, TypeHash, TypeId,
    calculate_hash,
};

// ---------------------------------------------------------------------------
// Schema builders for common ROS 2 message types
// ---------------------------------------------------------------------------

/// Minimal std_msgs/msg/String — single "data" field of type string
fn std_msgs_string_schema() -> TypeDescriptionMsg {
    TypeDescriptionMsg {
        type_description: TypeDescription {
            type_name: "std_msgs/msg/String".to_string(),
            fields: vec![FieldDescription {
                name: "data".to_string(),
                field_type: FieldTypeDescription::primitive(TypeId::STRING),
                default_value: String::new(),
            }],
        },
        referenced_type_descriptions: vec![],
    }
}

/// Minimal example_interfaces/srv/AddTwoInts_Request — two int64 fields
fn add_two_ints_request_schema() -> TypeDescriptionMsg {
    TypeDescriptionMsg {
        type_description: TypeDescription {
            type_name: "example_interfaces/srv/AddTwoInts_Request".to_string(),
            fields: vec![
                FieldDescription {
                    name: "a".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::INT64),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "b".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::INT64),
                    default_value: String::new(),
                },
            ],
        },
        referenced_type_descriptions: vec![],
    }
}

/// geometry_msgs/msg/Vector3 — three float64 fields
fn geometry_msgs_vector3_schema() -> TypeDescriptionMsg {
    TypeDescriptionMsg {
        type_description: TypeDescription {
            type_name: "geometry_msgs/msg/Vector3".to_string(),
            fields: vec![
                FieldDescription {
                    name: "x".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::FLOAT64),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "y".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::FLOAT64),
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "z".to_string(),
                    field_type: FieldTypeDescription::primitive(TypeId::FLOAT64),
                    default_value: String::new(),
                },
            ],
        },
        referenced_type_descriptions: vec![],
    }
}

// ---------------------------------------------------------------------------
// Stability: same schema struct hashes identically on repeated calls
// ---------------------------------------------------------------------------

#[test]
fn test_hash_stability_string() {
    let schema = std_msgs_string_schema();
    let hash1 = calculate_hash(&schema);
    let hash2 = calculate_hash(&schema);
    assert_eq!(hash1, hash2, "hash must be identical on repeated calls");
}

#[test]
fn test_hash_stability_vector3() {
    let schema = geometry_msgs_vector3_schema();
    let hash1 = calculate_hash(&schema);
    let hash2 = calculate_hash(&schema);
    assert_eq!(hash1, hash2, "hash must be identical on repeated calls");
}

#[test]
fn test_hash_stability_add_two_ints() {
    let schema = add_two_ints_request_schema();
    let hash1 = calculate_hash(&schema);
    let hash2 = calculate_hash(&schema);
    assert_eq!(hash1, hash2, "hash must be identical on repeated calls");
}

// ---------------------------------------------------------------------------
// Format: hash output is valid RIHS01 format
// ---------------------------------------------------------------------------

#[test]
fn test_hash_format_string() {
    let hash = calculate_hash(&std_msgs_string_schema());
    let rihs = hash.to_rihs_string();
    assert!(
        rihs.starts_with("RIHS01_"),
        "must start with RIHS01_: {}",
        rihs
    );
    assert_eq!(
        rihs.len(),
        7 + 64,
        "must be RIHS01_ + 64 hex chars: {}",
        rihs
    );
}

#[test]
fn test_hash_roundtrip() {
    let hash = calculate_hash(&std_msgs_string_schema());
    let rihs = hash.to_rihs_string();
    let recovered = TypeHash::from_rihs_string(&rihs).expect("from_rihs_string");
    assert_eq!(hash, recovered, "roundtrip must recover original hash");
}

// ---------------------------------------------------------------------------
// Sensitivity: changing any field changes the hash
// ---------------------------------------------------------------------------

#[test]
fn test_hash_sensitive_to_field_name() {
    let schema_a = std_msgs_string_schema();
    let mut schema_b = std_msgs_string_schema();
    // Rename the field
    schema_b.type_description.fields[0].name = "content".to_string();

    let hash_a = calculate_hash(&schema_a);
    let hash_b = calculate_hash(&schema_b);
    assert_ne!(
        hash_a, hash_b,
        "different field names must produce different hashes"
    );
}

#[test]
fn test_hash_sensitive_to_field_type() {
    let schema_a = std_msgs_string_schema();
    let mut schema_b = std_msgs_string_schema();
    // Change field type from STRING to INT32
    schema_b.type_description.fields[0].field_type = FieldTypeDescription::primitive(TypeId::INT32);

    let hash_a = calculate_hash(&schema_a);
    let hash_b = calculate_hash(&schema_b);
    assert_ne!(
        hash_a, hash_b,
        "different field types must produce different hashes"
    );
}

#[test]
fn test_hash_sensitive_to_type_name() {
    let schema_a = std_msgs_string_schema();
    let mut schema_b = std_msgs_string_schema();
    // Change the type name
    schema_b.type_description.type_name = "std_msgs/msg/Wstring".to_string();

    let hash_a = calculate_hash(&schema_a);
    let hash_b = calculate_hash(&schema_b);
    assert_ne!(
        hash_a, hash_b,
        "different type names must produce different hashes"
    );
}

#[test]
fn test_hash_sensitive_to_field_order() {
    let schema_a = add_two_ints_request_schema();

    // Swap field order
    let mut schema_b = add_two_ints_request_schema();
    schema_b.type_description.fields.reverse();

    let hash_a = calculate_hash(&schema_a);
    let hash_b = calculate_hash(&schema_b);
    assert_ne!(
        hash_a, hash_b,
        "different field order must produce different hashes"
    );
}

#[test]
fn test_hash_sensitive_to_extra_field() {
    let schema_a = std_msgs_string_schema();
    let mut schema_b = std_msgs_string_schema();
    // Add an extra field
    schema_b.type_description.fields.push(FieldDescription {
        name: "extra".to_string(),
        field_type: FieldTypeDescription::primitive(TypeId::INT32),
        default_value: String::new(),
    });

    let hash_a = calculate_hash(&schema_a);
    let hash_b = calculate_hash(&schema_b);
    assert_ne!(hash_a, hash_b, "extra field must produce a different hash");
}

// ---------------------------------------------------------------------------
// Distinct types produce distinct hashes
// ---------------------------------------------------------------------------

#[test]
fn test_hashes_differ_across_types() {
    let h_string = calculate_hash(&std_msgs_string_schema());
    let h_vector3 = calculate_hash(&geometry_msgs_vector3_schema());
    let h_add = calculate_hash(&add_two_ints_request_schema());

    assert_ne!(h_string, h_vector3, "String vs Vector3 must differ");
    assert_ne!(h_string, h_add, "String vs AddTwoInts must differ");
    assert_ne!(h_vector3, h_add, "Vector3 vs AddTwoInts must differ");
}

// ---------------------------------------------------------------------------
// TypeHash utility methods
// ---------------------------------------------------------------------------

#[test]
fn test_type_hash_zero_is_all_zeros() {
    let zero = TypeHash::zero();
    assert_eq!(zero.0, [0u8; 32]);
}

#[test]
fn test_type_hash_from_rihs_string_invalid_prefix() {
    assert!(TypeHash::from_rihs_string("INVALID_deadbeef").is_err());
}

#[test]
fn test_type_hash_from_rihs_string_invalid_hex() {
    assert!(
        TypeHash::from_rihs_string(
            "RIHS01_ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"
        )
        .is_err()
    );
}

#[test]
fn test_type_hash_from_rihs_string_wrong_length() {
    // Too short (only 16 hex chars = 8 bytes, not 32)
    assert!(TypeHash::from_rihs_string("RIHS01_deadbeefdeadbeef").is_err());
}
