#!/usr/bin/env bash
# Gate 1: topics, both directions.
source /gates/lib.sh
fail=0

# 1a: hiroz talker -> ros2 subscriber. The testbed's `talker` node publishes
# "Hello World: N" on /chatter every 500 ms.
check_chatter() {
  timeout 20 ros2 topic echo /chatter std_msgs/msg/String --once | grep -q "Hello World"
}
if retry 5 2 "ros2 topic echo /chatter" -- check_chatter; then
  green "1a hiroz talker -> ros2 echo on /chatter"
else
  red "1a no 'Hello World' seen on /chatter"
  fail=1
fi

# 1b: ros2 publisher -> hiroz -> ros2 subscriber round trip. The testbed's
# `chatter_echo` node republishes everything received on /ping to /pong, so
# seeing our payload on /pong proves the ros2 -> hiroz direction.
ros2 topic pub --rate 2 /ping std_msgs/msg/String '{data: ping-from-ros2}' >/dev/null 2>&1 &
PUB_PID=$!
check_pong() {
  timeout 15 ros2 topic echo /pong std_msgs/msg/String --once | grep -q "ping-from-ros2"
}
if retry 4 2 "ros2 topic echo /pong" -- check_pong; then
  green "1b ros2 pub /ping -> hiroz chatter_echo -> ros2 echo /pong"
else
  red "1b round trip via hiroz chatter_echo failed"
  fail=1
fi
kill "$PUB_PID" 2>/dev/null
wait "$PUB_PID" 2>/dev/null

exit "$fail"
