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
# Debug builds, not --release: this job's actual cost is the ~40+
# #[serial_test::serial]-tagged tests running strictly one-at-a-time (plain
# cargo test runs the whole suite in one process, so serial_test's in-process
# lock actually applies here, unlike under nextest) -- release-vs-debug
# compile time was pure overhead on top of that, not something the test
# results depend on.
cargo build -p hiroz-union
PLUGIN_DIR="crates/hiroz-union/plugins"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-meter/Cargo.toml"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-monitor/Cargo.toml"
# hu-plugin-template is the starting point third-party authors copy. Build it
# into the same HU_PLUGIN_PATH so the hu_meter test suite can load it through
# the real component-model host (`hu plugin validate` / `hu plugin list`) and
# catch a WIT/ABI regression that a compile-only check would miss.
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-plugin-template/Cargo.toml"

# Expose the WASM output directory as HU_PLUGIN_PATH so that `hu meter` /
# `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/debug"

# Tests spawn `hu` as a subprocess by name (Command::new("hu")) — put the
# just-built binary on PATH.
export PATH="${TARGET_DIR}/debug:${PATH}"

cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy
cargo test -p hiroz-tests --test hu_monitor --features hu-monitor-tests,jazzy
