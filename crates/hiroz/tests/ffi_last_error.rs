//! A warning that goes through the FFI warning macro must reach `hiroz_last_error_message`.
//!
//! An integration test links the library once, so it runs where the lib unit-test binary does not link.
//! It reaches the store through public C functions only: recording, clearing, replacement, per-thread isolation and a valid C string.

use std::ffi::CStr;

use hiroz::ffi::{
    context::{CContextConfig, hiroz_context_create_with_config},
    last_error::{hiroz_last_error_clear, hiroz_last_error_message},
};

fn current() -> Option<String> {
    let ptr = hiroz_last_error_message();
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned())
}

#[test]
fn null_config_records_a_message() {
    std::thread::spawn(|| {
        hiroz_last_error_clear();
        // Positive control: nothing is recorded before the call.
        assert_eq!(current(), None);

        let ctx = unsafe { hiroz_context_create_with_config(std::ptr::null()) };

        assert!(ctx.is_null());
        let msg = current().expect("the converted warning must record a message");
        assert!(msg.contains("Null config pointer"), "{msg}");
    })
    .join()
    .unwrap();
}

#[test]
fn clear_removes_a_recorded_message() {
    std::thread::spawn(|| {
        let _ = unsafe { hiroz_context_create_with_config(std::ptr::null()) };
        assert!(current().is_some());
        hiroz_last_error_clear();
        assert_eq!(current(), None);
    })
    .join()
    .unwrap();
}

/// Message recorded by the first call to `bad_remap_call` on this thread.
const REMAP_MARK: &str = "Invalid remap rules";

/// Run a real entry point that records a message and returns NULL before it builds a context.
///
/// The config sets a non-UTF-8 `mode`, so the string helper records a message first.
/// The malformed remap rule then replaces it, so the final message names the rule.
fn bad_remap_call() {
    let mode = std::ffi::CString::new(vec![0xFF, 0xFE]).unwrap();
    let rule = std::ffi::CString::new("not a remap rule \u{e9}").unwrap();
    let rules = [rule.as_ptr()];
    let cfg = CContextConfig {
        domain_id: 0,
        config_file: std::ptr::null(),
        connect_endpoints: std::ptr::null(),
        connect_endpoints_count: 0,
        mode: mode.as_ptr(),
        disable_multicast_scouting: false,
        connect_to_local_zenohd: false,
        json_config: std::ptr::null(),
        remap_rules: rules.as_ptr(),
        remap_rules_count: 1,
        enable_logging: false,
        namespace: std::ptr::null(),
    };
    let ctx = unsafe { hiroz_context_create_with_config(&cfg) };
    assert!(ctx.is_null(), "a malformed remap rule must fail the call");
}

fn null_config_call() {
    let ctx = unsafe { hiroz_context_create_with_config(std::ptr::null()) };
    assert!(ctx.is_null());
}

#[test]
fn a_second_failure_replaces_the_first_message() {
    std::thread::spawn(|| {
        hiroz_last_error_clear();
        null_config_call();
        let first = current().expect("the null config records a message");
        assert!(first.contains("Null config pointer"), "{first}");

        bad_remap_call();
        let second = current().expect("the remap failure records a message");
        assert!(second.contains(REMAP_MARK), "{second}");
        assert!(!second.contains("Null config pointer"), "{second}");

        hiroz_last_error_clear();
        assert_eq!(current(), None);
    })
    .join()
    .unwrap();
}

#[test]
/// The replacement of an interior NUL byte is not reachable here: a C argument cannot hold one, and `set` is crate-private.
fn the_message_stays_a_valid_c_string_after_non_utf8_and_non_ascii_arguments() {
    std::thread::spawn(|| {
        hiroz_last_error_clear();
        bad_remap_call();
        let ptr = hiroz_last_error_message();
        assert!(!ptr.is_null());
        // `CStr::from_ptr` finds the first NUL, so a valid message has none before its end.
        let bytes = unsafe { CStr::from_ptr(ptr) }.to_bytes();
        let msg = std::str::from_utf8(bytes).expect("the message must be valid UTF-8");
        assert!(msg.contains(REMAP_MARK), "{msg}");
    })
    .join()
    .unwrap();
}

#[test]
fn messages_do_not_cross_threads() {
    use std::sync::mpsc::channel;

    std::thread::spawn(|| {
        hiroz_last_error_clear();
        null_config_call();
        let mine = current().expect("parent message");
        assert!(mine.contains("Null config pointer"), "{mine}");

        let (to_child, child_go) = channel::<()>();
        let (to_parent, parent_go) = channel::<()>();
        let child = std::thread::spawn(move || {
            // Positive control: the child starts empty although the parent recorded a message.
            assert_eq!(current(), None, "the parent message leaked into the child");
            bad_remap_call();
            let theirs = current().expect("child message");
            assert!(theirs.contains(REMAP_MARK), "{theirs}");
            to_parent.send(()).unwrap();
            child_go.recv().unwrap();
            // The parent clears its own slot meanwhile. The child keeps its message.
            assert_eq!(current().as_deref(), Some(theirs.as_str()));
        });

        parent_go.recv().unwrap();
        // The child message did not replace the parent message.
        assert_eq!(current().as_deref(), Some(mine.as_str()));
        hiroz_last_error_clear();
        assert_eq!(current(), None);
        to_child.send(()).unwrap();
        child.join().unwrap();

        // A thread that exited leaves nothing behind for the parent or for a new thread.
        assert_eq!(current(), None);
        std::thread::spawn(|| assert_eq!(current(), None, "a new thread starts empty"))
            .join()
            .unwrap();
    })
    .join()
    .unwrap();
}
