use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::parser::{action::parse_action_file, msg::parse_msg_file, srv::parse_srv_file};
use crate::types::{ParsedAction, ParsedMessage, ParsedService};

/// The files in `package_path/<kind>` with extension `kind`, sorted by path.
///
/// Sorted because `read_dir` order is unspecified: it differs between
/// filesystems, and between two copies of the same tree (a build sandbox, a
/// fresh checkout), which would make the generated code differ as well.
fn interface_files(package_path: &Path, kind: &str) -> Result<Vec<PathBuf>> {
    let dir = package_path.join(kind);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("Failed to read {} directory: {:?}", kind, dir))?
    {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some(kind) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Discover and parse all messages in a package directory
pub fn discover_messages(package_path: &Path, package_name: &str) -> Result<Vec<ParsedMessage>> {
    interface_files(package_path, "msg")?
        .iter()
        .map(|path| {
            parse_msg_file(path, package_name)
                .with_context(|| format!("Failed to parse message file: {:?}", path))
        })
        .collect()
}

/// Discover and parse all services in a package directory
pub fn discover_services(package_path: &Path, package_name: &str) -> Result<Vec<ParsedService>> {
    interface_files(package_path, "srv")?
        .iter()
        .map(|path| {
            parse_srv_file(path, package_name)
                .with_context(|| format!("Failed to parse service file: {:?}", path))
        })
        .collect()
}

/// Discover and parse all actions in a package directory
pub fn discover_actions(package_path: &Path, package_name: &str) -> Result<Vec<ParsedAction>> {
    interface_files(package_path, "action")?
        .iter()
        .map(|path| {
            parse_action_file(path, package_name)
                .with_context(|| format!("Failed to parse action file: {:?}", path))
        })
        .collect()
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
    fn test_discover_messages_sorted() {
        let temp_dir = TempDir::new().unwrap();
        let msg_dir = temp_dir.path().join("msg");
        fs::create_dir(&msg_dir).unwrap();

        // Created out of order: the result must not depend on directory order.
        for name in ["Zone", "Alpha", "Zones", "Middle"] {
            fs::write(msg_dir.join(format!("{name}.msg")), "int32 value\n").unwrap();
        }
        fs::write(msg_dir.join("README.md"), "not a message\n").unwrap();

        let messages = discover_messages(temp_dir.path(), "test_pkg").unwrap();
        let names: Vec<_> = messages.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Middle", "Zone", "Zones"]);
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
