#!/usr/bin/env bash
# Gate 4: graph introspection (liveliness) and the Fibonacci action.
source /gates/lib.sh
fail=0

# 4a: hiroz nodes are visible to ros2 graph introspection.
nodes_out=$(ros2 node list "${ROS2_FLAGS[@]}" 2>/dev/null)
for node in /talker /add_two_ints_server /fibonacci_action_server /param_node; do
  if ! grep -q "^${node}$" <<<"$nodes_out"; then
    red "4a ${node} missing from ros2 node list: ${nodes_out}"
    fail=1
  fi
done
if [[ "$fail" -eq 0 ]]; then
  green "4a all hiroz nodes visible in ros2 node list"
fi

# 4b: hiroz topics are visible.
check_topic_list() {
  ros2 topic list "${ROS2_FLAGS[@]}" 2>/dev/null | grep -q '^/chatter$'
}
if retry 5 2 "ros2 topic list /chatter" -- check_topic_list; then
  green "4b /chatter visible in ros2 topic list"
else
  red "4b /chatter missing from ros2 topic list"
  fail=1
fi

# 4c: full action round trip (goal -> feedback -> result) via the ros2 CLI.
# Jazzy's CLI/tutorials use action_tutorials_interfaces; feedback field is
# partial_sequence (see examples/demo_nodes/fibonacci_action_server.rs).
run_goal() {
  goal_out=$(timeout 90 ros2 action send_goal /fibonacci \
    action_tutorials_interfaces/action/Fibonacci '{order: 5}' --feedback 2>&1)
  grep -q 'partial_sequence' <<<"$goal_out" \
    && grep -q 'SUCCEEDED' <<<"$goal_out" \
    && grep -q '0, 1, 1, 2, 3, 5' <<<"$goal_out"
}
if retry 3 5 "ros2 action send_goal /fibonacci" -- run_goal; then
  green "4c fibonacci action: feedback + SUCCEEDED + result [0, 1, 1, 2, 3, 5]"
else
  red "4c fibonacci action failed; last output: ${goal_out:-<none>}"
  fail=1
fi

exit "$fail"
