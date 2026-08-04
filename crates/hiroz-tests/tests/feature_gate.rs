//! Fails loudly when `hiroz-tests` is built without the features its suites need.
//!
//! Several suites here carry a crate-level `#![cfg(feature = "ros-msgs")]`. An
//! unsatisfied crate-level `cfg` neither errors nor warns: the file compiles to
//! an empty test binary reporting `0 passed`, which no runner or log can
//! distinguish from green.
//!
//! This file is deliberately **not** gated, so it is the one target a
//! featureless build cannot compile away. `build.rs` enforces the same
//! requirement for the whole package, covering the narrower `--test <name>`
//! invocations that never select this target.
//!
//! Consequence: `hiroz-tests` has no supported featureless configuration. Run it
//! as `cargo test -p hiroz-tests --features ros-msgs,jazzy`, or with
//! `ros-interop,<distro>` for the suites that also drive a ROS installation.
//!
//! Note that selection is a separate failure mode from compilation. `Cargo.toml`
//! sets `default-members` to exclude this crate, so a bare `cargo nextest run`
//! skips it entirely rather than building it empty; `scripts/test-pure-rust.nu`
//! names it explicitly to close that path.

/// Without `ros-msgs`, the gated suites are silently absent — fail instead.
#[test]
#[cfg(not(feature = "ros-msgs"))]
fn ros_msgs_gated_suites_must_not_be_silently_skipped() {
    panic!(
        "hiroz-tests was built without the `ros-msgs` feature.\n\
         \n\
         The suites gated on it — `cache.rs`, `subscriber_timeout.rs`, \
         `service_schema_discovery.rs`, the `z_*_example` suites and others — \
         have been compiled to empty test binaries and will report `0 passed`, \
         which reads as green. That is not a pass; it is no coverage.\n\
         \n\
         Build the crate with its features:\n\
         \n\
             cargo test -p hiroz-tests --features ros-msgs,jazzy\n\
         \n\
         Suites that additionally drive a real ROS 2 installation need \
         `--features ros-interop,<distro>` instead."
    );
}

/// With `ros-msgs`, record that the gate was satisfied.
///
/// Present so the guard is visible in the test list in *both* configurations —
/// a check whose only evidence is the absence of a failure is not a check.
#[test]
#[cfg(feature = "ros-msgs")]
fn ros_msgs_gated_suites_are_compiled_in() {}
