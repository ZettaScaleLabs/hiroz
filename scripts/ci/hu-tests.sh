#!/usr/bin/env bash
set -e

# Build hu (the plugin host) and the WASM plugins.
# Plugins are excluded from the workspace and must be built via --manifest-path.
cargo build -p hiroz-union --release
PLUGIN_DIR="crates/hiroz-union/plugins"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-meter/Cargo.toml" --release
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-monitor/Cargo.toml" --release

# Resolve the WASM output directory and expose it as HU_PLUGIN_PATH so that
# `hu meter` / `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/release"

# Tests spawn `hu` as a subprocess by name (Command::new("hu")) — put the
# just-built binary on PATH.
export PATH="${TARGET_DIR}/release:${PATH}"

cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy --release
cargo test -p hiroz-tests --test hu_monitor --features hu-monitor-tests,jazzy --release
