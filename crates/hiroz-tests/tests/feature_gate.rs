//! Fails loudly when `hiroz-tests` is built without the features its suites need.
//!
//! Ten suites in this crate are `#![cfg(feature = "ros-msgs")]` — `cache.rs`,
//! `lifecycle.rs`, `parameter_tests.rs`, `service_schema_discovery.rs`,
//! `subscriber_timeout.rs`, `type_description_integration.rs`, the `z_*_example`
//! suites, and others. A crate-level `cfg` that is not satisfied does not error
//! and does not warn: the file compiles to an empty test binary reporting
//! `0 passed`, which is indistinguishable from green in every runner and log.
//!
//! Two separate mechanisms could hide that, and they are worth keeping distinct:
//!
//! * **Selection.** `Cargo.toml` sets
//!   `default-members = ["crates/hiroz", "crates/hiroz-codegen"]`, so a bare
//!   `cargo nextest run` never selected `hiroz-tests` at all — the crate was not
//!   built, rather than built empty. `scripts/test-pure-rust.nu` now names the
//!   crate explicitly.
//! * **Empty compilation.** Ask for the crate without features and the gated
//!   suites really do compile away to `0 passed`. That is what this file and the
//!   build script guard against.
//!
//! This file is deliberately **not** gated, so it is the one thing in the crate
//! that a featureless build cannot compile away. If the features go missing
//! again, the run fails with a message naming the cause instead of quietly
//! shrinking.
//!
//! Consequence, stated so it is a decision rather than a surprise: `hiroz-tests`
//! has no supported featureless configuration. Run it as
//! `cargo test -p hiroz-tests --features ros-msgs,jazzy` (or with `ros-interop`
//! for the suites that also need a ROS installation).

/// Without `ros-msgs`, the gated suites are silently absent — fail instead.
#[test]
#[cfg(not(feature = "ros-msgs"))]
fn ros_msgs_gated_suites_must_not_be_silently_skipped() {
    panic!(
        "hiroz-tests was built without the `ros-msgs` feature.\n\
         \n\
         The suites gated on it — `cache.rs`, `lifecycle.rs`, \
         `parameter_tests.rs` and others — have been \
         compiled to empty test binaries and will report `0 passed`, which reads \
         as green. That is not a pass; it is no coverage.\n\
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
