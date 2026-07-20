//! Formatters for converting DynamicMessage to various output formats
//!
//! Provides JSON and human-readable text formatting for dynamic messages.

use hiroz::dynamic::{DynamicMessage, DynamicValue};
use serde_json;

/// Convert a DynamicMessage to a JSON value
///
/// Recursively converts all fields, handling nested messages and arrays.
pub fn dynamic_message_to_json(msg: &DynamicMessage) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    for (name, value) in msg.iter() {
        fields.insert(name.to_string(), dynamic_value_to_json(value));
    }
    serde_json::Value::Object(fields)
}

/// Convert a DynamicValue to a JSON value
///
/// Handles all DynamicValue variants including primitives, nested messages,
/// and arrays/sequences.
pub fn dynamic_value_to_json(value: &DynamicValue) -> serde_json::Value {
    match value {
        DynamicValue::Bool(b) => serde_json::Value::Bool(*b),
        DynamicValue::Int8(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Int16(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Int32(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Int64(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Uint8(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Uint16(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Uint32(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Uint64(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Float32(f) => serde_json::Number::from_f64(*f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynamicValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynamicValue::String(s) => serde_json::Value::String(s.clone()),
        DynamicValue::Bytes(b) => {
            // Encode bytes as space-separated hex string (e.g. "de ad be ef")
            serde_json::Value::String(
                b.iter()
                    .map(|x| format!("{x:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        DynamicValue::Message(msg) => dynamic_message_to_json(msg),
        DynamicValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(dynamic_value_to_json).collect())
        }
    }
}

/// Format a DynamicMessage as human-readable indented text
///
/// Produces output like:
/// ```text
/// data: "Hello World"
/// linear:
///   x: 1.0
///   y: 0.0
///   z: 0.0
/// ```
pub fn format_message_pretty(msg: &DynamicMessage) -> String {
    let mut output = String::new();
    for (name, value) in msg.iter() {
        format_value_pretty(&mut output, name, value, 0);
    }
    output
}

/// Recursively format a DynamicValue with indentation
fn push_scalar(output: &mut String, prefix: &str, name: &str, val: impl std::fmt::Display) {
    output.push_str(&format!("{}{}: {}\n", prefix, name, val));
}

fn format_value_pretty(output: &mut String, name: &str, value: &DynamicValue, indent: usize) {
    let prefix = "  ".repeat(indent);

    match value {
        DynamicValue::Bool(b) => push_scalar(output, &prefix, name, b),
        DynamicValue::Int8(i) => push_scalar(output, &prefix, name, i),
        DynamicValue::Int16(i) => push_scalar(output, &prefix, name, i),
        DynamicValue::Int32(i) => push_scalar(output, &prefix, name, i),
        DynamicValue::Int64(i) => push_scalar(output, &prefix, name, i),
        DynamicValue::Uint8(u) => push_scalar(output, &prefix, name, u),
        DynamicValue::Uint16(u) => push_scalar(output, &prefix, name, u),
        DynamicValue::Uint32(u) => push_scalar(output, &prefix, name, u),
        DynamicValue::Uint64(u) => push_scalar(output, &prefix, name, u),
        DynamicValue::Float32(f) => push_scalar(output, &prefix, name, f),
        DynamicValue::Float64(f) => push_scalar(output, &prefix, name, f),
        DynamicValue::String(s) => {
            output.push_str(&format!("{}{}: \"{}\"\n", prefix, name, s));
        }
        DynamicValue::Bytes(b) => {
            output.push_str(&format!("{}{}: [bytes: {} bytes]\n", prefix, name, b.len()));
        }
        DynamicValue::Message(msg) => {
            output.push_str(&format!("{}{}:\n", prefix, name));
            for (n, v) in msg.iter() {
                format_value_pretty(output, n, v, indent + 1);
            }
        }
        DynamicValue::Array(arr) => {
            if arr.is_empty() {
                output.push_str(&format!("{}{}[]: []\n", prefix, name));
            } else {
                output.push_str(&format!("{}{}[{}]:\n", prefix, name, arr.len()));
                for (i, v) in arr.iter().enumerate() {
                    format_value_pretty(output, &format!("[{}]", i), v, indent + 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiroz::dynamic::{FieldType, MessageSchema};

    #[test]
    fn test_json_primitives() {
        let schema = MessageSchema::builder("test_msgs/msg/Primitives")
            .field("flag", FieldType::Bool)
            .field("count", FieldType::Int32)
            .field("value", FieldType::Float64)
            .field("name", FieldType::String)
            .build()
            .unwrap();

        let mut msg = DynamicMessage::new(&schema);
        msg.set("flag", true).unwrap();
        msg.set("count", 42i32).unwrap();
        msg.set("value", std::f64::consts::PI).unwrap();
        msg.set("name", "test".to_string()).unwrap();

        let json = dynamic_message_to_json(&msg);
        assert_eq!(json["flag"], true);
        assert_eq!(json["count"], 42);
        assert_eq!(json["value"], std::f64::consts::PI);
        assert_eq!(json["name"], "test");
    }

    #[test]
    fn test_pretty_format_simple() {
        let schema = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let mut msg = DynamicMessage::new(&schema);
        msg.set("data", "Hello World".to_string()).unwrap();

        let formatted = format_message_pretty(&msg);
        assert!(formatted.contains("data: \"Hello World\""));
    }

    #[test]
    fn test_nested_message() {
        let inner_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let outer_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
            .field("linear", FieldType::Message(inner_schema.clone()))
            .field("angular", FieldType::Message(inner_schema))
            .build()
            .unwrap();

        let mut msg = DynamicMessage::new(&outer_schema);
        msg.set("linear.x", 1.0f64).unwrap();
        msg.set("linear.y", 0.0f64).unwrap();
        msg.set("linear.z", 0.0f64).unwrap();

        let json = dynamic_message_to_json(&msg);
        assert_eq!(json["linear"]["x"], 1.0);
        assert_eq!(json["linear"]["y"], 0.0);

        let formatted = format_message_pretty(&msg);
        assert!(formatted.contains("linear:"));
        assert!(formatted.contains("  x: 1"));
    }
}
