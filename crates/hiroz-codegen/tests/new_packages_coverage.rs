//! Codegen coverage tests for the 11 new Jazzy packages added in PR #195.
//!
//! Verifies that every new package parses cleanly and resolves without error in
//! both Humble mode (is_humble=true, no service_msgs/type_description_interfaces)
//! and Jazzy mode (is_humble=false). Also spot-checks the action parser fix for
//! LookupTransform.action, which has an empty Feedback section.

use std::path::PathBuf;

use hiroz_codegen::{
    discovery::{discover_actions, discover_all, discover_messages, discover_services},
    resolver::Resolver,
};

fn jazzy_assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy")
}

fn pkg(name: &str) -> PathBuf {
    jazzy_assets().join(name)
}

/// The 11 new packages, plus their known dependencies already bundled in assets/jazzy.
const NEW_PACKAGES: &[&str] = &[
    "tf2_msgs",
    "visualization_msgs",
    "rosgraph_msgs",
    "trajectory_msgs",
    "diagnostic_msgs",
    "shape_msgs",
    "stereo_msgs",
    "statistics_msgs",
    "composition_interfaces",
    "std_srvs",
    "rosbag2_interfaces",
];

/// Dependencies of the new packages that must be present in the resolver context.
/// service_msgs and type_description_interfaces are needed by the Jazzy resolver
/// (is_humble=false) for service hash computation.
const DEP_PACKAGES: &[&str] = &[
    "builtin_interfaces",
    "std_msgs",
    "geometry_msgs",
    "sensor_msgs",
    "rcl_interfaces",
    "service_msgs",
    "type_description_interfaces",
];

// ============================================================================
// Parse-only: no resolver, no distro flag — just confirm the IDL files are valid
// ============================================================================

#[test]
fn all_new_packages_parse_without_error() {
    for &pkg_name in NEW_PACKAGES {
        let path = pkg(pkg_name);
        discover_messages(&path, pkg_name)
            .unwrap_or_else(|e| panic!("{pkg_name}: message parse failed: {e}"));
        discover_services(&path, pkg_name)
            .unwrap_or_else(|e| panic!("{pkg_name}: service parse failed: {e}"));
        discover_actions(&path, pkg_name)
            .unwrap_or_else(|e| panic!("{pkg_name}: action parse failed: {e}"));
    }
}

/// LookupTransform.action has an empty Feedback section — the key case fixed by the
/// action parser change in this PR. Verify the parsed action has a zero-field Feedback.
#[test]
fn lookup_transform_empty_feedback_parses_as_zero_field_message() {
    let actions =
        discover_actions(&pkg("tf2_msgs"), "tf2_msgs").expect("tf2_msgs action discovery failed");
    let action = actions
        .iter()
        .find(|a| a.name == "LookupTransform")
        .expect("LookupTransform action not found in tf2_msgs");

    let feedback = action
        .feedback
        .as_ref()
        .expect("LookupTransform Feedback must be Some (not None) after parser fix");
    assert!(
        feedback.fields.is_empty(),
        "LookupTransform Feedback must be a zero-field message, got: {:?}",
        feedback.fields
    );

    // Result is non-empty (has the transform field)
    let result = action
        .result
        .as_ref()
        .expect("LookupTransform Result must be Some");
    assert!(
        !result.fields.is_empty(),
        "LookupTransform Result must have fields"
    );
}

// ============================================================================
// Resolve: check both Humble and Jazzy resolver modes
// ============================================================================

fn collect_all_paths() -> Vec<PathBuf> {
    DEP_PACKAGES
        .iter()
        .chain(NEW_PACKAGES.iter())
        .map(|&n| pkg(n))
        .collect()
}

fn resolve_new_packages(is_humble: bool) {
    let owned = collect_all_paths();
    let paths: Vec<&std::path::Path> = owned.iter().map(|p| p.as_path()).collect();

    let (msgs, srvs, actions) = discover_all(&paths).expect("discover_all failed for new packages");

    let mut resolver = Resolver::new(is_humble);
    resolver
        .resolve_messages(msgs)
        .unwrap_or_else(|e| panic!("resolve_messages failed (is_humble={is_humble}): {e}"));
    resolver
        .resolve_services(srvs)
        .unwrap_or_else(|e| panic!("resolve_services failed (is_humble={is_humble}): {e}"));
    resolver
        .resolve_actions(actions)
        .unwrap_or_else(|e| panic!("resolve_actions failed (is_humble={is_humble}): {e}"));
}

#[test]
fn new_packages_resolve_jazzy_mode() {
    resolve_new_packages(false);
}

#[test]
fn new_packages_resolve_humble_mode() {
    resolve_new_packages(true);
}

// ============================================================================
// Type hash sanity: every resolved type produces a well-formed RIHS01_ hash
// ============================================================================

#[test]
fn all_new_package_types_have_valid_hashes() {
    let owned = collect_all_paths();
    let path_refs: Vec<&std::path::Path> = owned.iter().map(|p| p.as_path()).collect();

    let (msgs, srvs, actions) = discover_all(&path_refs).expect("discover_all failed");

    // Only check types from the new packages — filter out dep packages
    let new_pkg_set: std::collections::HashSet<&str> = NEW_PACKAGES.iter().copied().collect();

    let mut resolver = Resolver::new(false);
    let resolved_msgs = resolver.resolve_messages(msgs).expect("resolve_messages");
    let resolved_srvs = resolver.resolve_services(srvs).expect("resolve_services");
    let resolved_actions = resolver.resolve_actions(actions).expect("resolve_actions");

    for msg in resolved_msgs
        .iter()
        .filter(|m| new_pkg_set.contains(m.parsed.package.as_str()))
    {
        let hash = msg.type_hash.to_rihs_string();
        assert!(
            hash.starts_with("RIHS01_") && hash.len() == 71,
            "{}/{}: malformed hash: {hash}",
            msg.parsed.package,
            msg.parsed.name
        );
    }
    for srv in resolved_srvs
        .iter()
        .filter(|s| new_pkg_set.contains(s.parsed.package.as_str()))
    {
        let hash = srv.type_hash.to_rihs_string();
        assert!(
            hash.starts_with("RIHS01_") && hash.len() == 71,
            "{}/{}: malformed srv hash: {hash}",
            srv.parsed.package,
            srv.parsed.name
        );
    }
    for action in resolved_actions
        .iter()
        .filter(|a| new_pkg_set.contains(a.parsed.package.as_str()))
    {
        for (label, h) in [
            ("goal", action.goal.type_hash.to_rihs_string()),
            (
                "result",
                action
                    .result
                    .as_ref()
                    .map(|r| r.type_hash.to_rihs_string())
                    .unwrap_or_default(),
            ),
            (
                "feedback",
                action
                    .feedback
                    .as_ref()
                    .map(|f| f.type_hash.to_rihs_string())
                    .unwrap_or_default(),
            ),
        ] {
            if h.is_empty() {
                continue;
            }
            assert!(
                h.starts_with("RIHS01_") && h.len() == 71,
                "{}/{} {label}: malformed action hash: {h}",
                action.parsed.package,
                action.parsed.name
            );
        }
    }
}
