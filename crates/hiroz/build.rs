#[cfg(any(feature = "generate-configs", feature = "ffi"))]
use std::path::PathBuf;

// Include config module from src/ to access ConfigOverride pattern
// Allow dead_code since build.rs only uses a subset of config.rs APIs
#[allow(dead_code)]
#[path = "src/config.rs"]
mod config;

fn main() {
    // Declare custom cfg flags for package availability
    // These are set by hiroz-msgs build.rs when packages are actually found
    println!("cargo::rustc-check-cfg=cfg(has_example_interfaces)");
    println!("cargo::rustc-check-cfg=cfg(has_test_msgs)");

    println!("cargo:rerun-if-changed=src/config.rs");
    println!("cargo:rerun-if-env-changed=HIROZ_CONFIG_OUTPUT_DIR");

    // Generate C FFI header when ffi feature is enabled
    #[cfg(feature = "ffi")]
    {
        println!("cargo:rerun-if-changed=src/ffi/");
        println!("cargo:rerun-if-changed=cbindgen.toml");

        let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let header_path = crate_dir
            .join("..")
            .join("hiroz-go")
            .join("hiroz")
            .join("hiroz_ffi.h");

        // Invoke cbindgen CLI rather than the library API, because the library
        // API produces incomplete output for crates with complex dependencies.
        let output = std::process::Command::new("cbindgen")
            .arg("-c")
            .arg(crate_dir.join("cbindgen.toml"))
            .arg(&crate_dir)
            .output();

        match output {
            Ok(result) if result.status.success() => {
                std::fs::write(&header_path, &result.stdout).expect("Failed to write FFI header");
                println!(
                    "cargo:warning=Generated FFI header: {}",
                    header_path.display()
                );
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                println!(
                    "cargo:warning=cbindgen failed (exit {}): {}",
                    result.status, stderr
                );
            }
            Err(e) => {
                println!(
                    "cargo:warning=cbindgen not found ({}), skipping header generation",
                    e
                );
            }
        }
    }

    // Only generate configs if the feature is enabled
    #[cfg(feature = "generate-configs")]
    {
        // Determine output directory:
        // 1. Use HIROZ_CONFIG_OUTPUT_DIR if set (absolute or relative to CARGO_MANIFEST_DIR)
        // 2. Otherwise use OUT_DIR/hiroz_config
        let config_dir = if let Ok(custom_dir) = std::env::var("HIROZ_CONFIG_OUTPUT_DIR") {
            let path = PathBuf::from(&custom_dir);
            if path.is_absolute() {
                path
            } else {
                // Resolve relative paths from CARGO_MANIFEST_DIR (package root)
                let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
                manifest_dir.join(path)
            }
        } else {
            let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
            out_dir.join("hiroz_config")
        };

        // Create config directory
        if let Err(e) = std::fs::create_dir_all(&config_dir) {
            eprintln!("Warning: Failed to create config directory: {}", e);
            return;
        }

        // Generate router config JSON5 using ConfigOverride pattern
        let router_json5 = config::generate_json5(&config::router_overrides(), "Router Config");
        if let Err(e) = std::fs::write(
            config_dir.join("DEFAULT_HIROZ_ROUTER_CONFIG.json5"),
            router_json5,
        ) {
            eprintln!("Warning: Failed to write router config: {}", e);
        }

        // Generate session config JSON5 using ConfigOverride pattern
        let session_json5 = config::generate_json5(&config::session_overrides(), "Session Config");
        if let Err(e) = std::fs::write(
            config_dir.join("DEFAULT_HIROZ_SESSION_CONFIG.json5"),
            session_json5,
        ) {
            eprintln!("Warning: Failed to write session config: {}", e);
        }

        println!(
            "cargo:warning=Generated ROS configs: {}",
            config_dir.display()
        );
    }

    #[cfg(not(feature = "generate-configs"))]
    {
        println!(
            "cargo:warning=Config generation disabled. Enable with --features generate-configs"
        );
    }
}
