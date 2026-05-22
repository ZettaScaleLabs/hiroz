//! Golden hash regression tests for real ROS 2 message types.
//!
//! These tests load real message definitions from the bundled Jazzy assets,
//! resolve the full type tree, and verify that the computed RIHS01 hashes:
//!
//! 1. Have the correct format (`RIHS01_` + 64 hex chars)
//! 2. Are deterministic across multiple calls
//! 3. Match the expected values (computed from the fixed implementation and
//!    verified against `rmw_zenoh_cpp` interop tests)
//!
//! The interop tests in `hiroz-tests` provide end-to-end correctness validation;
//! these tests guard against regressions in future refactors.

use std::{collections::HashMap, path::PathBuf};

use hiroz_codegen::{
    discovery::{discover_messages, discover_services},
    resolver::Resolver,
    types::{ResolvedMessage, ResolvedService},
};

/// Path to the bundled Jazzy message assets.
fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy")
}

/// Resolve all messages from the listed packages using the bundled Jazzy assets.
///
/// Returns a map from `pkg/msg/TypeName` to the resolved message (which carries
/// the computed `TypeHash`).
fn resolve_packages(packages: &[&str]) -> HashMap<String, ResolvedMessage> {
    let assets = assets_dir();

    let mut all_messages = Vec::new();
    for pkg in packages {
        let pkg_path = assets.join(pkg);
        let msgs = discover_messages(&pkg_path, pkg)
            .unwrap_or_else(|e| panic!("Failed to discover messages in {pkg}: {e}"));
        all_messages.extend(msgs);
    }

    let mut resolver = Resolver::new(false /* is_humble */);
    let resolved = resolver
        .resolve_messages(all_messages)
        .expect("Failed to resolve messages");

    resolved
        .into_iter()
        .map(|r| {
            let key = format!("{}/msg/{}", r.parsed.package, r.parsed.name);
            (key, r)
        })
        .collect()
}

/// All packages needed to resolve the geometry/sensor/nav message types used
/// in the tests below.
fn all_test_packages() -> &'static [&'static str] {
    &[
        "builtin_interfaces",
        "unique_identifier_msgs",
        "action_msgs",
        "std_msgs",
        "geometry_msgs",
        "sensor_msgs",
        "nav_msgs",
    ]
}

fn get_hash(resolved: &HashMap<String, ResolvedMessage>, type_name: &str) -> String {
    resolved
        .get(type_name)
        .unwrap_or_else(|| panic!("Type {type_name} not found in resolved messages"))
        .type_hash
        .to_rihs_string()
}

// --- Format and determinism tests ---

#[test]
fn test_hash_format_std_msgs_string() {
    // std_msgs/msg/Header depends on builtin_interfaces/msg/Time, so both
    // packages are needed for the resolver to successfully resolve all messages.
    let resolved = resolve_packages(&["builtin_interfaces", "std_msgs"]);
    let hash = get_hash(&resolved, "std_msgs/msg/String");
    assert!(
        hash.starts_with("RIHS01_"),
        "Hash should start with RIHS01_: {hash}"
    );
    assert_eq!(
        hash.len(),
        7 + 64,
        "Hash should be RIHS01_ + 64 hex chars: {hash}"
    );
}

#[test]
fn test_hash_deterministic_twist_stamped() {
    let resolved1 = resolve_packages(all_test_packages());
    let resolved2 = resolve_packages(all_test_packages());
    let hash1 = get_hash(&resolved1, "geometry_msgs/msg/TwistStamped");
    let hash2 = get_hash(&resolved2, "geometry_msgs/msg/TwistStamped");
    assert_eq!(hash1, hash2, "Hash must be deterministic");
}

#[test]
fn test_hashes_differ_by_type() {
    let resolved = resolve_packages(all_test_packages());
    let h_string = get_hash(&resolved, "std_msgs/msg/String");
    let h_header = get_hash(&resolved, "std_msgs/msg/Header");
    let h_twist_stamped = get_hash(&resolved, "geometry_msgs/msg/TwistStamped");
    let h_pose_stamped = get_hash(&resolved, "geometry_msgs/msg/PoseStamped");
    let h_imu = get_hash(&resolved, "sensor_msgs/msg/Imu");
    let h_odom = get_hash(&resolved, "nav_msgs/msg/Odometry");

    let all = [
        &h_string,
        &h_header,
        &h_twist_stamped,
        &h_pose_stamped,
        &h_imu,
        &h_odom,
    ];
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            if i != j {
                assert_ne!(
                    a, b,
                    "Hashes for different types must differ (indices {i}, {j})"
                );
            }
        }
    }
}

// --- Expected value tests ---
//
// These values were computed from the fixed implementation of `collect_referenced_types`
// (using `nested_type_name_to_key`) against the bundled Jazzy assets.
//
// End-to-end correctness (i.e. these match what rmw_zenoh_cpp computes) is
// validated by the interop tests in `hiroz-tests/tests/pubsub_interop.rs`.
// These tests guard against regressions in future refactors.

#[test]
fn test_expected_hash_std_msgs_string() {
    let resolved = resolve_packages(&["builtin_interfaces", "std_msgs"]);
    let hash = get_hash(&resolved, "std_msgs/msg/String");
    assert_eq!(
        hash, "RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18",
        "std_msgs/msg/String hash mismatch"
    );
}

#[test]
fn test_expected_hash_std_msgs_header() {
    let resolved = resolve_packages(&["builtin_interfaces", "std_msgs"]);
    let hash = get_hash(&resolved, "std_msgs/msg/Header");
    assert_eq!(
        hash, "RIHS01_f49fb3ae2cf070f793645ff749683ac6b06203e41c891e17701b1cb597ce6a01",
        "std_msgs/msg/Header hash mismatch"
    );
}

#[test]
fn test_expected_hash_twist_stamped() {
    let resolved = resolve_packages(all_test_packages());
    let hash = get_hash(&resolved, "geometry_msgs/msg/TwistStamped");
    assert_eq!(
        hash, "RIHS01_5f0fcd4f81d5d06ad9b4c4c63e3ea51b82d6ae4d0558f1d475229b1121db6f64",
        "geometry_msgs/msg/TwistStamped hash mismatch"
    );
}

#[test]
fn test_expected_hash_pose_stamped() {
    let resolved = resolve_packages(all_test_packages());
    let hash = get_hash(&resolved, "geometry_msgs/msg/PoseStamped");
    assert_eq!(
        hash, "RIHS01_10f3786d7d40fd2b54367835614bff85d4ad3b5dab62bf8bca0cc232d73b4cd8",
        "geometry_msgs/msg/PoseStamped hash mismatch"
    );
}

#[test]
fn test_expected_hash_imu() {
    let resolved = resolve_packages(all_test_packages());
    let hash = get_hash(&resolved, "sensor_msgs/msg/Imu");
    assert_eq!(
        hash, "RIHS01_7d9a00ff131080897a5ec7e26e315954b8eae3353c3f995c55faf71574000b5b",
        "sensor_msgs/msg/Imu hash mismatch"
    );
}

#[test]
fn test_expected_hash_odometry() {
    let resolved = resolve_packages(all_test_packages());
    let hash = get_hash(&resolved, "nav_msgs/msg/Odometry");
    assert_eq!(
        hash, "RIHS01_3cc97dc7fb7502f8714462c526d369e35b603cfc34d946e3f2eda2766dfec6e0",
        "nav_msgs/msg/Odometry hash mismatch"
    );
}

/// Resolve services from the listed packages using the bundled Jazzy assets.
fn resolve_services_from_packages(packages: &[&str]) -> HashMap<String, ResolvedService> {
    let assets = assets_dir();

    let mut all_messages = Vec::new();
    let mut all_services = Vec::new();
    for pkg in packages {
        let pkg_path = assets.join(pkg);
        let msgs = discover_messages(&pkg_path, pkg)
            .unwrap_or_else(|e| panic!("Failed to discover messages in {pkg}: {e}"));
        all_messages.extend(msgs);
        let srvs = discover_services(&pkg_path, pkg)
            .unwrap_or_else(|e| panic!("Failed to discover services in {pkg}: {e}"));
        all_services.extend(srvs);
    }

    let mut resolver = Resolver::new(false /* is_humble */);
    let _ = resolver
        .resolve_messages(all_messages)
        .expect("Failed to resolve messages");
    let resolved = resolver
        .resolve_services(all_services)
        .expect("Failed to resolve services");

    resolved
        .into_iter()
        .map(|r| {
            let key = format!("{}/srv/{}", r.parsed.package, r.parsed.name);
            (key, r)
        })
        .collect()
}

fn get_service_hash(resolved: &HashMap<String, ResolvedService>, type_name: &str) -> String {
    resolved
        .get(type_name)
        .unwrap_or_else(|| panic!("Type {type_name} not found in resolved services"))
        .type_hash
        .to_rihs_string()
}

// --- Helper to print current hash values (run with --ignored --nocapture) ---

#[test]
#[ignore = "utility: prints current hash values for capture and comparison with ROS 2"]
fn print_hashes() {
    let resolved = resolve_packages(all_test_packages());

    let types = [
        "std_msgs/msg/String",
        "std_msgs/msg/Header",
        "geometry_msgs/msg/Twist",
        "geometry_msgs/msg/TwistStamped",
        "geometry_msgs/msg/PoseStamped",
        "sensor_msgs/msg/Imu",
        "nav_msgs/msg/Odometry",
    ];

    println!("\n=== Computed RIHS01 hashes (jazzy assets, fixed nested lookup) ===");
    for t in types {
        let hash = get_hash(&resolved, t);
        println!("  {t}: {hash}");
    }
    println!();
}

/// All packages needed to resolve rcl_interfaces service types.
fn rcl_interfaces_packages() -> &'static [&'static str] {
    &["builtin_interfaces", "service_msgs", "rcl_interfaces"]
}

// --- rcl_interfaces service hash regression tests ---
//
// These values match the hardcoded hashes in `crates/hiroz/src/parameter/wire_types.rs`,
// which were verified against rmw_zenoh_cpp interop tests.
// They guard against regressions in byte-type mapping and action-suffix detection.

#[test]
fn test_expected_hash_describe_parameters() {
    let resolved = resolve_services_from_packages(rcl_interfaces_packages());
    let hash = get_service_hash(&resolved, "rcl_interfaces/srv/DescribeParameters");
    assert_eq!(
        hash, "RIHS01_845b484d71eb0673dae682f2e3ba3c4851a65a3dcfb97bddd82c5b57e91e4cff",
        "rcl_interfaces/srv/DescribeParameters hash mismatch"
    );
}

#[test]
fn test_expected_hash_get_parameters() {
    let resolved = resolve_services_from_packages(rcl_interfaces_packages());
    let hash = get_service_hash(&resolved, "rcl_interfaces/srv/GetParameters");
    assert_eq!(
        hash, "RIHS01_bf9803d5c74cf989a5de3e0c2e99444599a627c7ff75f97b8c05b01003675cbc",
        "rcl_interfaces/srv/GetParameters hash mismatch"
    );
}

#[test]
fn test_expected_hash_get_parameter_types() {
    let resolved = resolve_services_from_packages(rcl_interfaces_packages());
    let hash = get_service_hash(&resolved, "rcl_interfaces/srv/GetParameterTypes");
    assert_eq!(
        hash, "RIHS01_da199c878688b3e530bdfe3ca8f74cb9fa0c303101e980a9e8f260e25e1c80ca",
        "rcl_interfaces/srv/GetParameterTypes hash mismatch"
    );
}

#[test]
fn test_expected_hash_list_parameters() {
    let resolved = resolve_services_from_packages(rcl_interfaces_packages());
    let hash = get_service_hash(&resolved, "rcl_interfaces/srv/ListParameters");
    assert_eq!(
        hash, "RIHS01_3e6062bfbb27bfb8730d4cef2558221f51a11646d78e7bb30a1e83afac3aad9d",
        "rcl_interfaces/srv/ListParameters hash mismatch"
    );
}

#[test]
fn test_expected_hash_set_parameters() {
    let resolved = resolve_services_from_packages(rcl_interfaces_packages());
    let hash = get_service_hash(&resolved, "rcl_interfaces/srv/SetParameters");
    assert_eq!(
        hash, "RIHS01_56eed9a67e169f9cb6c1f987bc88f868c14a8fc9f743a263bc734c154015d7e0",
        "rcl_interfaces/srv/SetParameters hash mismatch"
    );
}

#[test]
fn test_expected_hash_set_parameters_atomically() {
    let resolved = resolve_services_from_packages(rcl_interfaces_packages());
    let hash = get_service_hash(&resolved, "rcl_interfaces/srv/SetParametersAtomically");
    assert_eq!(
        hash, "RIHS01_0e192ef259c07fc3c07a13191d27002222e65e00ccec653ca05e856f79285fcd",
        "rcl_interfaces/srv/SetParametersAtomically hash mismatch"
    );
}

// --- generate_user_messages regression (issue #168) ---
//
// `generate_user_messages` is the public API used by user crates from build.rs.
// Before issue #168 was fixed, it computed wrong hashes for user messages that
// referenced bundled types like `std_msgs/Header`, because the resolver had no
// access to those bundled type descriptions. Now `generate_user_messages` loads
// the bundled assets shipped with `hiroz-codegen` automatically, so user
// messages get the canonical RIHS01 hash without callers having to manually
// add bundled package paths to `HIROZ_MSG_PATH`.

mod issue_168 {
    use std::fs;

    use hiroz_codegen::generate_user_messages;
    use serial_test::serial;

    /// Canonical hash for `test_messages/msg/MsgWithHeader` defined as
    /// `std_msgs/Header header` + `float64 member_1`, computed by
    /// `rosidl_generator_type_description`.
    const EXPECTED_MSG_WITH_HEADER_HASH: &str =
        "RIHS01_0be811aef59cf675e4afdd9dca7f4e5d70fc4a8dee741e2e8ada6003e46a966c";

    fn set_env(k: &str, v: &str) {
        // SAFETY: tests are #[serial] to prevent data races on env vars.
        unsafe { std::env::set_var(k, v) };
    }

    fn remove_env(k: &str) {
        unsafe { std::env::remove_var(k) };
    }

    #[test]
    #[serial]
    fn generate_user_messages_msg_with_header_matches_canonical_hash() {
        let temp = tempfile::tempdir().unwrap();
        let pkg = temp.path().join("test_messages");
        let msg_dir = pkg.join("msg");
        fs::create_dir_all(&msg_dir).unwrap();
        fs::write(
            msg_dir.join("MsgWithHeader.msg"),
            "std_msgs/Header header\nfloat64 member_1\n",
        )
        .unwrap();

        let out = temp.path().join("out");
        fs::create_dir_all(&out).unwrap();

        // HIROZ_MSG_PATH points only at the user package — the bundled
        // `std_msgs` and `builtin_interfaces` paths must be picked up
        // automatically by `generate_user_messages`.
        set_env("HIROZ_MSG_PATH", pkg.to_str().unwrap());
        let result = generate_user_messages(&out, false);
        remove_env("HIROZ_MSG_PATH");
        assert!(result.is_ok(), "generate_user_messages failed: {result:?}");

        let generated = fs::read_to_string(out.join("generated.rs")).unwrap();
        assert!(
            generated.contains(EXPECTED_MSG_WITH_HEADER_HASH),
            "Generated code did not contain expected hash {EXPECTED_MSG_WITH_HEADER_HASH}.\n\
             Full generated code:\n{generated}"
        );

        // Bundled types must NOT be re-emitted as user code — they come from
        // the `hiroz_msgs` crate.
        assert!(
            !generated.contains("pub struct Header"),
            "Generated code unexpectedly contains bundled std_msgs::Header"
        );
        assert!(
            !generated.contains("pub mod std_msgs"),
            "Generated code unexpectedly contains bundled std_msgs module"
        );
        assert!(
            generated.contains("pub mod test_messages"),
            "Generated code missing user package module test_messages"
        );
    }
}

#[test]
#[ignore = "utility: prints current service hash values for comparison with wire_types.rs"]
fn print_service_hashes() {
    let packages = &["builtin_interfaces", "service_msgs", "rcl_interfaces"];
    let resolved = resolve_services_from_packages(packages);

    let services = [
        "rcl_interfaces/srv/GetParameters",
        "rcl_interfaces/srv/SetParameters",
        "rcl_interfaces/srv/ListParameters",
        "rcl_interfaces/srv/DescribeParameters",
        "rcl_interfaces/srv/GetParameterTypes",
        "rcl_interfaces/srv/SetParametersAtomically",
    ];

    println!("\n=== Computed RIHS01 service hashes (jazzy assets) ===");
    for t in services {
        if let Some(r) = resolved.get(t) {
            println!("  {t}: {}", r.type_hash.to_rihs_string());
        } else {
            println!("  {t}: NOT FOUND");
        }
    }
    println!();
}
