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

    embed_bundled_msgs();

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

/// Embed the bundled `.msg` definitions as source text, so a binary with no
/// `HIROZ_MSG_PATH` and no live publisher can still resolve a schema.
///
/// Emits `$OUT_DIR/embedded_msgs.rs`: a sorted `&[(&str, &str)]` of
/// `pkg/msg/Name` to the file's contents, via `include_str!` so the bytes are
/// the ones on disk at build time and cannot drift from them.
///
/// The whole jazzy set is ~122 KB of text. Subsetting it would only trade a
/// fraction of a percent of binary size for "why is my type missing?", so
/// everything bundled is embedded.
///
/// Reads `../hiroz-codegen/assets/{distro}` directly rather than calling
/// `hiroz_codegen::bundled_assets_dir`, which would make hiroz-codegen a
/// build-dependency for a directory walk. If the directory is absent the table
/// is empty and schema resolution behaves exactly as it did before.
fn embed_bundled_msgs() {
    use std::fmt::Write as _;

    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let distro = if std::env::var_os("CARGO_FEATURE_HUMBLE").is_some() {
        "humble"
    } else {
        "jazzy"
    };
    let assets = manifest
        .join("..")
        .join("hiroz-codegen")
        .join("assets")
        .join(distro);
    println!("cargo:rerun-if-changed={}", assets.display());

    let mut entries: Vec<(String, std::path::PathBuf)> = Vec::new();
    if let Ok(packages) = std::fs::read_dir(&assets) {
        for pkg in packages.flatten() {
            let pkg_path = pkg.path();
            if !pkg_path.is_dir() {
                continue;
            }
            let Some(pkg_name) = pkg_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let msg_dir = pkg_path.join("msg");
            let Ok(msgs) = std::fs::read_dir(&msg_dir) else {
                continue;
            };
            for m in msgs.flatten() {
                let path = m.path();
                if path.extension().and_then(|e| e.to_str()) != Some("msg") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|n| n.to_str()) else {
                    continue;
                };
                entries.push((format!("{pkg_name}/msg/{stem}"), path));
            }
        }
    } else {
        println!(
            "cargo:warning=bundled assets not found at {}; embedded schema fallback will be empty",
            assets.display()
        );
    }

    // Sorted, so the lookup can binary-search.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::from(
        "// @generated by build.rs - do not edit\npub static EMBEDDED_MSGS: &[(&str, &str)] = &[\n",
    );
    for (type_name, path) in &entries {
        writeln!(
            out,
            "    ({type_name:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");

    let dest = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_msgs.rs");
    if let Err(e) = std::fs::write(&dest, out) {
        println!("cargo:warning=failed to write {}: {e}", dest.display());
    }
}
