# Releasing hiroz

This document covers the full release process: local dry-run, CI smoke test, and cutting the real release.

## Version locations

Before releasing, bump the version in all three places consistently:

| File | Field |
|------|-------|
| `Cargo.toml` | `[workspace.package] version` |
| `crates/hiroz-msgs/python/pyproject.toml` | `version` |
| `crates/hiroz-py/pyproject.toml` | `version` |

The `hiroz-py` wheel depends on `hiroz-msgs-py>=<version>` — update that lower bound too when bumping.

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

This pushes `v0.0.0-smoke-test`, waits for all CI jobs to pass (builds, smoke tests, release creation), then reports the result. The script requires `gh` CLI authenticated to the repo.

```bash
# Push only — skip the polling wait
./scripts/test-release-workflow.nu --no-wait

# Clean up the smoke-test tag and draft release afterward
./scripts/test-release-workflow.nu --cleanup
```

The CI pipeline exercises:

- All wheel builds (jazzy + humble × x86_64 Linux, aarch64 Linux, aarch64 macOS)
- All binary builds (`hiroz-bridge`, `hiroz-console`)
- All Go library builds (`libhiroz` static + shared)
- Python smoke test: install into venv, `import hiroz_py`
- Binary smoke test: `--help` + 3-second runtime check (no crash)
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
