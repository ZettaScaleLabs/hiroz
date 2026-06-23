#!/usr/bin/env bash
# Temporary: hz saturation benchmark — restore after job completes.
set -euo pipefail

# Build hu-meter WASM plugin (excluded from workspace; must build explicitly).
RUSTFLAGS="" cargo build \
  --manifest-path crates/hiroz-union/plugins/hu-meter/Cargo.toml \
  --target wasm32-wasip2 \
  --release \
  -j4

PLUGIN_DIR="${CARGO_TARGET_DIR:-target}/wasm32-wasip2/release"
ln -sf "$PLUGIN_DIR/hu_meter.wasm" "$PLUGIN_DIR/hu-meter.wasm"

HU_PLUGIN_PATH="$PLUGIN_DIR" exec cargo nextest run -p hiroz-tests \
  --features hz-comparison-tests,jazzy \
  --test hz_accuracy \
  test_hz_python_saturation \
  --no-capture
