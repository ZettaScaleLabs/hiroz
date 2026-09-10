//! Assert that `hiroz-msgs`' Cargo feature flags scope codegen to the
//! requested packages, instead of silently generating the full distro
//! package set no matter which features are enabled.
//!
//! This pins a regression: an upstream change dropped `build.rs`'s
//! `selected_package_names` filtering, so every
//! build generated every package in the bundled asset tree regardless of
//! which Cargo features were on. `cargo check` did not catch it -- a build
//! that generates too much still compiles cleanly. Catching this needs a
//! positive assertion on *which* packages were generated, not just that the
//! crate builds.
//!
//! Run narrow, single-package feature sets and check the generated set
//! directly via `hiroz_msgs::GENERATED_PACKAGES` (embedded from `build.rs`'s
//! real `discover_ros_packages` output -- see that file and
//! `crates/hiroz-msgs/src/lib.rs`).
//!
//! This test only exercises the feature set this binary was actually built
//! with (there's no way for one test binary to also build the crate with a
//! different feature set). Run it under the narrow feature set CI checks:
//!
//! ```text
//! cargo test -p hiroz-msgs --no-default-features --features std_msgs
//! ```

/// `std_msgs` (Cargo.toml: `std_msgs = []`) has no dependent-package entries
/// of its own, so with `--no-default-features --features std_msgs` the only
/// packages generated should be `std_msgs` plus the handful of packages
/// `selected_package_names` always requires (`builtin_interfaces`,
/// `action_msgs`, `unique_identifier_msgs`, `lifecycle_msgs`, and — for
/// non-Humble distros — `service_msgs`/`type_description_interfaces`).
///
/// `visualization_msgs` and `rosbag2_interfaces` are picked because neither
/// is in `std_msgs`'s dependency closure (`Cargo.toml`:
/// `visualization_msgs = ["std_msgs", "geometry_msgs", "sensor_msgs"]` --
/// note this is the *reverse* direction, visualization_msgs depends on
/// std_msgs, not the other way around -- and `rosbag2_interfaces = []` has
/// no relation to `std_msgs` at all). Neither should ever appear here.
#[test]
fn std_msgs_feature_does_not_pull_in_unrelated_packages() {
    let generated: Vec<&str> = hiroz_msgs::GENERATED_PACKAGES
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    assert!(
        generated.contains(&"std_msgs"),
        "expected `std_msgs` to be generated for the `std_msgs` feature, got: {generated:?}"
    );

    for unrelated in ["visualization_msgs", "rosbag2_interfaces"] {
        assert!(
            !generated.contains(&unrelated),
            "`{unrelated}` was generated even though only the `std_msgs` feature was \
             enabled -- every Cargo feature used to generate the full distro package set \
             regardless of which was selected. Generated: {generated:?}"
        );
    }
}
