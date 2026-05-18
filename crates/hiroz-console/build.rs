fn main() {
    // Declare custom cfg for ROS version detection
    println!("cargo::rustc-check-cfg=cfg(ros_humble)");

    // Set ros_humble cfg when humble feature is enabled
    if cfg!(feature = "humble") {
        println!("cargo:rustc-cfg=ros_humble");
        println!("cargo:warning=ROS Humble detected - skipping type_description tests");
    }
}
