//! The re-entrancy violation reaches the operator through a path the default
//! panic hook does not use.
//!
//! The violation panics with a typed payload, which the bus matches to re-raise
//! it. Rust's default hook prints a non-string payload as `Box<dyn Any>`, so the
//! callback site and the repair instruction would vanish from the output.
//! The raise site therefore also logs the message with `tracing`.
//!
//! The test runs the violation in a child process, because it must read real
//! stderr and must not replace the panic hook of the shared test process. The
//! child has two modes. With a subscriber it must print the message. Without one
//! it prints only what the default hook prints, which must not contain it. The
//! second mode is the negative control: it shows the text comes from the
//! tracing line and not from the hook.

#![cfg(debug_assertions)]

use std::process::Command;

use hiroz::reentrancy::{TrackedMutex, assert_no_guards_held};

const MODE_ENV: &str = "HIROZ_REENTRANCY_CHILD";
const SITE: &str = "child_site";
const NEEDLE: &str = "re-entrancy rule violated at `child_site`";

/// Runs only inside the child process. In the parent it returns at once.
#[test]
fn child_raises_the_violation() {
    let Ok(mode) = std::env::var(MODE_ENV) else {
        return;
    };
    if mode == "with_subscriber" {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init();
    }
    let raised = std::panic::catch_unwind(|| {
        let m = TrackedMutex::new(0u32);
        let _guard = m.lock().unwrap();
        assert_no_guards_held(SITE);
    });
    assert!(raised.is_err(), "the violation did not fire");
}

fn run_child(mode: &str) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_raises_the_violation", "--nocapture"])
        .env(MODE_ENV, mode)
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn the child test process");
    assert!(
        out.status.success(),
        "the child failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_violation_message_reaches_stderr_through_tracing() {
    let with = run_child("with_subscriber");
    assert!(
        with.contains(NEEDLE),
        "the diagnostic is missing from stderr:\n{with}"
    );
    assert!(
        with.contains("1 lock guard(s)"),
        "the guard count is missing from stderr:\n{with}"
    );
}

#[test]
fn the_default_panic_hook_alone_does_not_print_the_message() {
    let without = run_child("without_subscriber");
    assert!(
        without.contains("Box<dyn Any"),
        "the control did not observe the default hook output:\n{without}"
    );
    assert!(
        !without.contains(NEEDLE),
        "the default hook printed the message, so the tracing line proves nothing:\n{without}"
    );
}
