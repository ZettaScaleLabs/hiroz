#!/usr/bin/env bash
# Gate 3: parameters via the ros2 param CLI against the hiroz `param_node`,
# which declares `count` (int, 0) and `label` (string, "hello"). Mirrors
# hiroz-tests/parameter_interop.rs.
source /gates/lib.sh
fail=0

check_node() {
  ros2 node list "${ROS2_FLAGS[@]}" 2>/dev/null | grep -q '^/param_node$'
}
if ! retry 15 2 "wait for /param_node" -- check_node; then
  red "3 /param_node never appeared in ros2 node list"
  exit 1
fi

list_out=$(ros2 param list /param_node "${ROS2_FLAGS[@]}" 2>&1)
if grep -q 'count' <<<"$list_out" && grep -q 'label' <<<"$list_out"; then
  green "3a ros2 param list sees count and label"
else
  red "3a param list missing count/label: ${list_out}"
  fail=1
fi

get_out=$(ros2 param get /param_node count "${ROS2_FLAGS[@]}" 2>&1)
if grep -q 'Integer value is: 0' <<<"$get_out"; then
  green "3b ros2 param get count -> 0"
else
  red "3b unexpected initial value: ${get_out}"
  fail=1
fi

set_out=$(ros2 param set /param_node count 42 "${ROS2_FLAGS[@]}" 2>&1)
if grep -q 'Set parameter successful' <<<"$set_out"; then
  green "3c ros2 param set count 42"
else
  red "3c param set failed: ${set_out}"
  fail=1
fi

get_out=$(ros2 param get /param_node count "${ROS2_FLAGS[@]}" 2>&1)
if grep -q 'Integer value is: 42' <<<"$get_out"; then
  green "3d ros2 param get count -> 42 after set"
else
  red "3d set did not stick: ${get_out}"
  fail=1
fi

exit "$fail"
