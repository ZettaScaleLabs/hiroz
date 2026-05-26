use std::{env, path::PathBuf};

use anyhow::Result;
#[cfg(feature = "python_registry")]
use hiroz_codegen::python_msgspec_generator;

fn main() -> Result<()> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Declare custom cfg for ROS version detection
    println!("cargo:rustc-check-cfg=cfg(ros_humble)");

    // Declare custom cfg flags for package availability
    println!("cargo::rustc-check-cfg=cfg(has_example_interfaces)");
    println!("cargo::rustc-check-cfg=cfg(has_test_msgs)");
    println!("cargo::rustc-check-cfg=cfg(has_rcl_interfaces)");

    // Detect ROS version and emit cfg
    let is_humble = detect_ros_version();

    // Discover ROS packages
    let ros_packages = discover_ros_packages(is_humble)?;

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

fn discover_ros_packages(is_humble: bool) -> Result<Vec<PathBuf>> {
    use std::collections::HashMap;

    // Use HashMap to track packages by name and deduplicate
    let mut package_map: HashMap<String, PathBuf> = HashMap::new();

    let all_packages = get_all_packages(is_humble);

    // Priority 1: Local bundled assets (highest priority - canonical source)
    // This ensures our bundled message definitions are always used consistently,
    // avoiding issues with system packages that may have different versions or
    // hardcoded paths from Nix wrapProgram.
    println!("cargo:info=Checking local bundled assets from hiroz-codegen/assets/jazzy");
    let local_asset_packages = discover_local_assets(&all_packages)?;
    let local_count = local_asset_packages.len();
    for pkg_path in local_asset_packages {
        if let Ok(name) = discover_package_name_from_path(&pkg_path) {
            println!("cargo:info=Local: Adding package {}", name);
            package_map.insert(name, pkg_path);
        }
    }

    if local_count > 0 {
        println!(
            "cargo:info=Found {} packages from local bundled assets",
            local_count
        );

        // Emit cfg flags for each found package
        for package_name in package_map.keys() {
            println!("cargo:rustc-cfg=has_{}", package_name);
        }

        return Ok(package_map.into_values().collect());
    }

    // Priority 2: System ROS installation (fallback for packages not bundled locally)
    let system_packages = discover_system_packages(&all_packages)?;
    let mut system_added = 0;
    for pkg_path in system_packages {
        if let Ok(name) = discover_package_name_from_path(&pkg_path) {
            // Only add if not already found in local assets
            if let std::collections::hash_map::Entry::Vacant(e) = package_map.entry(name) {
                println!("cargo:info=System: Adding package {}", e.key());
                e.insert(pkg_path);
                system_added += 1;
            }
        }
    }

    if system_added > 0 {
        println!(
            "cargo:info=Added {} packages from ROS 2 installation (not in local assets)",
            system_added
        );
    }

    println!(
        "cargo:info=Total unique packages discovered: {}",
        package_map.len()
    );

    // Warn if packages are still not found
    let still_missing: Vec<_> = all_packages
        .iter()
        .filter(|&&pkg| !package_map.contains_key(pkg))
        .collect();

    if !still_missing.is_empty() {
        println!("cargo:warning=Missing packages: {:?}", still_missing);
        println!("cargo:warning=Consider installing ROS 2 or checking hiroz-codegen/assets/jazzy");
    }

    Ok(package_map.into_values().collect())
}

/// Extract package name from path using package.xml or directory name
fn discover_package_name_from_path(package_path: &std::path::Path) -> Result<String> {
    hiroz_codegen::discovery::discover_package_name(package_path)
}

/// Get list of all package names based on enabled features
/// All packages are now bundled in hiroz-codegen/assets/jazzy
fn get_all_packages(is_humble: bool) -> Vec<&'static str> {
    let mut names = vec![
        "builtin_interfaces",     // Always required
        "action_msgs",            // Required for ROS 2 actions
        "unique_identifier_msgs", // Required by action_msgs
        "lifecycle_msgs",         // Required for lifecycle nodes
    ];

    // service_msgs was introduced in ROS 2 Iron (May 2023) as part of the service
    // introspection feature. It contains types like ServiceEventInfo for monitoring
    // service calls. This package doesn't exist in Humble (May 2022).
    if !is_humble {
        names.push("service_msgs");
        // type_description_interfaces was also introduced in Jazzy/Iron
        // for runtime type introspection support
        names.push("type_description_interfaces");
    }

    // Check features via environment variables (cfg! doesn't work in build scripts)
    if env::var("CARGO_FEATURE_STD_MSGS").is_ok() {
        names.push("std_msgs");
    }

    if env::var("CARGO_FEATURE_GEOMETRY_MSGS").is_ok() {
        names.push("geometry_msgs");
    }

    if env::var("CARGO_FEATURE_SENSOR_MSGS").is_ok() {
        names.push("sensor_msgs");
    }

    if env::var("CARGO_FEATURE_NAV_MSGS").is_ok() {
        names.push("nav_msgs");
    }

    if env::var("CARGO_FEATURE_EXAMPLE_INTERFACES").is_ok() {
        names.push("example_interfaces");
    }

    if env::var("CARGO_FEATURE_ACTION_TUTORIALS_INTERFACES").is_ok() {
        names.push("action_tutorials_interfaces");
    }

    if env::var("CARGO_FEATURE_TEST_MSGS").is_ok() {
        names.push("test_msgs");
    }

    if env::var("CARGO_FEATURE_RCL_INTERFACES").is_ok() {
        names.push("rcl_interfaces");
    }

    names
}

/// Try to discover packages from system ROS 2 installation
fn discover_system_packages(packages: &[&str]) -> Result<Vec<PathBuf>> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    let mut found_packages = Vec::new();

    // 1. Check AMENT_PREFIX_PATH (standard ROS 2 environment variable)
    if let Ok(ament_prefix_path) = env::var("AMENT_PREFIX_PATH") {
        for prefix in ament_prefix_path.split(':') {
            let prefix_path = PathBuf::from(prefix);
            for package_name in packages {
                let package_path = prefix_path.join("share").join(package_name);
                if package_path.exists()
                    && (package_path.join("msg").exists()
                        || package_path.join("srv").exists()
                        || package_path.join("action").exists())
                {
                    found_packages.push(package_path);
                }
            }
        }
    }

    // 2. Check CMAKE_PREFIX_PATH (also commonly set in ROS 2)
    if found_packages.is_empty()
        && let Ok(cmake_prefix_path) = env::var("CMAKE_PREFIX_PATH")
    {
        for prefix in cmake_prefix_path.split(':') {
            let prefix_path = PathBuf::from(prefix);
            for package_name in packages {
                let package_path = prefix_path.join("share").join(package_name);
                if package_path.exists()
                    && (package_path.join("msg").exists()
                        || package_path.join("srv").exists()
                        || package_path.join("action").exists())
                {
                    found_packages.push(package_path);
                }
            }
        }
    }

    // 3. Check common ROS 2 installation paths
    if found_packages.is_empty() {
        let common_install_paths = vec![
            "/opt/ros/rolling",
            "/opt/ros/jazzy",
            "/opt/ros/kilted",
            "/opt/ros/lyrical",
            "/opt/ros/humble",
        ];

        for install_path in common_install_paths {
            let install = PathBuf::from(install_path);
            if install.exists() {
                for package_name in packages {
                    let package_path = install.join("share").join(package_name);
                    if package_path.exists()
                        && (package_path.join("msg").exists()
                            || package_path.join("srv").exists()
                            || package_path.join("action").exists())
                    {
                        found_packages.push(package_path);
                    }
                }
                if !found_packages.is_empty() {
                    break;
                }
            }
        }
    }

    Ok(found_packages)
}

/// Discover packages from local bundled assets (hiroz-codegen/assets/jazzy/)
fn discover_local_assets(package_names: &[&str]) -> Result<Vec<PathBuf>> {
    let mut found_packages = Vec::new();

    // Get the path to hiroz-codegen/assets/jazzy relative to this crate
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let assets_dir = manifest_dir
        .parent()
        .expect("Failed to get parent directory")
        .join("hiroz-codegen/assets/jazzy");

    if !assets_dir.exists() {
        println!(
            "cargo:warning=Local assets directory not found: {:?}",
            assets_dir
        );
        return Ok(Vec::new());
    }

    // Search for packages in jazzy assets directory
    for package_name in package_names {
        let package_path = assets_dir.join(package_name);

        if package_path.exists()
            && (package_path.join("msg").exists()
                || package_path.join("srv").exists()
                || package_path.join("action").exists())
        {
            println!(
                "cargo:info=Found {} in local assets: {:?}",
                package_name, package_path
            );
            found_packages.push(package_path);
        }
    }

    Ok(found_packages)
}

/// Detect ROS version and emit cfg(ros_humble) if Humble is detected
/// Returns true if Humble is detected
fn detect_ros_version() -> bool {
    // Check feature flag first (explicitly requested Humble)
    if cfg!(feature = "humble") {
        println!("cargo:rustc-cfg=ros_humble");
        println!("cargo:warning=ROS Humble detected - using Humble-compatible codegen");
        return true;
    }

    // Check if ROS is installed by looking for AMENT_PREFIX_PATH
    if let Ok(ament_prefix) = env::var("AMENT_PREFIX_PATH") {
        // Jazzy and newer have type_description_interfaces, Humble doesn't
        let has_type_description = ament_prefix.split(':').any(|prefix| {
            PathBuf::from(prefix)
                .join("include/type_description_interfaces")
                .exists()
        });

        if !has_type_description {
            // No type_description_interfaces means Humble
            println!("cargo:rustc-cfg=ros_humble");
            println!("cargo:warning=ROS Humble detected - using Humble-compatible codegen");
            return true;
        } else {
            println!("cargo:warning=ROS Jazzy+ detected - using modern codegen");
            return false;
        }
    }

    // Default to Jazzy (modern)
    println!("cargo:warning=ROS Jazzy+ detected - using modern codegen");
    false
}
