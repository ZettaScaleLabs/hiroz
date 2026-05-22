//! ROS 2-style YAML parameter file loading.
//!
//! Supports the standard ROS 2 parameter file format:
//!
//! ```yaml
//! /**:
//!   ros__parameters:
//!     my_param: 42
//!     another: "hello"
//!
//! /my_node:
//!   ros__parameters:
//!     node_specific: true
//!
//! /my_ns/my_node:
//!   ros__parameters:
//!     nested_param: 3.14
//! ```
//!
//! Node patterns support wildcards: `/**` matches all nodes.
//! The node selector must match `/{namespace}/{node_name}` or just
//! `/{node_name}` for root-namespace nodes.

use std::collections::HashMap;
use std::path::Path;

use serde_yaml::Value;

use super::types::ParameterValue;

/// Load parameter overrides from a YAML file for the given node.
///
/// Returns a map of parameter name → value containing only the parameters
/// applicable to the specified node (by its fully-qualified name).
///
/// # Format
///
/// ```yaml
/// /**:
///   ros__parameters:
///     global_param: 1
///
/// /my_node:
///   ros__parameters:
///     local_param: "hello"
/// ```
pub fn load_parameter_file(
    path: &Path,
    node_fqn: &str,
) -> Result<HashMap<String, ParameterValue>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read parameter file {:?}: {}", path, e))?;
    load_parameter_string(&content, node_fqn)
}

/// Parse a YAML string and extract parameter overrides for the given node.
pub fn load_parameter_string(
    yaml: &str,
    node_fqn: &str,
) -> Result<HashMap<String, ParameterValue>, String> {
    let doc: Value =
        serde_yaml::from_str(yaml).map_err(|e| format!("Failed to parse YAML: {}", e))?;

    let mapping = doc
        .as_mapping()
        .ok_or_else(|| "YAML root must be a mapping".to_string())?;

    let mut result = HashMap::new();

    for (key, node_val) in mapping {
        let selector = key
            .as_str()
            .ok_or_else(|| "YAML keys must be strings".to_string())?;

        if !matches_node(selector, node_fqn) {
            continue;
        }

        // node_val should be a mapping with a "ros__parameters" key
        let node_map = node_val
            .as_mapping()
            .ok_or_else(|| format!("Value for '{}' must be a mapping", selector))?;

        let params_key = Value::String("ros__parameters".to_string());
        if let Some(params) = node_map.get(&params_key) {
            let params_map = params
                .as_mapping()
                .ok_or_else(|| "ros__parameters must be a mapping".to_string())?;

            for (pname, pval) in params_map {
                let name = pname
                    .as_str()
                    .ok_or_else(|| "Parameter names must be strings".to_string())?;

                if let Some(value) = yaml_value_to_parameter(pval) {
                    // Later entries override earlier ones (more specific selectors win)
                    result.insert(name.to_string(), value);
                }
            }
        }
    }

    Ok(result)
}

/// Check whether a node selector matches the given fully-qualified node name.
///
/// Selectors:
/// - `/**` — matches any node
/// - `/some_ns/**` — matches any node under `/some_ns/`
/// - `/node_name` — exact match
/// - `/ns/node_name` — exact match
fn matches_node(selector: &str, node_fqn: &str) -> bool {
    if selector == "/**" || selector == "**" {
        return true;
    }

    if let Some(prefix) = selector.strip_suffix("/**") {
        return node_fqn.starts_with(prefix);
    }

    if let Some(prefix) = selector.strip_suffix("/*") {
        // Match one level: /ns/* matches /ns/foo but not /ns/foo/bar
        let rest = node_fqn.strip_prefix(prefix).unwrap_or("");
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return !rest.is_empty() && !rest.contains('/');
    }

    selector == node_fqn
}

/// Convert a YAML value to a ParameterValue.
///
/// Type inference rules (matching rclcpp behavior):
/// - Integer YAML values → Integer
/// - Float YAML values → Double
/// - Boolean YAML values → Bool
/// - String YAML values → String
/// - Sequence of integers → IntegerArray
/// - Sequence of floats → DoubleArray
/// - Sequence of bools → BoolArray
/// - Sequence of strings → StringArray
/// - Sequence of bytes → ByteArray (if all items are 0-255 integers)
fn yaml_value_to_parameter(val: &Value) -> Option<ParameterValue> {
    match val {
        Value::Bool(b) => Some(ParameterValue::Bool(*b)),
        Value::Number(n) => n
            .as_i64()
            .map(ParameterValue::Integer)
            .or_else(|| n.as_f64().map(ParameterValue::Double)),
        Value::String(s) => Some(ParameterValue::String(s.clone())),
        Value::Sequence(seq) => infer_sequence_type(seq),
        Value::Null => Some(ParameterValue::NotSet),
        Value::Mapping(_) => None, // Nested mappings not supported as parameter values
        _ => None,
    }
}

fn infer_sequence_type(seq: &[Value]) -> Option<ParameterValue> {
    if seq.is_empty() {
        // Empty sequence — default to StringArray
        return Some(ParameterValue::StringArray(vec![]));
    }

    // Determine element type from the first element
    match seq.first()? {
        Value::Bool(_) => {
            let bools: Option<Vec<bool>> = seq.iter().map(|v| v.as_bool()).collect();
            bools.map(ParameterValue::BoolArray)
        }
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            // Check if it could be a byte array (all in 0-255 range)
            let ints: Option<Vec<i64>> = seq.iter().map(|v| v.as_i64()).collect();
            let ints = ints?;
            let is_byte_array = ints.iter().all(|&i| (0..=255).contains(&i));
            if is_byte_array {
                Some(ParameterValue::ByteArray(
                    ints.iter().map(|&i| i as u8).collect(),
                ))
            } else {
                Some(ParameterValue::IntegerArray(ints))
            }
        }
        Value::Number(_) => {
            // Float sequence
            let floats: Option<Vec<f64>> = seq.iter().map(|v| v.as_f64()).collect();
            floats.map(ParameterValue::DoubleArray)
        }
        Value::String(_) => {
            let strings: Option<Vec<String>> = seq
                .iter()
                .map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            strings.map(ParameterValue::StringArray)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML_SAMPLE: &str = r#"
/**:
  ros__parameters:
    global_int: 42
    global_str: "hello"
    global_bool: true
    global_float: 2.5

/my_node:
  ros__parameters:
    node_specific: 99
    override_me: "original"

/other_node:
  ros__parameters:
    not_mine: 1
"#;

    #[test]
    fn test_wildcard_match() {
        let params = load_parameter_string(YAML_SAMPLE, "/my_node").unwrap();
        // Gets both wildcard and node-specific
        assert_eq!(params["global_int"], ParameterValue::Integer(42));
        assert_eq!(
            params["global_str"],
            ParameterValue::String("hello".to_string())
        );
        assert_eq!(params["global_bool"], ParameterValue::Bool(true));
        assert_eq!(params["global_float"], ParameterValue::Double(2.5));
        assert_eq!(params["node_specific"], ParameterValue::Integer(99));
    }

    #[test]
    fn test_no_other_node_params() {
        let params = load_parameter_string(YAML_SAMPLE, "/my_node").unwrap();
        assert!(!params.contains_key("not_mine"));
    }

    #[test]
    fn test_exact_node_only() {
        let params = load_parameter_string(YAML_SAMPLE, "/other_node").unwrap();
        assert!(params.contains_key("not_mine"));
        assert!(params.contains_key("global_int")); // wildcard still applies
        assert!(!params.contains_key("node_specific"));
    }

    #[test]
    fn test_arrays() {
        let yaml = r#"
/**:
  ros__parameters:
    int_list: [1000, 2000, 3000]
    float_list: [1.0, 2.0, 3.0]
    str_list: ["a", "b"]
    bool_list: [true, false, true]
"#;
        let params = load_parameter_string(yaml, "/any_node").unwrap();
        assert_eq!(
            params["int_list"],
            ParameterValue::IntegerArray(vec![1000, 2000, 3000])
        );
        assert_eq!(
            params["float_list"],
            ParameterValue::DoubleArray(vec![1.0, 2.0, 3.0])
        );
        assert_eq!(
            params["str_list"],
            ParameterValue::StringArray(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            params["bool_list"],
            ParameterValue::BoolArray(vec![true, false, true])
        );
    }

    #[test]
    fn test_namespace_selector() {
        let yaml = r#"
/my_ns/**:
  ros__parameters:
    ns_param: 1
/other_ns/**:
  ros__parameters:
    other_param: 2
"#;
        let params = load_parameter_string(yaml, "/my_ns/my_node").unwrap();
        assert!(params.contains_key("ns_param"));
        assert!(!params.contains_key("other_param"));
    }

    #[test]
    fn test_matches_node() {
        assert!(matches_node("/**", "/any/node"));
        assert!(matches_node("/**", "/node"));
        assert!(matches_node("/my_ns/**", "/my_ns/node"));
        assert!(!matches_node("/my_ns/**", "/other_ns/node"));
        assert!(matches_node("/my_node", "/my_node"));
        assert!(!matches_node("/my_node", "/other_node"));
    }
}
