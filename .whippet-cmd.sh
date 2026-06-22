#!/usr/bin/env bash
# Whippet CI command. Excludes ROS-dependent crates when AMENT_PREFIX_PATH is not set
# (i.e., in pureRust-ci devshell which lacks ROS 2 headers).
set -euo pipefail

# Exclude rmw-zenoh-rs if clang cannot actually compile rcutils headers.
# File presence alone is insufficient: AMENT_PREFIX_PATH may be inherited
# from the host but clang's include path may not cover those dirs.
EXCLUDE_ARGS=""
CLANG="${CLANG_PATH:-clang}"
if ! echo '#include <rcutils/strdup.h>' \
     | "$CLANG" -x c -fsyntax-only - 2>/dev/null; then
  EXCLUDE_ARGS="--exclude rmw-zenoh-rs"
fi

exec cargo test --no-run --message-format json-render-diagnostics --workspace $EXCLUDE_ARGS
