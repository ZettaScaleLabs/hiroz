//! File-wide count of the allocator pin call in `context.rs`.
//!
//! `rmw-zenoh-rs` needs ROS headers to build, so this test reads its source instead of calling
//! `rmw_init`. It counts the lines of `context.rs` that contain the full path
//! `hiroz::malloc_pin::pin_large_message_thresholds()`. It asserts that exactly one line
//! matches.
//!
//! The test fails in these cases:
//! - The call is removed.
//! - The call is commented out on its own line with `//` or inside `/* */`.
//! - The call occurs more than once.
//! - The call is written under a shortened path, for example after a `use` import.
//!
//! A trailing comment after code on the same line does not hide the call.
//!
//! The match is file-wide in `context.rs`, not scoped to `rmw_init`. A call in any other
//! function satisfies it. The test does not fail in these cases:
//! - The call is present but unreachable.
//! - The call sits in a string literal.
//! - The call is inside a `#[cfg(test)]` block.
//! - The call is behind a `cfg` that is off.

use std::path::Path;

const CONTEXT_RS: &str = "../rmw-zenoh-rs/src/context.rs";
const CALL: &str = "hiroz::malloc_pin::pin_large_message_thresholds()";

/// `src` without its block comments, nested ones included. Newlines stay, so line numbers do not
/// move. A `/*` inside a string literal is not told apart from a comment.
fn strip_block_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let (mut i, mut depth) = (0, 0usize);
    while i < b.len() {
        if b[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if depth > 0 && b[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
        } else {
            let c = src[i..].chars().next().expect("char");
            if depth == 0 || c == '\n' {
                out.push(c);
            }
            i += c.len_utf8();
        }
    }
    out
}

/// Lines of `src` that call the pin, ignoring line and block comments.
fn pin_calls(src: &str) -> Vec<usize> {
    strip_block_comments(src)
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(CALL))
        .map(|(i, _)| i + 1)
        .collect()
}

/// Counts the allocator pin call file-wide in `context.rs`, in context with the comment
/// stripping above, and expects exactly one. It does not check which function holds the call.
#[test]
fn rmw_init_calls_the_allocator_pin() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTEXT_RS);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let calls = pin_calls(&src);
    println!("PIN_CALL_LINES {calls:?}");
    assert_eq!(
        calls.len(),
        1,
        "expected exactly one call to {CALL} in rmw-zenoh-rs context.rs, found lines {calls:?}"
    );
}

#[test]
fn comment_only_mention_is_not_a_call() {
    let src = format!("    // {CALL}\n    let x = 1;\n");
    assert!(pin_calls(&src).is_empty());
    let src = format!("    match {CALL} {{\n");
    assert_eq!(pin_calls(&src), vec![1]);
}

#[test]
fn block_comment_is_not_a_call() {
    let src = format!("/*\n    match {CALL} {{\n    }}\n*/\nlet x = 1;\n");
    assert!(pin_calls(&src).is_empty());
    let src = format!("/* a /* nested */ still comment {CALL} */\n{CALL};\n");
    assert_eq!(pin_calls(&src), vec![2]);
}

/// The part of `src` that handles the `Rejected` outcome, up to the next outcome arm.
fn rejected_arm(src: &str) -> &str {
    let start = src
        .find("PinOutcome::Rejected")
        .expect("context.rs handles PinOutcome::Rejected");
    let rest = &src[start..];
    let end = rest[1..].find("PinOutcome::").map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

/// Whether the `Rejected` arm of `src` logs at error level.
fn logs_at_error_level(src: &str) -> bool {
    rejected_arm(src).contains("tracing::error!")
}

/// `rmw_init` installs an `error` log filter just before the pin, so a rejection that is logged
/// below `error` never prints in the default configuration.
#[test]
fn a_rejected_pin_is_logged_at_error_level() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTEXT_RS);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert!(
        logs_at_error_level(&src),
        "the Rejected arm must log at error level: {}",
        rejected_arm(&src)
    );
}

#[test]
fn rejected_arm_ends_at_the_next_outcome() {
    let src = "match o {\n PinOutcome::Rejected { .. } => { tracing::error!(\"x\"); }\n PinOutcome::Pinned { .. } => { tracing::debug!(\"y\"); }\n}";
    let arm = rejected_arm(src);
    assert!(arm.contains("error!") && !arm.contains("debug!"), "{arm}");
}

#[test]
fn trailing_comment_is_not_a_call() {
    let src = format!("    let x = 1; // {CALL}\n");
    assert!(pin_calls(&src).is_empty());
    let src = format!("    match {CALL} {{ // keep\n");
    assert_eq!(pin_calls(&src), vec![1]);
}

#[test]
fn a_commented_out_error_log_does_not_satisfy_the_rejected_arm_check() {
    let src = "match o {\n PinOutcome::Rejected { .. } => {\n // tracing::error!(\"x\");\n tracing::debug!(\"y\"); /* tracing::error!(\"z\") */ }\n PinOutcome::Pinned { .. } => {}\n}";
    assert!(!logs_at_error_level(src));
    let src = "match o {\n PinOutcome::Rejected { .. } => { tracing::error!(\"x\"); }\n PinOutcome::Pinned { .. } => {}\n}";
    assert!(logs_at_error_level(src));
}
