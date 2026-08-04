//! Fails loudly when `hiroz-tests` is built without the features its suites need.
//!
//! Several suites carry a crate-level `#![cfg(feature = "ros-msgs")]`. An
//! unsatisfied crate-level `cfg` neither errors nor warns — the file compiles to
//! an empty test binary reporting `0 passed`, indistinguishable from green.
//!
//! This file is deliberately ungated, so a featureless build cannot compile it
//! away. `build.rs` covers the narrower `--test <name>` invocations that never
//! select this target.
//!
//! `hiroz-tests` therefore has no supported featureless configuration: run it as
//! `cargo test -p hiroz-tests --features ros-msgs,jazzy`, or with
//! `ros-interop,<distro>` for suites that drive a real ROS installation.
//!
//! Selection is a separate failure mode from compilation: this crate is not in
//! `default-members`, so a bare `cargo nextest run` skips it rather than
//! building it empty. `scripts/test-pure-rust.nu` names it explicitly.

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
