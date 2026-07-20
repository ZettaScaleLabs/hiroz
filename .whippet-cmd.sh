#!/usr/bin/env bash
# Whippet CI command — adapts to the active devShell:
#
#   pureRust-ci (wasm32-wasip2 target available via rustToolchainWasm):
#     Runs the pure-Rust checks via test-pure-rust.nu.
#     Skips test-shm (requires elevated prlimit) and bridge-interop tests.
#     Matches the GitHub CI no-ros-test + no-ros-checks jobs.
#
#   bridge-interop-ci / default (no wasm32-wasip2 target):
#     Runs the full workspace nextest suite.
#     rmw-zenoh-rs is a workspace member and compiles fine with ROS headers present.
#
# NOTE: We detect pureRust-ci by checking for the wasm32-wasip2 target, which is
# only installed in the pureRust-ci shell (via rustToolchainWasm). This is more
# reliable than AMENT_PREFIX_PATH, which can leak from the worker's ambient
# ROS environment into any devshell.
#
set -euo pipefail

if rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2; then
    exec nu scripts/test-pure-rust.nu \
        clippy-workspace \
        run-tests \
        check-bundled-msgs \
        check-console \
        check-examples \
        check-distro-features \
        clippy-hiroz-py
else
    exec cargo nextest run --workspace --no-fail-fast
fi
