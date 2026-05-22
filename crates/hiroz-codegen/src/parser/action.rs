use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{parse_constant, parse_field, strip_comment};
use crate::types::{ParsedAction, ParsedMessage};

/// Parse a .action file into a ParsedAction
///
/// Action files have three sections separated by '---':
/// 1. Goal definition (fields for the goal request)
/// 2. Result definition (fields for the result response)
/// 3. Feedback definition (fields for progress feedback)
///
/// Example action file format:
/// ```text
/// # Request
/// int32 order
/// ---
/// # Result
/// int32[] sequence
/// ---
/// # Feedback
/// int32[] partial_sequence
/// ```
pub fn parse_action_file(path: &Path, package: &str) -> Result<ParsedAction> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read action file: {:?}", path))?;

    let action_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .context("Invalid action file name")?
        .to_string();

    parse_action(&content, &action_name, package, path)
}

/// Parse action definition from string content
pub fn parse_action(
    content: &str,
    action_name: &str,
    package: &str,
    path: &Path,
) -> Result<ParsedAction> {
    // Split by "---" separator (three sections: goal, result, feedback)
    // Use line scanning to handle files with no leading/trailing newlines around delimiters
    let lines: Vec<&str> = content.lines().collect();
    let delimiter_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i)
        .collect();

    if delimiter_indices.len() != 2 {
        bail!(
            "Action file must have exactly 3 sections (goal, result, feedback), found {}",
            delimiter_indices.len() + 1
        );
    }

    let goal_content = lines[..delimiter_indices[0]].join("\n");
    let result_content = lines[delimiter_indices[0] + 1..delimiter_indices[1]].join("\n");
    let feedback_content = lines[delimiter_indices[1] + 1..].join("\n");

    // Parse each section as a message
    let goal = parse_action_section(
        &goal_content,
        &format!("{}Goal", action_name),
        package,
        path,
    )?;

    let result = if result_content.trim().is_empty() {
        None
    } else {
        Some(parse_action_section(
            &result_content,
            &format!("{}Result", action_name),
            package,
            path,
        )?)
    };

    let feedback = if feedback_content.trim().is_empty() {
        None
    } else {
        Some(parse_action_section(
            &feedback_content,
            &format!("{}Feedback", action_name),
            package,
            path,
        )?)
    };

    Ok(ParsedAction {
        name: action_name.to_string(),
        package: package.to_string(),
        goal,
        result,
        feedback,
        source: content.to_string(),
        path: path.to_path_buf(),
    })
}

/// Parse a single section of an action definition (goal/result/feedback)
fn parse_action_section(
    content: &str,
    name: &str,
    package: &str,
    path: &Path,
) -> Result<ParsedMessage> {
    let mut fields = Vec::new();
    let mut constants = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = strip_comment(line).trim();

        // Skip empty lines
        if line.is_empty() {
            continue;
        }

        // Check if it's a constant (contains '=')
        if line.contains('=') {
            let constant = parse_constant(line, line_num + 1)?;
            constants.push(constant);
        } else {
            // Parse as field
            let field = parse_field(line, package, line_num + 1)?;
            fields.push(field);
        }
    }

    Ok(ParsedMessage {
        name: name.to_string(),
        package: package.to_string(),
        fields,
        constants,
        source: content.to_string(),
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_parse_simple_action() {
        let content = r#"# Goal definition
int32 order
---
# Result definition
int32[] sequence
---
# Feedback
int32[] partial_sequence
"#;

        let path = PathBuf::from("Fibonacci.action");
        let result = parse_action(content, "Fibonacci", "action_tutorials_interfaces", &path);

        assert!(result.is_ok());
        let action = result.unwrap();

        assert_eq!(action.name, "Fibonacci");
        assert_eq!(action.package, "action_tutorials_interfaces");

        // Check goal
        assert_eq!(action.goal.name, "FibonacciGoal");
        assert_eq!(action.goal.fields.len(), 1);
        assert_eq!(action.goal.fields[0].name, "order");
        assert_eq!(action.goal.fields[0].field_type.base_type, "int32");

        // Check result
        let result = action.result.as_ref().unwrap();
        assert_eq!(result.name, "FibonacciResult");
        assert_eq!(result.fields.len(), 1);
        assert_eq!(result.fields[0].name, "sequence");
        assert_eq!(result.fields[0].field_type.base_type, "int32");

        // Check feedback
        let feedback = action.feedback.as_ref().unwrap();
        assert_eq!(feedback.name, "FibonacciFeedback");
        assert_eq!(feedback.fields.len(), 1);
        assert_eq!(feedback.fields[0].name, "partial_sequence");
        assert_eq!(feedback.fields[0].field_type.base_type, "int32");
    }

    #[test]
    fn test_parse_action_with_empty_feedback() {
        let content = r#"# Goal
geometry_msgs/PoseStamped target_pose
---
# Result
bool success
string message
---
# no feedback
"#;

        let path = PathBuf::from("Navigate.action");
        let result = parse_action(content, "Navigate", "nav_msgs", &path);

        assert!(result.is_ok());
        let action = result.unwrap();

        // Goal should have 1 field
        assert_eq!(action.goal.fields.len(), 1);

        // Result should have 2 fields
        assert_eq!(action.result.as_ref().unwrap().fields.len(), 2);

        // Feedback should be empty
        assert_eq!(action.feedback.as_ref().unwrap().fields.len(), 0);
    }

    #[test]
    fn test_parse_action_with_constants() {
        let content = r#"# Goal with constants
int32 MODE_FAST = 1
int32 MODE_SLOW = 2
int32 mode
---
# Result
bool success
---
# Feedback
float32 progress
"#;

        let path = PathBuf::from("Process.action");
        let result = parse_action(content, "Process", "test_msgs", &path);

        assert!(result.is_ok());
        let action = result.unwrap();

        // Goal should have constants and a field
        assert_eq!(action.goal.constants.len(), 2);
        assert_eq!(action.goal.fields.len(), 1);
        assert_eq!(action.goal.constants[0].name, "MODE_FAST");
        assert_eq!(action.goal.constants[0].value, "1");
    }

    #[test]
    fn test_parse_action_invalid_sections() {
        // Only 2 sections
        let content = r#"int32 value
---
int32 result
"#;

        let path = PathBuf::from("Invalid.action");
        let result = parse_action(content, "Invalid", "test", &path);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exactly 3 sections")
        );
    }

    #[test]
    fn test_parse_all_empty_sections() {
        // "---\n---\n" — valid, all three sections empty
        let content = "---\n---\n";
        let path = PathBuf::from("Empty.action");
        let result = parse_action(content, "Empty", "test_pkg", &path);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert!(action.goal.fields.is_empty());
        assert!(action.result.is_none());
        assert!(action.feedback.is_none());
    }

    #[test]
    fn test_parse_action_with_content_no_trailing_newline() {
        // Valid file without trailing newline after last section
        let content = "string test1\n---\nstring test2\n---\nstring test3";
        let path = PathBuf::from("NoTrail.action");
        let result = parse_action(content, "NoTrail", "test_pkg", &path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_action_comment_only_sections() {
        // Sections containing only comments — non-empty strings, produce Some with zero fields
        let content = "#goal\n---\n#result\n---\n#feedback\n";
        let path = PathBuf::from("CommentOnly.action");
        let result = parse_action(content, "CommentOnly", "test_pkg", &path);
        assert!(result.is_ok());
        let action = result.unwrap();
        assert!(action.goal.fields.is_empty());
        assert_eq!(action.result.as_ref().unwrap().fields.len(), 0);
        assert_eq!(action.feedback.as_ref().unwrap().fields.len(), 0);
    }

    #[test]
    fn test_parse_action_with_nested_types() {
        let content = r#"# Goal
geometry_msgs/Point target
std_msgs/Header header
---
# Result
bool reached
---
# Feedback
float64 distance_remaining
"#;

        let path = PathBuf::from("MoveToPoint.action");
        let result = parse_action(content, "MoveToPoint", "nav_msgs", &path);

        assert!(result.is_ok());
        let action = result.unwrap();

        assert_eq!(action.goal.fields.len(), 2);
        assert_eq!(
            action.goal.fields[0].field_type.package,
            Some("geometry_msgs".to_string())
        );
        assert_eq!(
            action.goal.fields[1].field_type.package,
            Some("std_msgs".to_string())
        );
    }
}
