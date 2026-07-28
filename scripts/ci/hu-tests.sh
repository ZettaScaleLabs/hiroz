#!/usr/bin/env bash
set -e

# Unify the target dir across the main workspace and the separate plugins
# workspace. Must be absolute: hiroz-tests binaries run with CWD at the crate
# root, so a relative path in the PATH entry below would misresolve.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
# Normalize a caller-provided relative CARGO_TARGET_DIR to absolute.
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) CARGO_TARGET_DIR="$(pwd)/$CARGO_TARGET_DIR"; export CARGO_TARGET_DIR ;;
esac

# Build hu (the plugin host) and the WASM plugins. Plugins are excluded from
# the workspace, so build each via --manifest-path. Debug, not --release: cost
# is dominated by the serial test run below, so release compile time is pure
# overhead.
cargo build -p hiroz-union
PLUGIN_DIR="crates/hiroz-union/plugins"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-meter/Cargo.toml"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-monitor/Cargo.toml"
# Build hu-plugin-template too so the suite loads it through the real host
# (`hu plugin validate`/`list`), catching WIT/ABI regressions a compile-only
# check would miss.
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-plugin-template/Cargo.toml"

# Expose the WASM output directory as HU_PLUGIN_PATH so that `hu meter` /
# `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/debug"

# Tests spawn `hu` by name (Command::new("hu")) — put the built binary on PATH.
export PATH="${TARGET_DIR}/debug:${PATH}"

# `hu meter pub` resolves the message schema from `.msg` files on disk (the
# dynamic-schema-loader), so it can publish to an empty topic without a live
# node — exactly like `ros2 topic pub`. This job runs in `.#pureRust-ci` (no
# ROS env / no AMENT_PREFIX_PATH), so point the loader at the bundled codegen
# assets, which ship the standard `.msg` set in prefix layout
# (<pkg>/msg/<Name>.msg). Absolute: `hu` is spawned with an arbitrary CWD.
export HIROZ_MSG_PATH="$(pwd)/crates/hiroz-codegen/assets/jazzy"

# Run single-threaded: each test spawns a router + hiroz node + `hu` subprocess
# (JIT wasmtime), and two at once on the 2-core runner over-subscribe the cores
# and starve graph discovery into a timeout. Wall-clock ≈ parallel here anyway.
cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy -- --test-threads=1
cargo test -p hiroz-tests --test hu_monitor --features hu-monitor-tests,jazzy -- --test-threads=1
