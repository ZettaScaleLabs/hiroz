//! Installing and removing WASM plugins.
//!
//! `hu meter` and `hu monitor` are not built into the binary — they are
//! `.wasm` components discovered on the plugin path. A user who downloads
//! `hu` therefore has no plugins at all until something puts them there.
//! This module is that something.
//!
//! Three sources are accepted, in decreasing order of how much we can check:
//!
//! - a **local path**, validated as a component before it is accepted;
//! - a **URL**, downloaded and, if a `.sha256` sits alongside it, verified;
//! - a **name** resolved through a release index, which carries a checksum
//!   and the WIT world the plugin was built against.
//!
//! Note on trust: the checksums here protect against corruption and accidental
//! substitution. They are *not* authenticity — nothing is signed, and the
//! plugin permission model is self-declared by the plugin (see the WIT source).
//! Installing a plugin means trusting whoever wrote it.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::wasm::{plugin_search_dirs, sanitize_plugin_stem, validate_plugin_static};

/// WIT world this build of `hu` hosts. A plugin built against a different
/// world will not instantiate, so we refuse it up front with a readable
/// message instead of letting wasmtime fail later with a link error.
pub const HOST_WIT_WORLD: &str = "hu:plugin@0.1.0";

const DEFAULT_REGISTRY_ENV: &str = "HU_PLUGIN_REGISTRY";

#[derive(Debug, Deserialize)]
struct RegistryIndex {
    #[allow(dead_code)]
    schema: u32,
    wit_world: String,
    plugins: Vec<RegistryEntry>,
}

#[derive(Debug, Deserialize)]
struct RegistryEntry {
    name: String,
    file: String,
    version: String,
    sha256: String,
}

/// Record of what was installed, so `hu plugin list` can tell a released
/// plugin from one a developer dropped in by hand.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct InstalledDb {
    #[serde(default)]
    pub plugins: Vec<InstalledEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledEntry {
    pub name: String,
    pub file: String,
    pub version: String,
    pub source: String,
}

/// The directory installs write to: always the last search dir, which is the
/// per-user one. `$HU_PLUGIN_PATH` entries are deliberately not written to —
/// those point at build trees during development and are not ours to manage.
pub fn install_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    Ok(home.join(".local/share/hu/plugins"))
}

fn db_path() -> Result<PathBuf> {
    Ok(install_dir()?.join("installed.json"))
}

pub fn load_db() -> InstalledDb {
    db_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_db(db: &InstalledDb) -> Result<()> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(db)?)
        .with_context(|| format!("writing {}", path.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Drop a trailing `-<version>` from a plugin filename stem.
///
/// Only a suffix made purely of digits and dots counts, so `hu_meter-0.1.0`
/// loses its version while a plugin genuinely named `hu_my-tool` keeps its
/// name. Conservative on purpose: mangling a legitimate name would silently
/// rename someone's subcommand.
fn strip_version_suffix(stem: &str) -> &str {
    match stem.rsplit_once('-') {
        Some((head, tail))
            if !head.is_empty()
                && !tail.is_empty()
                && tail.chars().all(|c| c.is_ascii_digit() || c == '.')
                && tail.chars().any(|c| c.is_ascii_digit()) =>
        {
            head
        }
        _ => stem,
    }
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Download over `curl`. `hu` deliberately carries no HTTP client — pulling in
/// a TLS stack for an occasional convenience command is a poor trade, and
/// `curl` is present anywhere a user could have downloaded `hu` in the first
/// place.
fn http_get(url: &str) -> Result<Vec<u8>> {
    let mut cmd = std::process::Command::new("curl");
    // `--fail` matters: without it an HTTP error page is written to stdout and
    // we would cheerfully install a 404 as a plugin.
    cmd.args(["-fsSL", "--", url]);
    if let Ok(token) = std::env::var("HU_RELEASE_TOKEN") {
        cmd.arg("-H").arg(format!("Authorization: token {token}"));
    }
    let out = cmd
        .output()
        .with_context(|| "running curl (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "download failed for {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Accept a `.wasm` payload: check the world, check it compiles as a
/// component, then place it. Returns the installed path.
fn accept(
    bytes: &[u8],
    file_name: &str,
    expected_sha: Option<&str>,
    source: &str,
    version: &str,
) -> Result<PathBuf> {
    if let Some(want) = expected_sha {
        let got = sha256_hex(bytes);
        if !want.eq_ignore_ascii_case(&got) {
            bail!(
                "checksum mismatch for {file_name}\n  expected {want}\n  got      {got}\n\
                 The download is corrupt or has been altered. Nothing was installed."
            );
        }
    }

    // The file name comes from a URL or an index we did not write, so it is
    // attacker-influenced. Reuse the same sanitizer the plugin work dirs use:
    // it collapses `..` and separators, so the result is one safe segment and
    // cannot escape the install dir.
    //
    // Strip the version first. Release assets are named `hu_meter-0.1.0.wasm`,
    // and discovery derives the subcommand from the filename — so keeping the
    // suffix would install `hu meter-0_1_0` instead of `hu meter`, i.e. the
    // documented command would not exist. Must happen before sanitizing, which
    // turns the dots into underscores and makes the suffix unrecognizable.
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let safe = sanitize_plugin_stem(strip_version_suffix(stem));

    let dir = install_dir()?;
    std::fs::create_dir_all(&dir)?;

    // Validate before it lands on the plugin path, using a temp file — a
    // component that will not compile must never be discoverable, not even
    // briefly.
    let tmp = dir.join(format!(".{safe}.wasm.partial"));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    let validation = validate_plugin_static(&tmp);
    if let Err(e) = validation {
        let _ = std::fs::remove_file(&tmp);
        bail!("{file_name} is not a loadable WASM component: {e}");
    }

    let dest = dir.join(format!("{safe}.wasm"));
    std::fs::rename(&tmp, &dest).with_context(|| format!("installing {}", dest.display()))?;

    let display_name = safe
        .strip_prefix("hu_")
        .or_else(|| safe.strip_prefix("hu-"))
        .unwrap_or(&safe)
        .to_string();

    let mut db = load_db();
    db.plugins.retain(|p| p.name != display_name);
    db.plugins.push(InstalledEntry {
        name: display_name,
        file: format!("{safe}.wasm"),
        version: version.to_string(),
        source: source.to_string(),
    });
    save_db(&db)?;

    Ok(dest)
}

/// Install from a local path, a URL, or a name resolved through the registry.
pub fn install(source: &str, registry: Option<&str>) -> Result<PathBuf> {
    let local = Path::new(source);
    if local.exists() {
        let bytes = std::fs::read(local).with_context(|| format!("reading {source}"))?;
        let name = local
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin.wasm");
        // A sibling `.sha256` is honoured when present; its absence is not a
        // failure for a local file the user already has in hand.
        let sidecar = local.with_extension("wasm.sha256");
        let expected = std::fs::read_to_string(&sidecar)
            .ok()
            .and_then(|s| s.split_whitespace().next().map(str::to_string));
        return accept(&bytes, name, expected.as_deref(), source, "local");
    }

    if is_url(source) {
        let bytes = http_get(source)?;
        let name = source.rsplit('/').next().unwrap_or("plugin.wasm");
        let expected = http_get(&format!("{source}.sha256"))
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.split_whitespace().next().map(str::to_string));
        return accept(&bytes, name, expected.as_deref(), source, "url");
    }

    install_from_registry(source, registry)
}

fn install_from_registry(name: &str, registry: Option<&str>) -> Result<PathBuf> {
    let index_url = registry
        .map(str::to_string)
        .or_else(|| std::env::var(DEFAULT_REGISTRY_ENV).ok())
        .ok_or_else(|| {
            anyhow!(
                "'{name}' is not an existing file or a URL, and no plugin registry is configured.\n\
                 Set {DEFAULT_REGISTRY_ENV} to a release index URL, pass --registry, or install \
                 from a downloaded file:\n  hu plugin install ./hu_{name}.wasm"
            )
        })?;

    let raw = http_get(&index_url)?;
    let index: RegistryIndex = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing plugin index at {index_url}"))?;

    if index.wit_world != HOST_WIT_WORLD {
        bail!(
            "plugin index targets WIT world {} but this hu hosts {}.\n\
             Install a release matching this hu, or upgrade hu.",
            index.wit_world,
            HOST_WIT_WORLD
        );
    }

    let entry = index
        .plugins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            let available: Vec<&str> = index.plugins.iter().map(|p| p.name.as_str()).collect();
            anyhow!(
                "no plugin named '{name}' in the index. Available: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;

    // Assets sit next to the index.
    let base = index_url
        .rsplit_once('/')
        .map(|(b, _)| b.to_string())
        .unwrap_or_default();
    let asset_url = format!("{base}/{}", entry.file);
    let bytes = http_get(&asset_url)?;

    accept(
        &bytes,
        &entry.file,
        Some(&entry.sha256),
        &asset_url,
        &entry.version,
    )
}

/// Remove an installed plugin by its subcommand name.
pub fn uninstall(name: &str) -> Result<PathBuf> {
    let dir = install_dir()?;
    let candidates = [
        dir.join(format!("hu_{name}.wasm")),
        dir.join(format!("hu-{name}.wasm")),
        dir.join(format!("{name}.wasm")),
    ];
    let found = candidates.iter().find(|p| p.exists()).ok_or_else(|| {
        // Point at the real cause when the plugin is on the path but not in
        // the dir we manage — removing it is not ours to do.
        let elsewhere = plugin_search_dirs()
            .into_iter()
            .filter(|d| *d != dir)
            .any(|d| {
                ["hu_", "hu-", ""]
                    .iter()
                    .any(|p| d.join(format!("{p}{name}.wasm")).exists())
            });
        if elsewhere {
            anyhow!(
                "'{name}' is loaded from a directory on $HU_PLUGIN_PATH, not from {}. \
                 Remove it there, or unset $HU_PLUGIN_PATH.",
                dir.display()
            )
        } else {
            anyhow!("no installed plugin named '{name}' in {}", dir.display())
        }
    })?;

    std::fs::remove_file(found).with_context(|| format!("removing {}", found.display()))?;

    let mut db = load_db();
    db.plugins.retain(|p| p.name != name);
    let _ = save_db(&db);

    Ok(found.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn traversal_in_a_downloaded_name_cannot_escape_the_install_dir() {
        // The name is attacker-influenced; every separator and dot must be
        // collapsed so the result stays one segment.
        for evil in ["../../../etc/passwd", "..", "a/b/c", "hu_../x"] {
            let stem = Path::new(evil)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let safe = sanitize_plugin_stem(stem);
            assert!(!safe.contains('/'), "{evil} → {safe}");
            assert!(!safe.contains(".."), "{evil} → {safe}");
            assert_eq!(Path::new(&safe).components().count(), 1, "{evil} → {safe}");
        }
    }

    #[test]
    fn release_asset_names_install_under_their_plain_subcommand_name() {
        // Regression: installing the release asset `hu_meter-0.1.0.wasm` gave
        // the subcommand `hu meter-0_1_0`, so the documented `hu meter` did
        // not exist after installing exactly what the release publishes.
        assert_eq!(strip_version_suffix("hu_meter-0.1.0"), "hu_meter");
        assert_eq!(strip_version_suffix("hu_monitor-1.2.3"), "hu_monitor");
        assert_eq!(strip_version_suffix("hu_meter-12"), "hu_meter");
    }

    #[test]
    fn a_name_that_merely_contains_a_hyphen_is_left_alone() {
        // Renaming someone's subcommand because it has a hyphen would be
        // worse than leaving a version on.
        assert_eq!(strip_version_suffix("hu_my-tool"), "hu_my-tool");
        assert_eq!(strip_version_suffix("hu_meter"), "hu_meter");
        assert_eq!(strip_version_suffix("hu_a-b-c"), "hu_a-b-c");
        assert_eq!(strip_version_suffix("-1.0"), "-1.0");
        assert_eq!(strip_version_suffix("hu_x-"), "hu_x-");
        assert_eq!(strip_version_suffix("hu_x-..."), "hu_x-...");
    }

    #[test]
    fn url_detection_does_not_treat_a_path_as_a_url() {
        assert!(is_url("https://example.com/hu_meter.wasm"));
        assert!(is_url("http://example.com/hu_meter.wasm"));
        assert!(!is_url("./hu_meter.wasm"));
        assert!(!is_url("meter"));
        assert!(!is_url("/home/u/hu_meter.wasm"));
    }
}
