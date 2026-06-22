#!/usr/bin/env bash
# Whippet CI command. Excludes ROS-dependent crates when AMENT_PREFIX_PATH is not set
# (i.e., in pureRust-ci devshell which lacks ROS 2 headers).
set -euo pipefail

# Exclude rmw-zenoh-rs if rcutils headers are not actually present.
# AMENT_PREFIX_PATH may be inherited from the host env but point to paths
# that don't contain the headers (e.g. on the whippet worker in pureRust-ci).
EXCLUDE_ARGS=""
rcutils_found=false
for prefix in ${AMENT_PREFIX_PATH//:/ } /opt/ros/jazzy /opt/ros/humble /opt/ros/rolling; do
  if [[ -f "${prefix}/include/rcutils/strdup.h" ]] || \
     [[ -f "${prefix}/include/rcutils/rcutils/strdup.h" ]]; then
    rcutils_found=true
    break
  fi
done
if ! $rcutils_found; then
  EXCLUDE_ARGS="--exclude rmw-zenoh-rs"
fi

exec cargo test --no-run --message-format json-render-diagnostics --workspace $EXCLUDE_ARGS
