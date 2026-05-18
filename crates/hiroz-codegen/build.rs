use std::{env, path::PathBuf};

fn main() {
    // Declare custom cfg for ROS version detection
    println!("cargo:rustc-check-cfg=cfg(ros_humble)");

    // Detect ROS version and emit cfg
    detect_ros_version();

    println!("cargo:rerun-if-env-changed=AMENT_PREFIX_PATH");
    println!("cargo:rerun-if-env-changed=CMAKE_PREFIX_PATH");
}

/// Detect ROS version and emit cfg(ros_humble) if Humble is detected
fn detect_ros_version() {
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
        } else {
            println!("cargo:warning=ROS Jazzy+ detected - using modern codegen");
        }
    } else if env::var("CARGO_FEATURE_HUMBLE_COMPAT").is_ok() {
        // Pure Rust mode: user explicitly requested Humble compatibility
        println!("cargo:rustc-cfg=ros_humble");
        println!("cargo:warning=humble_compat feature enabled - using Humble-compatible codegen");
    }
}
