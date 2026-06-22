#!/usr/bin/env bash
# Whippet CI command. Excludes ROS-dependent crates when AMENT_PREFIX_PATH is not set
# (i.e., in pureRust-ci devshell which lacks ROS 2 headers).
set -euo pipefail

# rmw-zenoh-rs requires ROS 2 headers for bindgen (rcutils, rmw, etc.).
# These are not available in pureRust-ci or bridge-interop-ci devshells.
# Always exclude it here; test it separately with a ROS-capable devshell.
exec cargo test --no-run --message-format json-render-diagnostics \
  --workspace --exclude rmw-zenoh-rs
