pub mod action;
pub mod msg;
pub mod srv;

use crate::types::{ArrayType, Constant, DefaultValue, Field, FieldType};
use anyhow::{Context, Result, bail};

/// Strip comments from a line (everything after '#')
pub fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or("")
}

/// Parse a constant line (format: "type NAME = value")
pub fn parse_constant(line: &str, line_num: usize) -> Result<Constant> {
    let parts: Vec<&str> = line.splitn(2, '=').collect();
    if parts.len() != 2 {
        bail!("Line {}: Invalid constant format, expected '='", line_num);
    }

    let type_and_name: Vec<&str> = parts[0].split_whitespace().collect();
    if type_and_name.is_empty() || type_and_name.len() > 2 {
        bail!(
            "Line {}: Invalid constant declaration, expected 'type NAME'",
            line_num
        );
    }

    let (const_type, name) = if type_and_name.len() == 2 {
        (type_and_name[0].to_string(), type_and_name[1].to_string())
    } else {
        (type_and_name[0].to_string(), String::new())
    };

    Ok(Constant {
        const_type,
        name,
        value: parts[1].trim().to_string(),
    })
}

/// Parse a field line (format: "type name" or "type name default_value")
pub fn parse_field(line: &str, source_package: &str, line_num: usize) -> Result<Field> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    if parts.len() < 2 {
        bail!("Line {}: Invalid field format: {}", line_num, line);
    }

    let field_type = parse_field_type(parts[0], source_package)?;
    let name = parts[1].to_string();

    let default = if parts.len() > 2 {
        Some(parse_default_value(&parts[2..].join(" "))?)
    } else {
        None
    };

    Ok(Field {
        name,
        field_type,
        default,
    })
}

/// Parse a field type string (e.g., "uint8[]", "geometry_msgs/Point", "string<=50")
pub fn parse_field_type(type_str: &str, _source_package: &str) -> Result<FieldType> {
    // Handle bounded strings: string<=50 (but NOT arrays like uint8[<=10])
    if let Some(idx) = type_str.find("<=") {
        // Check if this is a bounded array (has brackets) or bounded string (no brackets)
        if !type_str.contains('[') {
            let base = &type_str[..idx];
            let bound = type_str[idx + 2..]
                .parse::<usize>()
                .with_context(|| format!("Invalid string bound in '{}'", type_str))?;
            // Return bounded string with the bound stored
            return Ok(FieldType {
                base_type: base.to_string(),
                package: None,
                array: ArrayType::Single,
                string_bound: Some(bound),
            });
        }
    }

    // Handle arrays: type[], type[N], type[<=N]
    let (base_str, array) = if let Some(idx) = type_str.find('[') {
        if !type_str.ends_with(']') {
            bail!("Invalid array syntax: {}", type_str);
        }

        let base = &type_str[..idx];
        let array_str = &type_str[idx + 1..type_str.len() - 1];

        let array = if array_str.is_empty() {
            ArrayType::Unbounded
        } else if let Some(stripped) = array_str.strip_prefix("<=") {
            ArrayType::Bounded(
                stripped
                    .parse()
                    .with_context(|| format!("Invalid bounded array size in '{}'", type_str))?,
            )
        } else {
            ArrayType::Fixed(
                array_str
                    .parse()
                    .with_context(|| format!("Invalid fixed array size in '{}'", type_str))?,
            )
        };

        (base, array)
    } else {
        (type_str, ArrayType::Single)
    };

    // Extract string bound from base_str if it's a bounded string used in an array context
    // e.g. "string<=10" in "string<=10[]" -> base="string", string_bound=Some(10)
    let (base_str, string_bound) = if let Some(idx) = base_str.find("<=") {
        if !base_str.contains('[') {
            let base = &base_str[..idx];
            let bound = base_str[idx + 2..]
                .parse::<usize>()
                .with_context(|| format!("Invalid string bound in '{}'", type_str))?;
            (base, Some(bound))
        } else {
            (base_str, None)
        }
    } else {
        (base_str, None)
    };

    // Parse package/Type or Type
    let (package, base_type) = if let Some(idx) = base_str.find('/') {
        let pkg = base_str[..idx].to_string();
        let typ = base_str[idx + 1..].to_string();
        (Some(pkg), typ)
    } else {
        // Special case: Header -> std_msgs/Header
        if base_str == "Header" {
            (Some("std_msgs".to_string()), "Header".to_string())
        } else {
            (None, base_str.to_string())
        }
    };

    Ok(FieldType {
        base_type,
        package,
        array,
        string_bound,
    })
}

/// Parse a default value
pub fn parse_default_value(s: &str) -> Result<DefaultValue> {
    if s.starts_with('[') && s.ends_with(']') {
        // Parse array
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() {
            bail!("Empty array default value: {}", s);
        }
        let elements: Vec<&str> = inner.split(',').map(|e| e.trim()).collect();
        if elements.is_empty() {
            bail!("Empty array default value: {}", s);
        }

        // Try to parse as bool array
        let mut bools = Vec::new();
        let mut all_bool = true;
        for elem in &elements {
            if *elem == "true" || *elem == "True" {
                bools.push(true);
            } else if *elem == "false" || *elem == "False" {
                bools.push(false);
            } else {
                all_bool = false;
                break;
            }
        }
        if all_bool {
            return Ok(DefaultValue::BoolArray(bools));
        }

        // Try to parse as int array
        let mut ints = Vec::new();
        let mut all_int = true;
        for elem in &elements {
            if let Ok(i) = elem.parse::<i64>() {
                ints.push(i);
            } else {
                all_int = false;
                break;
            }
        }
        if all_int {
            return Ok(DefaultValue::IntArray(ints));
        }

        // Try to parse as float array
        let mut floats = Vec::new();
        let mut all_float = true;
        for elem in &elements {
            if let Ok(f) = elem.parse::<f64>() {
                floats.push(f);
            } else {
                all_float = false;
                break;
            }
        }
        if all_float {
            return Ok(DefaultValue::FloatArray(floats));
        }

        // Try to parse as string array
        let mut strings = Vec::new();
        let mut all_string = true;
        for elem in &elements {
            if (elem.starts_with('"') && elem.ends_with('"'))
                || (elem.starts_with('\'') && elem.ends_with('\''))
            {
                strings.push(elem[1..elem.len() - 1].to_string());
            } else {
                all_string = false;
                break;
            }
        }
        if all_string {
            return Ok(DefaultValue::StringArray(strings));
        }

        bail!("Invalid array default value: {}", s);
    } else if s == "true" || s == "True" {
        Ok(DefaultValue::Bool(true))
    } else if s == "false" || s == "False" {
        Ok(DefaultValue::Bool(false))
    } else if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))
    {
        Ok(DefaultValue::String(s[1..s.len() - 1].to_string()))
    } else if let Ok(i) = s.parse::<i64>() {
        Ok(DefaultValue::Int(i))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(DefaultValue::Float(f))
    } else {
        bail!("Invalid default value: {}", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_comment() {
        assert_eq!(strip_comment("uint8 data # comment"), "uint8 data ");
        assert_eq!(strip_comment("uint8 data"), "uint8 data");
        assert_eq!(strip_comment("# only comment"), "");
    }

    #[test]
    fn test_parse_constant() {
        let constant = parse_constant("uint8 TYPE_A = 1", 0).unwrap();
        assert_eq!(constant.const_type, "uint8");
        assert_eq!(constant.name, "TYPE_A");
        assert_eq!(constant.value, "1");
    }

    #[test]
    fn test_parse_field_primitive() {
        let field = parse_field("uint8 data", "test_pkg", 0).unwrap();
        assert_eq!(field.name, "data");
        assert_eq!(field.field_type.base_type, "uint8");
        assert_eq!(field.field_type.array, ArrayType::Single);
        assert!(field.field_type.package.is_none());
    }

    #[test]
    fn test_parse_field_type_unbounded_array() {
        let field_type = parse_field_type("uint8[]", "test_pkg").unwrap();
        assert_eq!(field_type.base_type, "uint8");
        assert_eq!(field_type.array, ArrayType::Unbounded);
    }

    #[test]
    fn test_parse_field_type_fixed_array() {
        let field_type = parse_field_type("uint8[10]", "test_pkg").unwrap();
        assert_eq!(field_type.base_type, "uint8");
        assert_eq!(field_type.array, ArrayType::Fixed(10));
    }

    #[test]
    fn test_parse_field_type_bounded_array() {
        let field_type = parse_field_type("uint8[<=10]", "test_pkg").unwrap();
        assert_eq!(field_type.base_type, "uint8");
        assert_eq!(field_type.array, ArrayType::Bounded(10));
    }

    #[test]
    fn test_parse_field_type_with_package() {
        let field_type = parse_field_type("geometry_msgs/Point", "test_pkg").unwrap();
        assert_eq!(field_type.base_type, "Point");
        assert_eq!(field_type.package, Some("geometry_msgs".to_string()));
        assert_eq!(field_type.array, ArrayType::Single);
    }

    #[test]
    fn test_parse_field_type_header_special_case() {
        let field_type = parse_field_type("Header", "test_pkg").unwrap();
        assert_eq!(field_type.base_type, "Header");
        assert_eq!(field_type.package, Some("std_msgs".to_string()));
    }

    #[test]
    fn test_parse_default_value_bool() {
        match parse_default_value("true").unwrap() {
            DefaultValue::Bool(b) => assert!(b),
            _ => panic!("Expected Bool"),
        }
        match parse_default_value("false").unwrap() {
            DefaultValue::Bool(b) => assert!(!b),
            _ => panic!("Expected Bool"),
        }
    }

    #[test]
    fn test_parse_default_value_int() {
        match parse_default_value("42").unwrap() {
            DefaultValue::Int(i) => assert_eq!(i, 42),
            _ => panic!("Expected Int"),
        }
        match parse_default_value("-100").unwrap() {
            DefaultValue::Int(i) => assert_eq!(i, -100),
            _ => panic!("Expected Int"),
        }
    }

    #[test]
    fn test_parse_default_value_float() {
        match parse_default_value("1.23").unwrap() {
            DefaultValue::Float(f) => assert!((f - 1.23).abs() < 0.001),
            _ => panic!("Expected Float"),
        }
    }

    #[test]
    fn test_parse_default_value_string() {
        match parse_default_value("\"hello\"").unwrap() {
            DefaultValue::String(s) => assert_eq!(s, "hello"),
            _ => panic!("Expected String"),
        }
        match parse_default_value("'world'").unwrap() {
            DefaultValue::String(s) => assert_eq!(s, "world"),
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_parse_default_value_bool_array() {
        match parse_default_value("[false, true, false]").unwrap() {
            DefaultValue::BoolArray(b) => assert_eq!(b, vec![false, true, false]),
            _ => panic!("Expected BoolArray"),
        }
    }

    #[test]
    fn test_parse_default_value_int_array() {
        match parse_default_value("[1, 2, 3]").unwrap() {
            DefaultValue::IntArray(i) => assert_eq!(i, vec![1, 2, 3]),
            _ => panic!("Expected IntArray"),
        }
    }

    #[test]
    fn test_parse_default_value_float_array() {
        match parse_default_value("[1.1, 2.2, 3.3]").unwrap() {
            DefaultValue::FloatArray(f) => {
                assert!((f[0] - 1.1).abs() < 0.001);
                assert!((f[1] - 2.2).abs() < 0.001);
                assert!((f[2] - 3.3).abs() < 0.001);
            }
            _ => panic!("Expected FloatArray"),
        }
    }

    #[test]
    fn test_parse_default_value_string_array() {
        match parse_default_value("[\"a\", \"b\", \"c\"]").unwrap() {
            DefaultValue::StringArray(s) => assert_eq!(s, vec!["a", "b", "c"]),
            _ => panic!("Expected StringArray"),
        }
    }
}
