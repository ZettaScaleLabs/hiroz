use std::{env, path::PathBuf};

fn main() {
    // Package-wide enforcement of the feature requirement.
    //
    // `tests/feature_gate.rs` only fires if Cargo selects that target.
    // `cargo test -p hiroz-tests --test cache` with no features builds only
    // `cache`, which the crate-level `cfg` compiles to an empty binary --
    // `0 passed`, guard never run. A build script runs for every build of the
    // package regardless of target selection, so this is the one place the
    // requirement holds everywhere.
    if std::env::var_os("CARGO_FEATURE_ROS_MSGS").is_none() {
        panic!(
            "\n\nhiroz-tests requires the `ros-msgs` feature.\n\n\
             Without it the suites gated on it compile to empty test binaries \n\
             that report `0 passed`, which reads as green but is no coverage.\n\n\
             Build it as:\n\n    \
             cargo test -p hiroz-tests --features ros-msgs,jazzy\n\n\
             Suites that drive a real ROS 2 installation need \n    \
             --features ros-interop,<distro> instead.\n"
        );
    }

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
