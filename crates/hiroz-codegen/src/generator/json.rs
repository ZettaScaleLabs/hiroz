//! JSON export for external code generators (Go, Python, etc.)

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::types::{
    ArrayType, Constant, DefaultValue, Field, FieldType, ResolvedAction, ResolvedMessage,
    ResolvedService,
};
#[cfg(test)]
use crate::types::{ParsedMessage, TypeHash};

/// Schema version for compatibility checking
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level manifest for JSON export
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CodegenManifest {
    pub version: u32,
    pub messages: Vec<MessageDefinition>,
    pub services: Vec<ServiceDefinition>,
    pub actions: Vec<ActionDefinition>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MessageDefinition {
    pub package: String,
    pub name: String,
    pub full_name: String,
    pub type_hash: String,
    pub fields: Vec<FieldDefinition>,
    pub constants: Vec<ConstantDefinition>,
    /// Complete ROS description, including referenced types, for runtime registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_description: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FieldDefinition {
    pub name: String,
    pub field_type: JsonFieldType,
    pub is_array: bool,
    pub array_kind: String, // "single", "fixed", "bounded", "unbounded"
    pub array_size: Option<usize>,
    pub default_value: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind")]
pub enum JsonFieldType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    String,
    Time,
    Duration,
    Custom { package: String, name: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ConstantDefinition {
    pub name: String,
    pub const_type: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ServiceDefinition {
    pub package: String,
    pub name: String,
    pub full_name: String,
    pub type_hash: String,
    pub request: MessageDefinition,
    pub response: MessageDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ActionDefinition {
    pub package: String,
    pub name: String,
    pub full_name: String,
    pub type_hash: String,
    pub send_goal_hash: String,
    pub get_result_hash: String,
    pub cancel_goal_hash: String,
    pub feedback_message_hash: String,
    pub status_hash: String,
    pub goal: MessageDefinition,
    pub result: Option<MessageDefinition>,
    pub feedback: Option<MessageDefinition>,
}

/// Export resolved messages, services, and actions to JSON
pub fn export_json(
    messages: &[ResolvedMessage],
    services: &[ResolvedService],
    actions: &[ResolvedAction],
    output_path: &Path,
) -> Result<()> {
    let mut manifest = CodegenManifest {
        version: SCHEMA_VERSION,
        messages: messages.iter().map(convert_message).collect(),
        services: services.iter().map(convert_service).collect(),
        actions: actions.iter().map(convert_action).collect(),
    };

    // Build descriptors using the same identities and dependency descriptions as
    // payload hashing. In particular, historical service/action payload names
    // must not be replaced by their canonical RPC wrapper identities here.
    use crate::resolver::ResolvedMessageExt;
    let all = messages
        .iter()
        .chain(services.iter().flat_map(|s| [&s.request, &s.response]))
        .chain(actions.iter().flat_map(|a| {
            std::iter::once(&a.goal)
                .chain(a.result.iter())
                .chain(a.feedback.iter())
        }));
    let descriptions = all
        .map(|message| {
            (
                format!("{}/{}", message.parsed.package, message.parsed.name),
                message.type_description(),
            )
        })
        .collect();
    fn attach(
        message: &mut MessageDefinition,
        resolved: &ResolvedMessage,
        descriptions: &std::collections::BTreeMap<String, crate::hashing::TypeDescription>,
    ) -> Result<()> {
        let description =
            crate::hashing::build_type_description_msg(&resolved.parsed, descriptions)?;
        let referenced: std::collections::BTreeSet<_> = description
            .referenced_type_descriptions
            .iter()
            .map(|nested| nested.type_name.as_str())
            .chain(std::iter::once(
                description.type_description.type_name.as_str(),
            ))
            .collect();
        for nested in std::iter::once(&description.type_description)
            .chain(&description.referenced_type_descriptions)
        {
            for field in &nested.fields {
                let name = field.field_type.nested_type_name.as_str();
                // Partial manifests remain usable, but must not advertise an incomplete schema.
                if !name.is_empty() && !referenced.contains(name) {
                    return Ok(());
                }
            }
        }
        message.type_description = Some(serde_json::to_value(description)?);
        Ok(())
    }
    for (message, resolved) in manifest.messages.iter_mut().zip(messages) {
        attach(message, resolved, &descriptions)?;
    }
    for (service, resolved) in manifest.services.iter_mut().zip(services) {
        attach(&mut service.request, &resolved.request, &descriptions)?;
        attach(&mut service.response, &resolved.response, &descriptions)?;
    }
    for (action, resolved) in manifest.actions.iter_mut().zip(actions) {
        attach(&mut action.goal, &resolved.goal, &descriptions)?;
        if let (Some(message), Some(resolved)) = (&mut action.result, &resolved.result) {
            attach(message, resolved, &descriptions)?;
        }
        if let (Some(message), Some(resolved)) = (&mut action.feedback, &resolved.feedback) {
            attach(message, resolved, &descriptions)?;
        }
    }

    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(output_path, json)?;

    Ok(())
}

fn convert_message(msg: &ResolvedMessage) -> MessageDefinition {
    let parent_pkg = &msg.parsed.package;
    MessageDefinition {
        package: msg.parsed.package.clone(),
        name: msg.parsed.name.clone(),
        full_name: format!("{}/{}", msg.parsed.package, msg.parsed.name),
        type_hash: msg.type_hash.to_rihs_string(),
        fields: msg
            .parsed
            .fields
            .iter()
            .map(|f| convert_field(f, parent_pkg))
            .collect(),
        constants: msg.parsed.constants.iter().map(convert_constant).collect(),
        type_description: None,
    }
}

fn convert_field(field: &Field, parent_package: &str) -> FieldDefinition {
    let (is_array, array_kind, array_size) = match &field.field_type.array {
        ArrayType::Single => (false, "single".to_string(), None),
        ArrayType::Fixed(n) => (true, "fixed".to_string(), Some(*n)),
        ArrayType::Bounded(n) => (true, "bounded".to_string(), Some(*n)),
        ArrayType::Unbounded => (true, "unbounded".to_string(), None),
    };

    FieldDefinition {
        name: field.name.clone(),
        field_type: convert_field_type(&field.field_type, parent_package),
        is_array,
        array_kind,
        array_size,
        default_value: field.default.as_ref().map(convert_default_value),
    }
}

fn convert_field_type(ft: &FieldType, parent_package: &str) -> JsonFieldType {
    match ft.base_type.as_str() {
        "bool" => JsonFieldType::Bool,
        "int8" => JsonFieldType::Int8,
        "int16" => JsonFieldType::Int16,
        "int32" => JsonFieldType::Int32,
        "int64" => JsonFieldType::Int64,
        "uint8" | "byte" | "char" => JsonFieldType::UInt8,
        "uint16" => JsonFieldType::UInt16,
        "uint32" => JsonFieldType::UInt32,
        "uint64" => JsonFieldType::UInt64,
        "float32" => JsonFieldType::Float32,
        "float64" => JsonFieldType::Float64,
        "string" => JsonFieldType::String,
        "builtin_interfaces/Time" | "time" => JsonFieldType::Time,
        "builtin_interfaces/Duration" | "duration" => JsonFieldType::Duration,
        _ => {
            // Custom message type — use parent package as fallback
            let package = ft
                .package
                .clone()
                .unwrap_or_else(|| parent_package.to_string());
            JsonFieldType::Custom {
                package,
                name: ft.base_type.clone(),
            }
        }
    }
}

fn convert_default_value(dv: &DefaultValue) -> serde_json::Value {
    match dv {
        DefaultValue::Bool(b) => serde_json::Value::Bool(*b),
        DefaultValue::Int(i) => serde_json::Value::Number((*i).into()),
        DefaultValue::Float(f) => serde_json::json!(*f),
        DefaultValue::String(s) => serde_json::Value::String(s.clone()),
        DefaultValue::BoolArray(arr) => serde_json::json!(arr),
        DefaultValue::IntArray(arr) => serde_json::json!(arr),
        DefaultValue::FloatArray(arr) => serde_json::json!(arr),
        DefaultValue::StringArray(arr) => serde_json::json!(arr),
    }
}

fn convert_constant(c: &Constant) -> ConstantDefinition {
    ConstantDefinition {
        name: c.name.clone(),
        const_type: c.const_type.clone(),
        value: c.value.clone(),
    }
}

fn convert_service(srv: &ResolvedService) -> ServiceDefinition {
    ServiceDefinition {
        package: srv.parsed.package.clone(),
        name: srv.parsed.name.clone(),
        full_name: format!("{}/{}", srv.parsed.package, srv.parsed.name),
        type_hash: srv.type_hash.to_rihs_string(),
        request: convert_message(&srv.request),
        response: convert_message(&srv.response),
    }
}

fn convert_action(action: &ResolvedAction) -> ActionDefinition {
    ActionDefinition {
        package: action.parsed.package.clone(),
        name: action.parsed.name.clone(),
        full_name: format!("{}/{}", action.parsed.package, action.parsed.name),
        type_hash: action.type_hash.to_rihs_string(),
        send_goal_hash: action.send_goal_hash.to_rihs_string(),
        get_result_hash: action.get_result_hash.to_rihs_string(),
        cancel_goal_hash: action.cancel_goal_hash.to_rihs_string(),
        feedback_message_hash: action.feedback_message_hash.to_rihs_string(),
        status_hash: action.status_hash.to_rihs_string(),
        goal: convert_message(&action.goal),
        result: action.result.as_ref().map(convert_message),
        feedback: action.feedback.as_ref().map(convert_message),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::{
        ArrayType, Constant, DefaultValue, Field, FieldType, ParsedMessage, ResolvedMessage,
        TypeHash,
    };

    #[test]
    fn action_schema_uses_resolved_nested_message_identity() {
        use crate::{
            parser::{action::parse_action, msg::parse_msg_string},
            resolver::Resolver,
        };

        let nested = parse_msg_string(
            "float32 error\n",
            "test_pkg",
            Path::new("test_pkg/msg/TrackingFeedback.msg"),
        )
        .unwrap();
        let action = parse_action(
            "uint32 target\n---\nbool success\n---\nTrackingFeedback tracking_feedback\ntest_pkg/TrackingFeedback[3] recent_feedback\n",
            "FollowPath",
            "test_pkg",
            Path::new("test_pkg/action/FollowPath.action"),
        )
        .unwrap();
        let mut resolver = Resolver::new(true);
        let messages = resolver.resolve_messages(vec![nested]).unwrap();
        let action = resolver.resolve_action(action).unwrap();
        let original_feedback_hash = action.feedback.as_ref().unwrap().type_hash.to_rihs_string();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("manifest.json");
        export_json(&messages, &[], &[action], &path).unwrap();
        let manifest: CodegenManifest =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for payload in std::iter::once(&manifest.actions[0].goal)
            .chain(manifest.actions[0].result.iter())
            .chain(manifest.actions[0].feedback.iter())
        {
            let description: crate::hashing::TypeDescriptionMsg =
                serde_json::from_value(payload.type_description.clone().unwrap()).unwrap();
            assert_eq!(
                crate::hashing::calculate_hash(&description).to_rihs_string(),
                payload.type_hash
            );
        }
        let feedback = manifest.actions[0].feedback.as_ref().unwrap();
        assert_eq!(feedback.type_hash, original_feedback_hash);
        let description = feedback.type_description.as_ref().unwrap();
        assert_eq!(
            description["type_description"]["type_name"],
            "test_pkg/msg/FollowPathFeedback"
        );
        for field in description["type_description"]["fields"]
            .as_array()
            .unwrap()
        {
            assert_eq!(
                field["type"]["nested_type_name"],
                "test_pkg/msg/TrackingFeedback"
            );
        }
        assert_eq!(
            description["type_description"]["fields"][1]["type"]["capacity"],
            3
        );
        assert_eq!(
            description["referenced_type_descriptions"],
            serde_json::json!([
                manifest.messages[0].type_description.as_ref().unwrap()["type_description"]
            ])
        );
    }

    #[test]
    fn service_payload_schema_hashes_match_resolved_hashes() {
        use crate::{parser::srv::parse_srv_string, resolver::Resolver};

        let service = parse_srv_string(
            "bool data\n---\nbool success\nstring message\n",
            "std_srvs",
            Path::new("std_srvs/srv/SetBool.srv"),
        )
        .unwrap();
        let service = Resolver::new(true).resolve_service(service).unwrap();
        let original_hashes = [
            service.request.type_hash.to_rihs_string(),
            service.response.type_hash.to_rihs_string(),
        ];
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("manifest.json");
        export_json(&[], &[service], &[], &path).unwrap();
        let manifest: CodegenManifest =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (payload, original_hash) in [
            &manifest.services[0].request,
            &manifest.services[0].response,
        ]
        .into_iter()
        .zip(original_hashes)
        {
            let description: crate::hashing::TypeDescriptionMsg =
                serde_json::from_value(payload.type_description.clone().unwrap()).unwrap();
            assert_eq!(payload.type_hash, original_hash);
            assert_eq!(
                crate::hashing::calculate_hash(&description).to_rihs_string(),
                original_hash
            );
            assert_eq!(
                description.type_description.type_name,
                format!("std_srvs/msg/{}", payload.name)
            );
        }
    }

    #[test]
    fn exported_descriptions_preserve_nested_string_bounds() {
        fn message(name: &str, fields: Vec<Field>) -> ResolvedMessage {
            ResolvedMessage {
                parsed: ParsedMessage {
                    package: "test_pkg".into(),
                    name: name.into(),
                    source: String::new(),
                    path: PathBuf::from(format!("msg/{name}.msg")),
                    fields,
                    constants: vec![],
                },
                type_hash: TypeHash::zero(),
                definition: String::new(),
            }
        }
        let child = message(
            "Child",
            vec![Field {
                name: "text".into(),
                field_type: FieldType {
                    base_type: "string".into(),
                    package: None,
                    array: ArrayType::Single,
                    string_bound: Some(12),
                },
                default: None,
            }],
        );
        let parent = message(
            "Parent",
            vec![Field {
                name: "children".into(),
                field_type: FieldType {
                    base_type: "Child".into(),
                    package: Some("test_pkg".into()),
                    array: ArrayType::Bounded(3),
                    string_bound: None,
                },
                default: None,
            }],
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("manifest.json");
        export_json(&[parent, child], &[], &[], &path).unwrap();
        let manifest: CodegenManifest =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let description = manifest.messages[0].type_description.as_ref().unwrap();
        assert_eq!(
            description["type_description"]["fields"][0]["type"]["capacity"],
            3
        );
        assert_eq!(
            description["referenced_type_descriptions"][0]["type_name"],
            "test_pkg/msg/Child"
        );
        assert_eq!(
            description["referenced_type_descriptions"][0]["fields"][0]["type"]["type_id"],
            21
        );
        assert_eq!(
            description["referenced_type_descriptions"][0]["fields"][0]["type"]["string_capacity"],
            12
        );
    }

    #[test]
    fn test_json_export_basic_message() {
        // Create a simple test message
        let msg = ResolvedMessage {
            parsed: ParsedMessage {
                package: "test_pkg".to_string(),
                name: "TestMsg".to_string(),
                path: PathBuf::from("test.msg"),
                source: "test source".to_string(),
                fields: vec![
                    Field {
                        name: "data".to_string(),
                        field_type: FieldType {
                            base_type: "string".to_string(),
                            package: None,
                            array: ArrayType::Single,
                            string_bound: None,
                        },
                        default: None,
                    },
                    Field {
                        name: "count".to_string(),
                        field_type: FieldType {
                            base_type: "int32".to_string(),
                            package: None,
                            array: ArrayType::Single,
                            string_bound: None,
                        },
                        default: Some(DefaultValue::Int(42)),
                    },
                ],
                constants: vec![Constant {
                    name: "MAX_COUNT".to_string(),
                    const_type: "int32".to_string(),
                    value: "100".to_string(),
                }],
            },
            type_hash: TypeHash::from_rihs_string(
                "RIHS01_123456789abcdef0112233445566778899aabbccddeeff001122334455667788",
            )
            .unwrap(),
            definition: "test definition".to_string(),
        };

        let messages = vec![msg];
        let services = vec![];
        let actions = vec![];

        // Export to JSON
        let temp_dir = tempfile::tempdir().unwrap();
        let json_path = temp_dir.path().join("test_manifest.json");

        export_json(&messages, &services, &actions, &json_path).unwrap();

        // Read back and verify
        let json_content = std::fs::read_to_string(&json_path).unwrap();
        let manifest: CodegenManifest = serde_json::from_str(&json_content).unwrap();

        assert_eq!(manifest.version, SCHEMA_VERSION);
        assert_eq!(manifest.messages.len(), 1);
        assert_eq!(manifest.services.len(), 0);
        assert_eq!(manifest.actions.len(), 0);

        let msg_def = &manifest.messages[0];
        assert_eq!(msg_def.package, "test_pkg");
        assert_eq!(msg_def.name, "TestMsg");
        assert_eq!(msg_def.full_name, "test_pkg/TestMsg");
        assert_eq!(msg_def.fields.len(), 2);
        assert_eq!(msg_def.constants.len(), 1);

        // Check first field (string)
        let field1 = &msg_def.fields[0];
        assert_eq!(field1.name, "data");
        assert!(matches!(field1.field_type, JsonFieldType::String));
        assert!(!field1.is_array);

        // Check second field (int32 with default)
        let field2 = &msg_def.fields[1];
        assert_eq!(field2.name, "count");
        assert!(matches!(field2.field_type, JsonFieldType::Int32));
        assert!(!field2.is_array);
        assert_eq!(field2.default_value, Some(serde_json::json!(42)));

        // Check constant
        let constant = &msg_def.constants[0];
        assert_eq!(constant.name, "MAX_COUNT");
        assert_eq!(constant.const_type, "int32");
        assert_eq!(constant.value, "100");
    }

    #[test]
    fn test_json_roundtrip() {
        // Create a test message
        let original_msg = ResolvedMessage {
            parsed: ParsedMessage {
                package: "test_pkg".to_string(),
                name: "RoundtripMsg".to_string(),
                path: PathBuf::from("roundtrip.msg"),
                source: "roundtrip source".to_string(),
                fields: vec![Field {
                    name: "id".to_string(),
                    field_type: FieldType {
                        base_type: "uint32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                }],
                constants: vec![],
            },
            type_hash: TypeHash::from_rihs_string(
                "RIHS01_1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            )
            .unwrap(),
            definition: "roundtrip definition".to_string(),
        };

        let messages = vec![original_msg.clone()];
        let services = vec![];
        let actions = vec![];

        // Export to JSON
        let temp_dir = tempfile::tempdir().unwrap();
        let json_path = temp_dir.path().join("roundtrip_manifest.json");

        export_json(&messages, &services, &actions, &json_path).unwrap();

        // Read back and verify structure
        let json_content = std::fs::read_to_string(&json_path).unwrap();
        let manifest: CodegenManifest = serde_json::from_str(&json_content).unwrap();

        assert_eq!(manifest.version, SCHEMA_VERSION);
        assert_eq!(manifest.messages.len(), 1);

        let msg_def = &manifest.messages[0];
        assert_eq!(msg_def.package, original_msg.parsed.package);
        assert_eq!(msg_def.name, original_msg.parsed.name);
        assert_eq!(
            msg_def.full_name,
            format!(
                "{}/{}",
                original_msg.parsed.package, original_msg.parsed.name
            )
        );
        assert_eq!(msg_def.type_hash, original_msg.type_hash.to_rihs_string());
        assert_eq!(msg_def.fields.len(), original_msg.parsed.fields.len());
        assert_eq!(msg_def.constants.len(), original_msg.parsed.constants.len());

        // Verify JSON can be pretty-printed and re-parsed
        let pretty_json = serde_json::to_string_pretty(&manifest).unwrap();
        let reparsed: CodegenManifest = serde_json::from_str(&pretty_json).unwrap();
        assert_eq!(manifest, reparsed);
    }
}

#[test]
fn test_json_export_array_fields() {
    // Create a message with array fields
    let msg = ResolvedMessage {
        parsed: ParsedMessage {
            package: "test_pkg".to_string(),
            name: "ArrayMsg".to_string(),
            path: PathBuf::from("array.msg"),
            source: "array source".to_string(),
            fields: vec![
                Field {
                    name: "fixed_array".to_string(),
                    field_type: FieldType {
                        base_type: "float32".to_string(),
                        package: None,
                        array: ArrayType::Fixed(10),
                        string_bound: None,
                    },
                    default: None,
                },
                Field {
                    name: "dynamic_array".to_string(),
                    field_type: FieldType {
                        base_type: "string".to_string(),
                        package: None,
                        array: ArrayType::Unbounded,
                        string_bound: None,
                    },
                    default: None,
                },
            ],
            constants: vec![],
        },
        type_hash: TypeHash::from_rihs_string(
            "RIHS01_abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap(),
        definition: "array definition".to_string(),
    };

    let messages = vec![msg];
    let services = vec![];
    let actions = vec![];

    let temp_dir = tempfile::tempdir().unwrap();
    let json_path = temp_dir.path().join("array_manifest.json");

    export_json(&messages, &services, &actions, &json_path).unwrap();

    let json_content = std::fs::read_to_string(&json_path).unwrap();
    let manifest: CodegenManifest = serde_json::from_str(&json_content).unwrap();

    let msg_def = &manifest.messages[0];
    assert_eq!(msg_def.fields.len(), 2);

    // Check fixed array
    let fixed_field = &msg_def.fields[0];
    assert_eq!(fixed_field.name, "fixed_array");
    assert!(matches!(fixed_field.field_type, JsonFieldType::Float32));
    assert!(fixed_field.is_array);
    assert_eq!(fixed_field.array_kind, "fixed");
    assert_eq!(fixed_field.array_size, Some(10));

    // Check dynamic array
    let dynamic_field = &msg_def.fields[1];
    assert_eq!(dynamic_field.name, "dynamic_array");
    assert!(matches!(dynamic_field.field_type, JsonFieldType::String));
    assert!(dynamic_field.is_array);
    assert_eq!(dynamic_field.array_kind, "unbounded");
    assert_eq!(dynamic_field.array_size, None);
}

#[test]
fn test_json_export_custom_type() {
    // Create a message with a custom type reference
    let msg = ResolvedMessage {
        parsed: ParsedMessage {
            package: "test_pkg".to_string(),
            name: "CustomMsg".to_string(),
            path: PathBuf::from("custom.msg"),
            source: "custom source".to_string(),
            fields: vec![Field {
                name: "header".to_string(),
                field_type: FieldType {
                    base_type: "Header".to_string(),
                    package: Some("std_msgs".to_string()),
                    array: ArrayType::Single,
                    string_bound: None,
                },
                default: None,
            }],
            constants: vec![],
        },
        type_hash: TypeHash::from_rihs_string(
            "RIHS01_1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap(),
        definition: "custom definition".to_string(),
    };

    let messages = vec![msg];
    let services = vec![];
    let actions = vec![];

    let temp_dir = tempfile::tempdir().unwrap();
    let json_path = temp_dir.path().join("custom_manifest.json");

    export_json(&messages, &services, &actions, &json_path).unwrap();

    let json_content = std::fs::read_to_string(&json_path).unwrap();
    let manifest: CodegenManifest = serde_json::from_str(&json_content).unwrap();

    let msg_def = &manifest.messages[0];
    let field = &msg_def.fields[0];
    assert_eq!(field.name, "header");
    match &field.field_type {
        JsonFieldType::Custom { package, name } => {
            assert_eq!(package, "std_msgs");
            assert_eq!(name, "Header");
        }
        _ => panic!("Expected Custom field type"),
    }
}

#[test]
fn test_json_roundtrip() {
    // Create a test message
    let original_msg = ResolvedMessage {
        parsed: ParsedMessage {
            package: "test_pkg".to_string(),
            name: "RoundtripMsg".to_string(),
            path: PathBuf::from("roundtrip.msg"),
            source: "roundtrip source".to_string(),
            fields: vec![Field {
                name: "id".to_string(),
                field_type: FieldType {
                    base_type: "uint32".to_string(),
                    package: None,
                    array: ArrayType::Single,
                    string_bound: None,
                },
                default: None,
            }],
            constants: vec![],
        },
        type_hash: TypeHash::from_rihs_string(
            "RIHS01_1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap(),
        definition: "roundtrip definition".to_string(),
    };

    let messages = vec![original_msg.clone()];
    let services = vec![];
    let actions = vec![];

    // Export to JSON
    let temp_dir = tempfile::tempdir().unwrap();
    let json_path = temp_dir.path().join("roundtrip_manifest.json");

    export_json(&messages, &services, &actions, &json_path).unwrap();

    // Read back and verify structure
    let json_content = std::fs::read_to_string(&json_path).unwrap();
    let manifest: CodegenManifest = serde_json::from_str(&json_content).unwrap();

    assert_eq!(manifest.version, SCHEMA_VERSION);
    assert_eq!(manifest.messages.len(), 1);

    let msg_def = &manifest.messages[0];
    assert_eq!(msg_def.package, original_msg.parsed.package);
    assert_eq!(msg_def.name, original_msg.parsed.name);
    assert_eq!(
        msg_def.full_name,
        format!(
            "{}/{}",
            original_msg.parsed.package, original_msg.parsed.name
        )
    );
    assert_eq!(msg_def.type_hash, original_msg.type_hash.to_rihs_string());
    assert_eq!(msg_def.fields.len(), original_msg.parsed.fields.len());
    assert_eq!(msg_def.constants.len(), original_msg.parsed.constants.len());

    // Verify JSON can be pretty-printed and re-parsed
    let pretty_json = serde_json::to_string_pretty(&manifest).unwrap();
    let reparsed: CodegenManifest = serde_json::from_str(&pretty_json).unwrap();
    assert_eq!(manifest, reparsed);
}
