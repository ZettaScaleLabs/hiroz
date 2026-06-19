pub mod abi;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub commands: Vec<String>,
}

/// Scan PATH for `hu-*` binaries and return their paths.
pub fn discover_plugins() -> Vec<(String, PathBuf)> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut plugins = Vec::new();

    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(plugin_name) = name.strip_prefix("hu-")
                && seen.insert(plugin_name.to_string())
            {
                plugins.push((plugin_name.to_string(), entry.path()));
            }
        }
    }

    plugins.sort_by(|a, b| a.0.cmp(&b.0));
    plugins
}

/// Query a plugin binary for its manifest via `--hu-manifest`.
pub fn query_manifest(path: &std::path::Path) -> Option<PluginManifest> {
    let output = std::process::Command::new(path)
        .arg("--hu-manifest")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Dispatch `hu <plugin> [args...]`.
///
/// Tries dynamic loading (`libhu_<plugin>.so`) first; falls back to a
/// `hu-<plugin>` subprocess on PATH.  Passes `HU_ROUTER` and `HU_DOMAIN`
/// as environment variables in both cases.
pub fn dispatch(
    plugin_name: &str,
    args: &[String],
    router: &str,
    domain: usize,
) -> anyhow::Result<std::process::ExitStatus> {
    // Set env vars so both the .so and subprocess paths pick them up.
    // SAFETY: we are single-threaded at this point (pre-tokio dispatch).
    unsafe {
        std::env::set_var("HU_ROUTER", router);
        std::env::set_var("HU_DOMAIN", domain.to_string());
    }

    // argv[0] = canonical binary name; rest = the args the caller passed.
    let full_args: Vec<String> = std::iter::once(format!("hu-{}", plugin_name))
        .chain(args.iter().cloned())
        .collect();

    if let Some(code) = abi::try_dispatch_so(plugin_name, &full_args) {
        std::process::exit(code);
    }

    // Fall back to subprocess.
    let binary = format!("hu-{}", plugin_name);
    let path = which::which(&binary).map_err(|_| {
        anyhow::anyhow!(
            "Plugin '{}' not found on PATH (no .so found either)",
            binary
        )
    })?;

    let status = std::process::Command::new(path)
        .args(args)
        .env("HU_ROUTER", router)
        .env("HU_DOMAIN", domain.to_string())
        .status()?;

    Ok(status)
}
