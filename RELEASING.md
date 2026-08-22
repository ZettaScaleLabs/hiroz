# Releasing hiroz

This document covers the full release process: local dry-run, CI smoke test, and cutting the real release.

## Version locations

Before releasing, bump the version in all three places consistently:

| File | Field | Controls |
|------|-------|----------|
| `Cargo.toml` | `[workspace.package] version` | **every crate under `crates/`** — they all inherit it, so this one row governs the crates.io versions, every `hu` release asset name, and what `hu --version` prints |
| `crates/hiroz-msgs/python/pyproject.toml` | `version` | the `hiroz-msgs-py` wheel |
| `crates/hiroz-py/pyproject.toml` | `version` | the `hiroz-py` wheel |

The `hiroz-py` wheel depends on `hiroz-msgs-py>=<version>` — update that lower bound too when bumping.

**One version governs every Rust crate, and a check enforces it.** `hiroz`, `hiroz-protocol` and `hiroz-union` each used to carry a literal `version`, which meant `cargo publish --workspace` could leave a published crate behind at the old number while the tag said otherwise, and a `v0.2.0` tag could produce `hu` assets named `0.1.0`. They now inherit, and `scripts/test-release-version-semantics.sh` fails if any crate under `crates/` reintroduces a literal. `ci.yml` runs it on every pull request.

`hu` ships from the workspace's own `v*` tags. It has neither an independent number nor, at present, an independent tag namespace.

> **Do not bump the WIT world alongside the product version.** `hu:plugin@0.1.0` is the plugin **ABI contract**, not a product version, and the two move on different clocks. It lives in three places that must agree — `HOST_WIT_WORLD` in `crates/hiroz-union/src/plugin/install.rs`, the `WIT_WORLD` constant in `scripts/build-hu-release.nu`, and the `package` line of `crates/hiroz-union/wit/v0.1/hu-plugin.wit` — and `install.rs` compares it to a release index by **exact string equality**. Bump the string and `hu plugin install <name>` refuses every index still declaring the old world, with a message telling the user to upgrade `hu` — for a change that never happened. Rename the package in `hu-plugin.wit` as well and the breakage is real rather than cosmetic: plugins built against the old package no longer instantiate. Change it only when the interface in `hu-plugin.wit` changes incompatibly, and then change all three sites in the same commit.

## Step 1 — Local dry-run (optional)

Build the Python wheels locally to catch obvious issues before touching CI:

```bash
# Build jazzy + humble wheels into crates/hiroz-py/dist/
./scripts/build-python-wheels.nu

# Build and immediately install into a local venv to verify import
./scripts/build-python-wheels.nu --install jazzy

# Single distro only
./scripts/build-python-wheels.nu jazzy
```

The script produces the same wheel filenames as CI (e.g. `hiroz_py-0.2.0-0jazzy-cp311-abi3-linux_x86_64.whl`).

## Step 2 — Smoke-test the release workflow

Before tagging a real version, verify the entire CI release pipeline works end-to-end using a throwaway tag:

```bash
./scripts/test-release-workflow.nu
```

This pushes `v<crate-version>-smoke-test` — e.g. `v0.1.0-smoke-test` — waits for all CI jobs to pass (builds, smoke tests, release creation), then reports the result. The script requires `gh` CLI authenticated to the repo.

The tag carries the current workspace version deliberately. `build-hu-release.nu` cross-checks a tag's core version against it and fails the build on a mismatch, so a fixed tag like `v0.0.0-smoke-test` dies at the first packaging step. The `-smoke-test` suffix makes it a semver pre-release, so it publishes as a pre-release and skips the crates.io step.

```bash
# Push only — skip the polling wait
./scripts/test-release-workflow.nu --no-wait

# Clean up the smoke-test tag and draft release afterward
./scripts/test-release-workflow.nu --cleanup
```

The CI pipeline exercises:

- All wheel builds (jazzy + humble × x86_64 Linux, aarch64 Linux, aarch64 macOS)
- The `hu` binary build (built with `--features web-plugins`, so the documented `hu web` subcommand works in the artifact users download)
- The `hu-meter` / `hu-monitor` WASM plugins (`hu_meter.wasm` / `hu_monitor.wasm`, `wasm32-wasip2`), plus `hu-plugins-<ver>.tar.gz` and the `hu-plugins-<ver>.json` index, built once in the `build-hu-plugins` job — the output is platform-independent, so it is not part of the per-target matrix
- All Go library builds (`libhiroz` static + shared)
- Python smoke test: install into venv, `import hiroz_py`
- Binary smoke test: `--help`, plus a clean-install check that unpacks the plugins into `~/.local/share/hu/plugins` with `HU_PLUGIN_PATH` unset and asserts `hu plugin list` finds `meter` and `monitor`
- Go smoke test: CGO compilation against the downloaded `.a`
- Install-from-release-URL test: `pip install` from the actual GitHub Release artifacts

Do not proceed to Step 3 until this is fully green.

## Step 3 — Cut the release

```bash
git tag v0.x.y
git push origin v0.x.y
```

This triggers `.github/workflows/release.yml` which:

1. Builds all wheels, binaries, and Go libraries in parallel
2. Runs smoke tests (same as Step 2)
3. Generates a changelog from conventional commits via `git-cliff`
4. Creates a GitHub Release with all artifacts attached

The release is live once the `Create GitHub Release` job completes (~15–20 min total).

## Changelog

Changelog entries are generated automatically from conventional commit messages. Only `feat` and `fix` commits appear by default; `chore`, `ci`, `style`, and `build` are filtered out. See `cliff.toml` for the full configuration.

To preview the changelog before releasing:

```bash
git cliff --latest --strip header
```
