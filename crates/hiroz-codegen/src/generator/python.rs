use crate::types::{ArrayType, FieldType, ResolvedMessage, ResolvedService};
use anyhow::Result;

/// Generate Python module for a package containing messages
pub fn generate_python_package(
    package: &str,
    messages: &[ResolvedMessage],
    services: &[ResolvedService],
) -> Result<String> {
    let mut code = format!(
        "\"\"\"Auto-generated ROS2 message types for {}.\"\"\"\n\
         import msgspec\n\
         from typing import ClassVar\n\n",
        package
    );

    // Generate message classes
    for msg in messages {
        code.push_str(&generate_python_message(msg)?);
        code.push('\n');
    }

    // Generate service Request/Response classes with service type hash
    for srv in services {
        code.push_str(&generate_python_service_message(
            &srv.request,
            &srv.type_hash,
        )?);
        code.push('\n');
        code.push_str(&generate_python_service_message(
            &srv.response,
            &srv.type_hash,
        )?);
        code.push('\n');
    }

    Ok(code)
}

/// Generate Python msgspec class for a single message
fn generate_python_message(msg: &ResolvedMessage) -> Result<String> {
    generate_python_message_with_svc_hash(msg, None)
}

/// Generate Python msgspec class for a service request/response message
fn generate_python_service_message(
    msg: &ResolvedMessage,
    svc_hash: &crate::types::TypeHash,
) -> Result<String> {
    generate_python_message_with_svc_hash(msg, Some(svc_hash))
}

/// Generate Python msgspec class for a message, optionally with service type hash
fn generate_python_message_with_svc_hash(
    msg: &ResolvedMessage,
    svc_hash: Option<&crate::types::TypeHash>,
) -> Result<String> {
    let name = &msg.parsed.name;
    let package = &msg.parsed.package;
    let hash = msg.type_hash.to_rihs_string();

    let mut code = format!(
        "class {}(msgspec.Struct, frozen=True, kw_only=True):\n",
        name
    );

    // Generate fields
    if msg.parsed.fields.is_empty() {
        code.push_str("    pass\n");
    } else {
        for field in &msg.parsed.fields {
            let py_type = to_python_type(&field.field_type)?;
            let default = python_default(&field.field_type);
            code.push_str(&format!("    {}: {} = {}\n", field.name, py_type, default));
        }
    }

    // Add metadata
    code.push_str(&format!(
        "\n    __msgtype__: ClassVar[str] = '{}/msg/{}'\n",
        package, name
    ));

    // Use service type hash for service request/response, message type hash for regular messages
    if let Some(srv_type_hash) = svc_hash {
        code.push_str(&format!(
            "    __hash__: ClassVar[str] = '{}'\n",
            srv_type_hash.to_rihs_string()
        ));
    } else {
        code.push_str(&format!("    __hash__: ClassVar[str] = '{}'\n", hash));
    }

    Ok(code)
}

/// Convert ROS field type to Python type annotation
fn to_python_type(field_type: &FieldType) -> Result<String> {
    let base = match field_type.base_type.as_str() {
        "bool" => "bool",
        "byte" | "char" | "uint8" | "int8" | "uint16" | "int16" | "uint32" | "int32" | "uint64"
        | "int64" => "int",
        "float32" | "float64" => "float",
        "string" => "str",
        custom => {
            let pkg = field_type
                .package
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Missing package for {}", custom))?;
            return Ok(match field_type.array {
                ArrayType::Single => format!("'{}.{}'", pkg, custom),
                _ => format!("list['{}.{}']", pkg, custom),
            });
        }
    };

    Ok(match &field_type.array {
        ArrayType::Single => base.to_string(),
        ArrayType::Fixed(n) => {
            // Python doesn't have fixed-size arrays, use tuple
            if *n <= 10 {
                // For small sizes, use tuple[T, T, ...]
                let types = vec![base; *n];
                format!("tuple[{}]", types.join(", "))
            } else {
                // For larger sizes, use list with comment
                format!("list[{}]  # fixed[{}]", base, n)
            }
        }
        ArrayType::Bounded(_) | ArrayType::Unbounded => {
            // Special case for bytes
            if matches!(field_type.base_type.as_str(), "uint8" | "byte") {
                "bytes".to_string()
            } else {
                format!("list[{}]", base)
            }
        }
    })
}

/// Generate Python default value for a field type
fn python_default(field_type: &FieldType) -> String {
    match &field_type.array {
        ArrayType::Single => match field_type.base_type.as_str() {
            "bool" => "False".to_string(),
            "byte" | "char" | "uint8" | "int8" | "uint16" | "int16" | "uint32" | "int32"
            | "uint64" | "int64" => "0".to_string(),
            "float32" | "float64" => "0.0".to_string(),
            "string" => "''".to_string(),
            _ => "None".to_string(), // Custom types need factory
        },
        ArrayType::Fixed(n) => {
            let elem_default = python_default(&FieldType {
                base_type: field_type.base_type.clone(),
                package: field_type.package.clone(),
                array: ArrayType::Single,
                string_bound: None,
            });
            format!(
                "({}{})",
                elem_default,
                format!(", {}", elem_default).repeat(*n - 1)
            )
        }
        ArrayType::Bounded(_) | ArrayType::Unbounded => {
            if matches!(field_type.base_type.as_str(), "uint8" | "byte") {
                "b''".to_string()
            } else {
                "msgspec.UNSET".to_string() // Use factory
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Field, ParsedMessage, TypeHash};
    use std::path::PathBuf;

    #[test]
    fn test_to_python_type_primitives() {
        let field_type = FieldType {
            base_type: "int32".to_string(),
            package: None,
            array: ArrayType::Single,
            string_bound: None,
        };
        assert_eq!(to_python_type(&field_type).unwrap(), "int");

        let field_type = FieldType {
            base_type: "bool".to_string(),
            package: None,
            array: ArrayType::Single,
            string_bound: None,
        };
        assert_eq!(to_python_type(&field_type).unwrap(), "bool");

        let field_type = FieldType {
            base_type: "string".to_string(),
            package: None,
            array: ArrayType::Single,
            string_bound: None,
        };
        assert_eq!(to_python_type(&field_type).unwrap(), "str");
    }

    #[test]
    fn test_to_python_type_arrays() {
        let field_type = FieldType {
            base_type: "uint8".to_string(),
            package: None,
            array: ArrayType::Unbounded,
            string_bound: None,
        };
        assert_eq!(to_python_type(&field_type).unwrap(), "bytes");

        let field_type = FieldType {
            base_type: "int32".to_string(),
            package: None,
            array: ArrayType::Unbounded,
            string_bound: None,
        };
        assert_eq!(to_python_type(&field_type).unwrap(), "list[int]");
    }

    #[test]
    fn test_to_python_type_custom() {
        let field_type = FieldType {
            base_type: "Point".to_string(),
            package: Some("geometry_msgs".to_string()),
            array: ArrayType::Single,
            string_bound: None,
        };
        assert_eq!(
            to_python_type(&field_type).unwrap(),
            "'geometry_msgs.Point'"
        );

        let field_type = FieldType {
            base_type: "Point".to_string(),
            package: Some("geometry_msgs".to_string()),
            array: ArrayType::Unbounded,
            string_bound: None,
        };
        assert_eq!(
            to_python_type(&field_type).unwrap(),
            "list['geometry_msgs.Point']"
        );
    }

    #[test]
    fn test_python_default() {
        let field_type = FieldType {
            base_type: "int32".to_string(),
            package: None,
            array: ArrayType::Single,
            string_bound: None,
        };
        assert_eq!(python_default(&field_type), "0");

        let field_type = FieldType {
            base_type: "string".to_string(),
            package: None,
            array: ArrayType::Single,
            string_bound: None,
        };
        assert_eq!(python_default(&field_type), "''");

        let field_type = FieldType {
            base_type: "uint8".to_string(),
            package: None,
            array: ArrayType::Unbounded,
            string_bound: None,
        };
        assert_eq!(python_default(&field_type), "b''");
    }

    #[test]
    fn test_generate_simple_python_message() {
        let msg = ResolvedMessage {
            parsed: ParsedMessage {
                name: "String".to_string(),
                package: "std_msgs".to_string(),
                fields: vec![Field {
                    name: "data".to_string(),
                    field_type: FieldType {
                        base_type: "string".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                }],
                constants: vec![],
                source: String::new(),
                path: PathBuf::new(),
            },
            type_hash: TypeHash([0u8; 32]),
            definition: String::new(),
        };

        let result = generate_python_message(&msg).unwrap();

        assert!(result.contains("class String(msgspec.Struct"));
        assert!(result.contains("data: str = ''"));
        assert!(result.contains("__msgtype__"));
        assert!(result.contains("std_msgs/msg/String"));
    }

    #[test]
    fn test_generate_python_package() {
        let msg = ResolvedMessage {
            parsed: ParsedMessage {
                name: "Simple".to_string(),
                package: "test_msgs".to_string(),
                fields: vec![Field {
                    name: "value".to_string(),
                    field_type: FieldType {
                        base_type: "int32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                }],
                constants: vec![],
                source: String::new(),
                path: PathBuf::new(),
            },
            type_hash: TypeHash([0u8; 32]),
            definition: String::new(),
        };

        let result = generate_python_package("test_msgs", &[msg], &[]).unwrap();

        assert!(result.contains("import msgspec"));
        assert!(result.contains("class Simple"));
        assert!(result.contains("value: int = 0"));
    }
}
