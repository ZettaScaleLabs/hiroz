use std::{env, path::PathBuf};

fn main() {
    // Declare custom cfg for ROS version detection
    println!("cargo::rustc-check-cfg=cfg(ros_humble)");

    // Detect and set ROS version
    detect_ros_version();

    // Declare custom cfg flags for package availability
    // These are set by hiroz-msgs build.rs when packages are actually found
    println!("cargo::rustc-check-cfg=cfg(has_example_interfaces)");
    println!("cargo::rustc-check-cfg=cfg(has_test_msgs)");
}

/// Detect ROS version and emit cfg(ros_humble) if Humble is detected
fn detect_ros_version() {
    // Check feature flag first (explicitly requested Humble)
    if cfg!(feature = "humble") {
        println!("cargo:rustc-cfg=ros_humble");
        println!("cargo:warning=ROS Humble detected - skipping type_description tests");
        return;
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
            println!("cargo:warning=ROS Humble detected - skipping type_description tests");
        }
    }
}
