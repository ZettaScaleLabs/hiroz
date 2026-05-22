use crate::parser::{parse_constant, parse_field, strip_comment};
use crate::types::ParsedMessage;
use anyhow::{Context, Result};
use std::path::Path;

/// Parse a .msg file from a path
pub fn parse_msg_file(path: &Path, package: &str) -> Result<ParsedMessage> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_msg_string(&source, package, path)
}

/// Parse a .msg file from a string
pub fn parse_msg_string(source: &str, package: &str, path: &Path) -> Result<ParsedMessage> {
    let name = path
        .file_stem()
        .context("Invalid filename")?
        .to_str()
        .context("Non-UTF8 filename")?
        .to_string();

    let mut fields = Vec::new();
    let mut constants = Vec::new();

    for (line_num, line) in source.lines().enumerate() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }

        // Check if this is a constant (contains '=' but not bounded string/array like string<=255 or uint8[<=10])
        let is_constant = if let Some(eq_pos) = line.find('=') {
            // Check if the '=' is part of '<=' which is used for bounded types
            if eq_pos > 0 && line.as_bytes().get(eq_pos - 1) == Some(&b'<') {
                // This is a bounded type, not a constant
                false
            } else {
                // Check if there are brackets before the '='
                let before_eq = &line[..eq_pos];
                // If there's an opening bracket without a closing one, the '=' is inside brackets
                let open_brackets = before_eq.matches('[').count();
                let close_brackets = before_eq.matches(']').count();
                // It's a constant if brackets are balanced (not inside an array spec)
                open_brackets == close_brackets
            }
        } else {
            false
        };

        if is_constant {
            match parse_constant(line, line_num) {
                Ok(c) if !c.name.is_empty() => constants.push(c),
                Ok(_) => {}  // Skip constants with empty name
                Err(_) => {} // Skip invalid constants
            }
        } else {
            fields.push(parse_field(line, package, line_num)?);
        }
    }

    Ok(ParsedMessage {
        name,
        package: package.to_string(),
        fields,
        constants,
        source: source.to_string(),
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ArrayType;
    use std::path::PathBuf;

    #[test]
    fn test_parse_simple_message() {
        let msg_content = "uint8 data\nstring name";
        let path = PathBuf::from("Simple.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.name, "Simple");
        assert_eq!(msg.package, "test_msgs");
        assert_eq!(msg.fields.len(), 2);
        assert_eq!(msg.fields[0].name, "data");
        assert_eq!(msg.fields[0].field_type.base_type, "uint8");
        assert_eq!(msg.fields[1].name, "name");
        assert_eq!(msg.fields[1].field_type.base_type, "string");
    }

    #[test]
    fn test_parse_message_with_comments() {
        let msg_content = r#"
# This is a comment
uint8 data  # inline comment
# Another comment
string name
"#;
        let path = PathBuf::from("Test.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.fields.len(), 2);
        assert_eq!(msg.fields[0].name, "data");
        assert_eq!(msg.fields[1].name, "name");
    }

    #[test]
    fn test_parse_message_with_constants() {
        let msg_content = r#"
uint8 TYPE_A = 1
uint8 TYPE_B = 2
uint8 type
string name
"#;
        let path = PathBuf::from("WithConstants.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.constants.len(), 2);
        assert_eq!(msg.constants[0].name, "TYPE_A");
        assert_eq!(msg.constants[0].value, "1");
        assert_eq!(msg.constants[1].name, "TYPE_B");
        assert_eq!(msg.constants[1].value, "2");
        assert_eq!(msg.fields.len(), 2);
    }

    #[test]
    fn test_parse_message_with_arrays() {
        let msg_content = r#"
uint8[] unbounded
uint8[10] fixed
uint8[<=20] bounded
"#;
        let path = PathBuf::from("Arrays.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.fields.len(), 3);
        assert_eq!(msg.fields[0].field_type.array, ArrayType::Unbounded);
        assert_eq!(msg.fields[1].field_type.array, ArrayType::Fixed(10));
        assert_eq!(msg.fields[2].field_type.array, ArrayType::Bounded(20));
    }

    #[test]
    fn test_parse_message_with_nested_types() {
        let msg_content = r#"
std_msgs/Header header
geometry_msgs/Point position
geometry_msgs/Point[] waypoints
"#;
        let path = PathBuf::from("Nested.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.fields.len(), 3);
        assert_eq!(
            msg.fields[0].field_type.package,
            Some("std_msgs".to_string())
        );
        assert_eq!(msg.fields[0].field_type.base_type, "Header");
        assert_eq!(
            msg.fields[1].field_type.package,
            Some("geometry_msgs".to_string())
        );
        assert_eq!(msg.fields[1].field_type.base_type, "Point");
        assert_eq!(msg.fields[2].field_type.array, ArrayType::Unbounded);
    }

    #[test]
    fn test_parse_message_with_header_shorthand() {
        let msg_content = "Header header\nuint8 data";
        let path = PathBuf::from("WithHeader.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.fields.len(), 2);
        // Header should be expanded to std_msgs/Header
        assert_eq!(
            msg.fields[0].field_type.package,
            Some("std_msgs".to_string())
        );
        assert_eq!(msg.fields[0].field_type.base_type, "Header");
    }

    #[test]
    fn test_parse_empty_message() {
        let msg_content = "# Just comments\n# Nothing else";
        let path = PathBuf::from("Empty.msg");

        let msg = parse_msg_string(msg_content, "test_msgs", &path).unwrap();

        assert_eq!(msg.fields.len(), 0);
        assert_eq!(msg.constants.len(), 0);
    }

    #[test]
    fn test_parse_std_msgs_string() {
        let msg_content = "string data";
        let path = PathBuf::from("String.msg");

        let msg = parse_msg_string(msg_content, "std_msgs", &path).unwrap();

        assert_eq!(msg.name, "String");
        assert_eq!(msg.package, "std_msgs");
        assert_eq!(msg.fields.len(), 1);
        assert_eq!(msg.fields[0].name, "data");
        assert_eq!(msg.fields[0].field_type.base_type, "string");
    }

    #[test]
    fn test_parse_bounded_string() {
        let msg_content = r#"
string<=255 type_name
Field[] fields
"#;
        let path = PathBuf::from("IndividualTypeDescription.msg");

        let msg = parse_msg_string(msg_content, "type_description_interfaces", &path).unwrap();

        assert_eq!(msg.fields.len(), 2);
        assert_eq!(msg.fields[0].name, "type_name");
        assert_eq!(msg.fields[0].field_type.base_type, "string");
        assert_eq!(msg.fields[1].name, "fields");
    }

    #[test]
    fn test_parse_bounded_string_with_constants() {
        // This tests that bounded strings don't get confused with constants
        let msg_content = r#"
uint8 FIELD_TYPE_STRING = 17
uint8 type_id 0
string<=255 nested_type_name
"#;
        let path = PathBuf::from("FieldType.msg");

        let msg = parse_msg_string(msg_content, "type_description_interfaces", &path).unwrap();

        assert_eq!(msg.constants.len(), 1);
        assert_eq!(msg.constants[0].name, "FIELD_TYPE_STRING");
        assert_eq!(msg.constants[0].value, "17");
        assert_eq!(msg.fields.len(), 2);
        assert_eq!(msg.fields[0].name, "type_id");
        assert_eq!(msg.fields[1].name, "nested_type_name");
        assert_eq!(msg.fields[1].field_type.base_type, "string");
    }
}
