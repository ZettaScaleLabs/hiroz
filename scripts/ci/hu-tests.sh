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

# Run strictly single-threaded (`--test-threads=1`). Each test spawns a Zenoh
# router + a hiroz node + a `hu` subprocess (which JIT-loads wasmtime for the
# plugin). On the 2-core `ubuntu-latest` runner, default parallelism runs two
# of these CPU-heavy tests at once and each already over-subscribes both cores
# with its own subprocesses, starving hu's liveliness-graph discovery until it
# times out (or deadlocks).
#
# The determinism work on this branch (Graph::wait_settled for one-shot
# commands, pub_.wait_for_subscription on the publisher side) removed the
# *timer*-based races and let the previously-quarantined one-shot tests be
# un-ignored — but a condition-based wait still cannot manufacture CPU time
# that isn't there. Serial execution gives each test the whole machine, which
# is what discovery needs. Wall-clock is comparable either way: two CPU-bound
# tests on 2 cores run at ~half speed each, so parallel ≈ serial here.
cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy -- --test-threads=1
cargo test -p hiroz-tests --test hu_monitor --features hu-monitor-tests,jazzy -- --test-threads=1
