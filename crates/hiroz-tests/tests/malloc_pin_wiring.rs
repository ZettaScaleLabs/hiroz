//! `rmw_init` must call the allocator pin.
//!
//! `rmw-zenoh-rs` needs ROS headers to build, so this test reads its source instead of calling
//! `rmw_init`. It fails when the call to `hiroz::malloc_pin::pin_large_message_thresholds` is
//! removed from `rmw_init`, or when it is only commented out. It does not fail when the call
//! is present but unreachable.

use std::path::Path;

const CONTEXT_RS: &str = "../rmw-zenoh-rs/src/context.rs";
const CALL: &str = "hiroz::malloc_pin::pin_large_message_thresholds()";

/// Lines of `src` that call the pin, ignoring comment lines.
fn pin_calls(src: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(CALL))
        .map(|(i, _)| i + 1)
        .collect()
}

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
