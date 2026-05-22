//! CLI tool to export codegen JSON manifest from message packages.
//!
//! Usage:
//!   cargo run -p hiroz-codegen --bin export_json -- \
//!     --assets crates/hiroz-codegen/assets/jazzy \
//!     --output _tmp/manifest.json

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let mut assets_dir = None;
    let mut output_path = None;
    let mut packages: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--assets" => {
                i += 1;
                assets_dir = Some(PathBuf::from(&args[i]));
            }
            "--output" | "-o" => {
                i += 1;
                output_path = Some(PathBuf::from(&args[i]));
            }
            "--packages" | "-p" => {
                i += 1;
                packages = args[i].split(',').map(|s| s.to_string()).collect();
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: export_json --assets <dir> --output <file> [--packages pkg1,pkg2]"
                );
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --assets <dir>       Assets directory with ROS message packages");
                eprintln!("  --output <file>      Output JSON manifest path (or - for stdout)");
                eprintln!("  --packages <list>    Comma-separated list of packages to include");
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let assets_dir = assets_dir.context("--assets is required")?;
    let output_path = output_path.context("--output is required")?;

    // Discover package directories
    let mut package_dirs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&assets_dir)
        .with_context(|| format!("Failed to read assets directory: {:?}", assets_dir))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap_or("").to_string();

        // Filter by package list if specified
        if !packages.is_empty() && !packages.contains(&name) {
            continue;
        }

        // Must have msg/, srv/, or action/ subdirectory
        if path.join("msg").exists() || path.join("srv").exists() || path.join("action").exists() {
            package_dirs.push(path);
        }
    }

    if package_dirs.is_empty() {
        anyhow::bail!("No message packages found in {:?}", assets_dir);
    }

    package_dirs.sort();
    eprintln!(
        "Discovered {} packages: {:?}",
        package_dirs.len(),
        package_dirs
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>()
    );

    // Use MessageGenerator with json_out
    let use_stdout = output_path == Path::new("-");

    // For stdout mode, use a temp file in _tmp/
    let json_path = if use_stdout {
        std::env::temp_dir().join("hiroz_codegen_manifest.json")
    } else {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        output_path.clone()
    };

    // output_dir is required but unused when generate_cdr=false
    let dummy_out = std::env::temp_dir().join("hiroz_codegen_dummy");
    std::fs::create_dir_all(&dummy_out)?;

    let config = hiroz_codegen::GeneratorConfig {
        generate_cdr: false,
        generate_protobuf: false,
        generate_type_info: false,
        is_humble: false,
        output_dir: dummy_out,
        external_crate: None,
        local_packages: std::collections::HashSet::new(),
        json_out: Some(json_path.clone()),
    };

    let generator = hiroz_codegen::MessageGenerator::new(config);
    let package_refs: Vec<&Path> = package_dirs.iter().map(|p| p.as_path()).collect();
    generator
        .generate_from_msg_files(&package_refs)
        .context("Code generation failed")?;

    if use_stdout {
        let content = std::fs::read_to_string(&json_path)?;
        print!("{}", content);
        let _ = std::fs::remove_file(&json_path);
    } else {
        eprintln!("Wrote JSON manifest to {:?}", output_path);
    }

    Ok(())
}
