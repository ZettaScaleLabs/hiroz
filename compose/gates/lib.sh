#!/usr/bin/env bash
# Shared helpers for the interop gate scripts.

# Source ROS before `set -u`: ament setup scripts reference unset variables.
source /opt/ros/jazzy/setup.bash
set -uo pipefail

# Flags for ros2 CLI graph/parameter commands. Mirrors crates/hiroz-tests
# (parameter_interop.rs): --no-daemon avoids stale daemon state, --spin-time
# bounds discovery.
ROS2_FLAGS=(--spin-time 2 --no-daemon)

# retry <attempts> <sleep_seconds> <description> -- <command...>
retry() {
  local attempts=$1 sleep_s=$2 desc=$3
  shift 3
  [[ "${1:-}" == "--" ]] && shift
  local i
  for ((i = 1; i <= attempts; i++)); do
    if "$@"; then
      return 0
    fi
    echo "  attempt ${i}/${attempts} failed: ${desc}" >&2
    sleep "$sleep_s"
  done
  return 1
}

green() { echo "GREEN  $*"; }
red() { echo "RED    $*"; }
