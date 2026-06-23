#!/usr/bin/env bash
set -e

# Build hu (the plugin host) and the WASM plugins.
cargo build -p hiroz-union --release -j4
cargo build -p hu-meter -p hu-monitor --target wasm32-wasip2 --release -j4

# Resolve the WASM output directory and expose it as HU_PLUGIN_PATH so that
# `hu meter` / `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/release"

cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy --release -j4 2>&1
