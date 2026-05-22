use crate::parser::msg::parse_msg_string;
use crate::types::ParsedService;
use anyhow::{Context, Result, bail};
use std::path::Path;

/// Parse a .srv file from a path
pub fn parse_srv_file(path: &Path, package: &str) -> Result<ParsedService> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_srv_string(&source, package, path)
}

/// Parse a .srv file from a string
pub fn parse_srv_string(source: &str, package: &str, path: &Path) -> Result<ParsedService> {
    let name = path
        .file_stem()
        .context("Invalid filename")?
        .to_str()
        .context("Non-UTF8 filename")?
        .to_string();

    // Split on "---" delimiter (handle different line endings)
    // First normalize by finding lines that are just "---"
    let lines: Vec<&str> = source.lines().collect();
    let mut delimiter_indices = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            delimiter_indices.push(i);
        }
    }

    if delimiter_indices.len() != 1 {
        bail!(
            "Service must have exactly one '---' delimiter, found {}",
            delimiter_indices.len()
        );
    }

    let delimiter_idx = delimiter_indices[0];
    let request_lines = &lines[..delimiter_idx];
    let response_lines = &lines[delimiter_idx + 1..];

    let request_source = request_lines.join("\n");
    let response_source = response_lines.join("\n");

    // Parse request and response as messages
    let request_path = path.with_file_name(format!("{}Request.msg", name));
    let response_path = path.with_file_name(format!("{}Response.msg", name));

    let mut request = parse_msg_string(&request_source, package, &request_path)?;
    request.name = format!("{}Request", name);

    let mut response = parse_msg_string(&response_source, package, &response_path)?;
    response.name = format!("{}Response", name);

    Ok(ParsedService {
        name,
        package: package.to_string(),
        request,
        response,
        source: source.to_string(),
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_simple_service() {
        let srv_content = "int64 a\nint64 b\n---\nint64 sum";
        let path = PathBuf::from("AddTwoInts.srv");

        let srv = parse_srv_string(srv_content, "example_interfaces", &path).unwrap();

        assert_eq!(srv.name, "AddTwoInts");
        assert_eq!(srv.package, "example_interfaces");
        assert_eq!(srv.request.name, "AddTwoIntsRequest");
        assert_eq!(srv.request.fields.len(), 2);
        assert_eq!(srv.request.fields[0].name, "a");
        assert_eq!(srv.request.fields[1].name, "b");
        assert_eq!(srv.response.name, "AddTwoIntsResponse");
        assert_eq!(srv.response.fields.len(), 1);
        assert_eq!(srv.response.fields[0].name, "sum");
    }

    #[test]
    fn test_parse_service_with_comments() {
        let srv_content = r#"
# Request
int64 a  # first number
int64 b  # second number
---
# Response
int64 sum  # result
"#;
        let path = PathBuf::from("AddTwoInts.srv");

        let srv = parse_srv_string(srv_content, "example_interfaces", &path).unwrap();

        assert_eq!(srv.request.fields.len(), 2);
        assert_eq!(srv.response.fields.len(), 1);
    }

    #[test]
    fn test_parse_service_empty_request() {
        let srv_content = "---\nbool success\nstring message";
        let path = PathBuf::from("Trigger.srv");

        let srv = parse_srv_string(srv_content, "std_srvs", &path).unwrap();

        assert_eq!(srv.name, "Trigger");
        assert_eq!(srv.request.fields.len(), 0);
        assert_eq!(srv.response.fields.len(), 2);
        assert_eq!(srv.response.fields[0].field_type.base_type, "bool");
        assert_eq!(srv.response.fields[1].field_type.base_type, "string");
    }

    #[test]
    fn test_parse_service_empty_response() {
        let srv_content = "string command\n---";
        let path = PathBuf::from("Execute.srv");

        let srv = parse_srv_string(srv_content, "test_srvs", &path).unwrap();

        assert_eq!(srv.request.fields.len(), 1);
        assert_eq!(srv.response.fields.len(), 0);
    }

    #[test]
    fn test_parse_service_with_nested_types() {
        let srv_content = r#"
std_msgs/Header header
geometry_msgs/Point target
---
bool success
geometry_msgs/Point result
"#;
        let path = PathBuf::from("GetPoint.srv");

        let srv = parse_srv_string(srv_content, "test_srvs", &path).unwrap();

        assert_eq!(srv.request.fields.len(), 2);
        assert_eq!(
            srv.request.fields[0].field_type.package,
            Some("std_msgs".to_string())
        );
        assert_eq!(
            srv.request.fields[1].field_type.package,
            Some("geometry_msgs".to_string())
        );
        assert_eq!(srv.response.fields.len(), 2);
        assert_eq!(
            srv.response.fields[1].field_type.package,
            Some("geometry_msgs".to_string())
        );
    }

    #[test]
    fn test_parse_service_missing_delimiter() {
        let srv_content = "int64 a\nint64 b\nint64 sum";
        let path = PathBuf::from("Invalid.srv");

        let result = parse_srv_string(srv_content, "test_srvs", &path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("---"));
    }

    #[test]
    fn test_parse_service_multiple_delimiters() {
        let srv_content = "int64 a\n---\nint64 b\n---\nint64 c";
        let path = PathBuf::from("Invalid.srv");

        let result = parse_srv_string(srv_content, "test_srvs", &path);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_service_with_arrays() {
        let srv_content = r#"
float64[] points
---
bool success
float64[] transformed_points
"#;
        let path = PathBuf::from("Transform.srv");

        let srv = parse_srv_string(srv_content, "test_srvs", &path).unwrap();

        assert_eq!(srv.request.fields.len(), 1);
        assert_eq!(srv.response.fields.len(), 2);
    }
}
