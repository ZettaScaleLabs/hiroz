#!/usr/bin/env bash
set -e

# Build hu (the plugin host) and the WASM plugins.
# Plugins are excluded from the workspace and must be built via --manifest-path.
cargo build -p hiroz-union --release -j4
PLUGIN_DIR="crates/hiroz-union/plugins"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-meter/Cargo.toml" --release -j4
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-monitor/Cargo.toml" --release -j4

# Resolve the WASM output directory and expose it as HU_PLUGIN_PATH so that
# `hu meter` / `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/release"

cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy --release -j4 2>&1
