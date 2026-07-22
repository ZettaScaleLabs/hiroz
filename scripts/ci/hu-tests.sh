#!/usr/bin/env bash
set -e

# Resolve a single unified target dir up front and export it so every cargo
# invocation below — including the plugin builds, which live in their own
# separate Cargo workspace (crates/hiroz-union/plugins/Cargo.toml) and would
# otherwise build into crates/hiroz-union/plugins/target/ — writes into the
# same place. Must be absolute: `cargo test -p hiroz-tests` runs test
# binaries with their CWD set to the crate root (crates/hiroz-tests/), not
# the workspace root, so a relative PATH entry here silently resolves to the
# wrong directory.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"

# Build hu (the plugin host) and the WASM plugins.
# Plugins are excluded from the workspace and must be built via --manifest-path.
cargo build -p hiroz-union --release
PLUGIN_DIR="crates/hiroz-union/plugins"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-meter/Cargo.toml" --release
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-monitor/Cargo.toml" --release

# Expose the WASM output directory as HU_PLUGIN_PATH so that `hu meter` /
# `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/release"

# Tests spawn `hu` as a subprocess by name (Command::new("hu")) — put the
# just-built binary on PATH.
export PATH="${TARGET_DIR}/release:${PATH}"

cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy --release
cargo test -p hiroz-tests --test hu_monitor --features hu-monitor-tests,jazzy --release
