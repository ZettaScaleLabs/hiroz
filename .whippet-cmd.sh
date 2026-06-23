#!/usr/bin/env bash
# Whippet CI command for bridge-interop-ci devshell.
# Runs the full workspace test suite (bridge-interop-ci has Jazzy + Humble ROS 2).
set -euo pipefail
exec cargo nextest run --workspace --exclude rmw-zenoh-rs --no-fail-fast
