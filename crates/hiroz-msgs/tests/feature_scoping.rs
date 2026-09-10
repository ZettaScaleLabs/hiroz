//! Asserts that `hiroz-msgs`' Cargo features scope codegen -- a build that
//! generates too much still compiles cleanly, so `cargo check` alone can't
//! catch over-generation. Checks the real output via
//! `hiroz_msgs::GENERATED_PACKAGES`.
//!
//! Only exercises the feature set this binary was built with:
//!
//! ```text
//! cargo test -p hiroz-msgs --no-default-features --features std_msgs
//! ```

/// `std_msgs` has no dependent packages of its own, so with only that
/// feature enabled, `visualization_msgs` and `rosbag2_interfaces` (neither
/// in its dependency closure) should never be generated.
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
            "`{unrelated}` was generated with only `std_msgs` enabled. Generated: {generated:?}"
        );
    }
}
