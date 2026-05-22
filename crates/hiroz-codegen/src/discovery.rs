use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::parser::{action::parse_action_file, msg::parse_msg_file, srv::parse_srv_file};
use crate::types::{ParsedAction, ParsedMessage, ParsedService};

/// Discover and parse all messages in a package directory
pub fn discover_messages(package_path: &Path, package_name: &str) -> Result<Vec<ParsedMessage>> {
    let mut messages = Vec::new();

    let msg_dir = package_path.join("msg");
    if msg_dir.exists() && msg_dir.is_dir() {
        for entry in fs::read_dir(&msg_dir)
            .with_context(|| format!("Failed to read msg directory: {:?}", msg_dir))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("msg") {
                let msg = parse_msg_file(&path, package_name)
                    .with_context(|| format!("Failed to parse message file: {:?}", path))?;
                messages.push(msg);
            }
        }
    }

    Ok(messages)
}

/// Discover and parse all services in a package directory
pub fn discover_services(package_path: &Path, package_name: &str) -> Result<Vec<ParsedService>> {
    let mut services = Vec::new();

    let srv_dir = package_path.join("srv");
    if srv_dir.exists() && srv_dir.is_dir() {
        for entry in fs::read_dir(&srv_dir)
            .with_context(|| format!("Failed to read srv directory: {:?}", srv_dir))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("srv") {
                let srv = parse_srv_file(&path, package_name)
                    .with_context(|| format!("Failed to parse service file: {:?}", path))?;
                services.push(srv);
            }
        }
    }

    Ok(services)
}

/// Discover and parse all actions in a package directory
pub fn discover_actions(package_path: &Path, package_name: &str) -> Result<Vec<ParsedAction>> {
    let mut actions = Vec::new();

    let action_dir = package_path.join("action");
    if action_dir.exists() && action_dir.is_dir() {
        for entry in fs::read_dir(&action_dir)
            .with_context(|| format!("Failed to read action directory: {:?}", action_dir))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("action") {
                let action = parse_action_file(&path, package_name)
                    .with_context(|| format!("Failed to parse action file: {:?}", path))?;
                actions.push(action);
            }
        }
    }

    Ok(actions)
}

/// Discover package name from package.xml or directory name
pub fn discover_package_name(package_path: &Path) -> Result<String> {
    // Try to read from package.xml
    let package_xml = package_path.join("package.xml");
    if package_xml.exists() {
        let content = fs::read_to_string(&package_xml)
            .with_context(|| format!("Failed to read package.xml: {:?}", package_xml))?;

        // Simple XML parsing to extract <name>
        if let Some(start) = content.find("<name>")
            && let Some(end) = content[start..].find("</name>")
        {
            let name = &content[start + 6..start + end];
            return Ok(name.trim().to_string());
        }
    }

    // Fallback to directory name
    package_path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot determine package name from path: {:?}",
                package_path
            )
        })
}

/// Discover all messages, services, and actions from multiple package paths
pub fn discover_all(
    package_paths: &[&Path],
) -> Result<(Vec<ParsedMessage>, Vec<ParsedService>, Vec<ParsedAction>)> {
    let mut all_messages = Vec::new();
    let mut all_services = Vec::new();
    let mut all_actions = Vec::new();

    for &package_path in package_paths {
        let package_name = discover_package_name(package_path)?;

        let messages = discover_messages(package_path, &package_name)?;
        let services = discover_services(package_path, &package_name)?;
        let actions = discover_actions(package_path, &package_name)?;

        all_messages.extend(messages);
        all_services.extend(services);
        all_actions.extend(actions);
    }

    Ok((all_messages, all_services, all_actions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_discover_package_name_from_xml() {
        let temp_dir = TempDir::new().unwrap();
        let package_xml = temp_dir.path().join("package.xml");

        fs::write(
            &package_xml,
            r#"<?xml version="1.0"?>
<package format="3">
  <name>test_msgs</name>
  <version>1.0.0</version>
</package>"#,
        )
        .unwrap();

        let name = discover_package_name(temp_dir.path()).unwrap();
        assert_eq!(name, "test_msgs");
    }

    #[test]
    fn test_discover_package_name_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let package_dir = temp_dir.path().join("my_package");
        fs::create_dir(&package_dir).unwrap();

        let name = discover_package_name(&package_dir).unwrap();
        assert_eq!(name, "my_package");
    }

    #[test]
    fn test_discover_messages_empty() {
        let temp_dir = TempDir::new().unwrap();
        let messages = discover_messages(temp_dir.path(), "test_pkg").unwrap();
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_discover_messages() {
        let temp_dir = TempDir::new().unwrap();
        let msg_dir = temp_dir.path().join("msg");
        fs::create_dir(&msg_dir).unwrap();

        fs::write(msg_dir.join("Simple.msg"), "int32 value\n").unwrap();
        fs::write(msg_dir.join("Another.msg"), "string data\n").unwrap();

        let messages = discover_messages(temp_dir.path(), "test_pkg").unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_discover_services() {
        let temp_dir = TempDir::new().unwrap();
        let srv_dir = temp_dir.path().join("srv");
        fs::create_dir(&srv_dir).unwrap();

        fs::write(
            srv_dir.join("AddTwoInts.srv"),
            "int64 a\nint64 b\n---\nint64 sum\n",
        )
        .unwrap();

        let services = discover_services(temp_dir.path(), "test_pkg").unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].name, "AddTwoInts");
    }
}
