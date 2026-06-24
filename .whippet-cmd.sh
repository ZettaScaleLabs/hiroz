#!/usr/bin/env bash
# Whippet CI command — adapts to the active devShell:
#
#   pureRust-ci (AMENT_PREFIX_PATH unset):
#     Runs the pure-Rust checks via test-pure-rust.nu.
#     Skips test-shm (requires elevated prlimit) and bridge-interop tests.
#     Matches the GitHub CI no-ros-test + no-ros-checks jobs.
#
#   bridge-interop-ci / default (AMENT_PREFIX_PATH set):
#     Runs the full workspace nextest suite.
#     rmw-zenoh-rs is a workspace member and compiles fine with ROS headers present.
#
set -euo pipefail

if [ -n "${AMENT_PREFIX_PATH:-}" ]; then
    exec cargo nextest run --workspace --no-fail-fast
else
    exec nu scripts/test-pure-rust.nu \
        clippy-workspace \
        run-tests \
        check-bundled-msgs \
        check-console \
        check-examples \
        check-distro-features \
        clippy-hiroz-py
fi
