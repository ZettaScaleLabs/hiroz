use std::{env, path::PathBuf};

use anyhow::Result;
#[cfg(feature = "python_registry")]
use hiroz_codegen::python_msgspec_generator;

fn main() -> Result<()> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Declare custom cfg for ROS version detection
    println!("cargo:rustc-check-cfg=cfg(ros_humble)");

    // Select the target ROS distro from Cargo features (default: jazzy). The
    // distro selects both the bundled asset tree (hiroz-codegen/assets/<distro>/)
    // and the type-hash semantics (Humble predates ServiceEventInfo and uses
    // placeholder hashes).
    let distro = Distro::from_features();
    let is_humble = distro.is_humble();
    if is_humble {
        println!("cargo:rustc-cfg=ros_humble");
    }
    println!("cargo:warning=target ROS distro: {}", distro.assets_dir());
    if distro.bundled_dir() != distro.assets_dir() {
        println!(
            "cargo:warning=assets/{} has no packages yet — reading assets/{} instead",
            distro.assets_dir(),
            distro.bundled_dir()
        );
    }

    // Re-run if the selected distro's bundled asset tree changes.
    let codegen_assets = hiroz_codegen::bundled_assets_dir_for(distro.bundled_dir());
    if codegen_assets.exists() {
        println!("cargo:rerun-if-changed={}", codegen_assets.display());
    }

    // Discover ROS packages by enumerating the selected distro's asset tree.
    let ros_packages = discover_ros_packages(distro)?;

    // Expose the generated package set so tests can assert on it directly --
    // `cargo check` succeeding doesn't prove the right packages were
    // generated. See `tests/feature_scoping.rs`.
    let generated_package_names: Vec<String> = ros_packages
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    println!(
        "cargo:rustc-env=HIROZ_MSGS_GENERATED_PACKAGES={}",
        generated_package_names.join(",")
    );

    println!(
        "cargo:warning=protobuf feature: {}",
        cfg!(feature = "protobuf")
    );
    println!("cargo:warning=ros_packages len: {}", ros_packages.len());

    if !ros_packages.is_empty() {
        println!("cargo:warning=generating messages");
        let config = hiroz_codegen::GeneratorConfig {
            generate_cdr: true, // Always generate for ROS2 compatibility
            generate_protobuf: cfg!(feature = "protobuf"),
            generate_type_info: true,
            is_humble,
            output_dir: out_dir.clone(),
            external_crate: None, // All packages are local in hiroz-msgs
            local_packages: std::collections::HashSet::new(), // All packages are local
            json_out: None,       // Not needed for Rust codegen
        };

        let generator = hiroz_codegen::MessageGenerator::new(config);

        let package_refs: Vec<&std::path::Path> =
            ros_packages.iter().map(|p| p.as_path()).collect();
        generator.generate_from_msg_files(&package_refs)?;
        println!("cargo:warning=generated messages");

        println!(
            "cargo:info=Generated ROS messages from {} packages",
            ros_packages.len()
        );

        // Generate Python bindings if python_registry feature is enabled
        #[cfg(feature = "python_registry")]
        {
            // Use hiroz_codegen's discovery and resolver to get resolved messages
            let (messages, services, _actions) =
                hiroz_codegen::discovery::discover_all(&package_refs)?;

            // Filter out problematic messages
            let messages: Vec<_> = messages
                .into_iter()
                .filter(|msg| {
                    let full_name = format!("{}/{}", msg.package, msg.name);

                    // Filter out actionlib_msgs and old-style Action messages
                    if full_name.starts_with("actionlib_msgs/")
                        || full_name.ends_with("Action")
                        || full_name.ends_with("ActionGoal")
                        || full_name.ends_with("ActionResult")
                        || full_name.ends_with("ActionFeedback")
                    {
                        return false;
                    }

                    // Filter out redundant service Request/Response message files
                    if msg.name.ends_with("_Request") || msg.name.ends_with("_Response") {
                        return false;
                    }

                    // Filter out messages with wstring fields
                    let has_wstring = msg
                        .fields
                        .iter()
                        .any(|field| field.field_type.base_type.contains("wstring"));

                    !has_wstring
                })
                .collect();

            let services: Vec<_> = services
                .into_iter()
                .filter(|srv| {
                    let full_name = format!("{}/{}", srv.package, srv.name);
                    !full_name.starts_with("actionlib_msgs/")
                })
                .collect();

            // Resolve dependencies using hiroz_codegen resolver
            let mut resolver = hiroz_codegen::resolver::Resolver::new(is_humble);
            let resolved_msgs = resolver.resolve_messages(messages)?;
            let resolved_srvs = resolver.resolve_services(services)?;

            // Create Python output directory
            let python_output_dir = PathBuf::from("python/hiroz_msgs_py/types");
            std::fs::create_dir_all(&python_output_dir)?;

            // Generate Python bindings + complete PyO3 module
            python_msgspec_generator::generate_python_bindings(
                &resolved_msgs,
                &resolved_srvs,
                &python_output_dir,
                &out_dir.join("python_bindings.rs"),
            )?;

            println!(
                "cargo:info=Generated Python bindings for {} messages",
                resolved_msgs.len()
            );
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AMENT_PREFIX_PATH");
    println!("cargo:rerun-if-env-changed=CMAKE_PREFIX_PATH");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_PROTOBUF");

    // Ensure generated_proto.rs exists even if protobuf generation is skipped
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let proto_file = out_dir.join("generated_proto.rs");
    if !proto_file.exists() {
        std::fs::write(&proto_file, "// Empty protobuf generated file\n").unwrap();
    }

    Ok(())
}

/// The target ROS 2 distro, selected by Cargo feature. Determines both which
/// bundled asset tree (hiroz-codegen/assets/<distro>/) is generated and the
/// type-hash semantics (Humble predates ServiceEventInfo and uses placeholder
/// hashes).
#[derive(Clone, Copy, Debug)]
enum Distro {
    Humble,
    Jazzy,
    Lyrical,
}

impl Distro {
    /// Pick the distro from Cargo features. Defaults to Jazzy when no explicit
    /// distro feature is set. If several are enabled the newest wins.
    fn from_features() -> Self {
        if env::var("CARGO_FEATURE_LYRICAL").is_ok() {
            Distro::Lyrical
        } else if env::var("CARGO_FEATURE_JAZZY").is_ok() {
            Distro::Jazzy
        } else if env::var("CARGO_FEATURE_HUMBLE").is_ok() {
            Distro::Humble
        } else {
            Distro::Jazzy
        }
    }

    fn assets_dir(self) -> &'static str {
        match self {
            Distro::Humble => "humble",
            Distro::Jazzy => "jazzy",
            Distro::Lyrical => "lyrical",
        }
    }

    /// `assets/humble/` has no real packages yet, so Humble falls back to
    /// jazzy (a safe superset), same as before #344.
    fn bundled_dir(self) -> &'static str {
        match self {
            Distro::Humble => "jazzy",
            Distro::Jazzy => "jazzy",
            Distro::Lyrical => "lyrical",
        }
    }

    fn is_humble(self) -> bool {
        matches!(self, Distro::Humble)
    }
}

/// Packages requested by enabled Cargo features. Always includes the four
/// base packages, plus `service_msgs`/`type_description_interfaces` for
/// non-Humble distros.
fn selected_package_names(is_humble: bool) -> Vec<&'static str> {
    let mut names = vec![
        "builtin_interfaces",
        "action_msgs",
        "unique_identifier_msgs",
        "lifecycle_msgs",
    ];

    if !is_humble {
        names.push("service_msgs");
        names.push("type_description_interfaces");
    }

    const FEATURE_PACKAGES: &[&str] = &[
        "std_msgs",
        "geometry_msgs",
        "sensor_msgs",
        "nav_msgs",
        "example_interfaces",
        "action_tutorials_interfaces",
        "test_msgs",
        "rcl_interfaces",
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
    for &pkg in FEATURE_PACKAGES {
        if env::var(format!("CARGO_FEATURE_{}", pkg.to_uppercase())).is_ok() {
            names.push(pkg);
        }
    }

    names
}

/// Resolves `selected_package_names` against the asset tree. A requested
/// package missing from the tree is a hard error, not a silent omission.
fn discover_ros_packages(distro: Distro) -> Result<Vec<PathBuf>> {
    let assets_dir = hiroz_codegen::bundled_assets_dir_for(distro.bundled_dir());

    if !assets_dir.exists() {
        anyhow::bail!(
            "bundled assets directory not found for distro {} (reading assets/{}): {:?}",
            distro.assets_dir(),
            distro.bundled_dir(),
            assets_dir
        );
    }

    // Re-run the build script if the asset tree changes.
    println!("cargo:rerun-if-changed={}", assets_dir.display());

    let wanted = selected_package_names(distro.is_humble());
    let mut packages = Vec::with_capacity(wanted.len());
    for name in wanted {
        let path = assets_dir.join(name);
        let has_interfaces =
            path.join("msg").exists() || path.join("srv").exists() || path.join("action").exists();
        if !has_interfaces {
            anyhow::bail!(
                "feature requested package `{name}` but it was not found in assets/{} \
                 (distro {}): {path:?}",
                distro.bundled_dir(),
                distro.assets_dir(),
            );
        }
        packages.push(path);
    }

    if packages.is_empty() {
        anyhow::bail!(
            "no packages selected for distro {} — enable at least one hiroz-msgs package feature",
            distro.assets_dir()
        );
    }
    println!(
        "cargo:warning=generating {} packages from assets/{} (distro {})",
        packages.len(),
        distro.bundled_dir(),
        distro.assets_dir()
    );

    Ok(packages)
}
