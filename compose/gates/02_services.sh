#!/usr/bin/env bash
# Gate 2: services, both directions.
source /gates/lib.sh
fail=0

# 2a: ros2 client -> hiroz server (mirrors
# hiroz-tests/service_interop.rs::test_hiroz_server_ros2_client).
call_hiroz_srv() {
  timeout 15 ros2 service call /add_two_ints \
    example_interfaces/srv/AddTwoInts '{a: 10, b: 7}' 2>&1 | grep -Eq 'sum[:=] ?17'
}
if retry 10 3 "ros2 service call /add_two_ints" -- call_hiroz_srv; then
  green "2a ros2 client -> hiroz add_two_ints server (sum=17)"
else
  red "2a ros2 service call /add_two_ints did not return sum=17"
  fail=1
fi

# 2b: hiroz client -> ros2-hosted server. Start a ROS 2 AddTwoInts server on
# /add_two_ints_ros2 (the name the testbed client polls with {a: 2, b: 3});
# once the testbed gets sum=5 it publishes "client_ok:sum=5" on
# /add_two_ints_client_ok every second.
ros2 run demo_nodes_cpp add_two_ints_server \
  --ros-args -r add_two_ints:=add_two_ints_ros2 >/dev/null 2>&1 &
SRV_PID=$!
check_client_ok() {
  timeout 10 ros2 topic echo /add_two_ints_client_ok std_msgs/msg/String --once \
    | grep -q 'client_ok:sum=5'
}
if retry 12 5 "echo /add_two_ints_client_ok" -- check_client_ok; then
  green "2b hiroz client called ros2-hosted /add_two_ints_ros2 (sum=5 confirmed)"
else
  red "2b hiroz client never reported success on /add_two_ints_client_ok"
  fail=1
fi
kill "$SRV_PID" 2>/dev/null
wait "$SRV_PID" 2>/dev/null

exit "$fail"
