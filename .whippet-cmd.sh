#!/usr/bin/env bash
# Whippet CI command. Excludes ROS-dependent crates when AMENT_PREFIX_PATH is not set
# (i.e., in pureRust-ci devshell which lacks ROS 2 headers).
set -euo pipefail

EXCLUDE_ARGS=""
if [[ -z "${AMENT_PREFIX_PATH:-}" ]]; then
  # pureRust-ci: no ROS headers, skip crates that need them
  EXCLUDE_ARGS="--exclude rmw-zenoh-rs"
fi

exec cargo test --no-run --message-format json-render-diagnostics --workspace $EXCLUDE_ARGS
