#!/usr/bin/env bash
# Check if ros2cli#871 reproduces with rmw_cyclonedds_cpp.
#
# ros2 topic hz uses rclpy which deserializes every message in Python.
# For large payloads, Python deserialization becomes the bottleneck and
# ros2 topic hz under-reports the actual publish rate.
#
# This script publishes large std_msgs/String at 50 Hz via ros2 topic pub
# (cyclonedds), then concurrently measures with ros2 topic hz (cyclonedds).
# Under-reporting > 20% indicates the bug is present.
set -euo pipefail

TOPIC="/cyclone_hz_large"
TARGET_HZ=50
PAYLOAD_SIZE=5000000
MEASURE_SECS=12

echo "=== ros2cli#871 reproduction check (rmw_cyclonedds_cpp) ==="
echo "Payload:    ${PAYLOAD_SIZE} bytes"
echo "Target:     ${TARGET_HZ} Hz"
echo ""

if ! command -v ros2 &>/dev/null; then
    echo "ERROR: ros2 CLI not found"
    exit 1
fi

if ! RMW_IMPLEMENTATION=rmw_cyclonedds_cpp ros2 pkg list 2>/dev/null | grep -q cyclonedds; then
    echo "rmw_cyclonedds_cpp: not available — skipping"
    exit 0
fi

echo "rmw_cyclonedds_cpp: available"

PAYLOAD=$(python3 -c "print('x' * ${PAYLOAD_SIZE})")

# Start publisher in background.
RMW_IMPLEMENTATION=rmw_cyclonedds_cpp ros2 topic pub "${TOPIC}" \
    std_msgs/msg/String "{data: '${PAYLOAD}'}" \
    --rate ${TARGET_HZ} &
PUB_PID=$!
trap "kill ${PUB_PID} 2>/dev/null || true" EXIT

echo "Publisher started (PID ${PUB_PID}), waiting 2s for discovery..."
sleep 2

# Measure with ros2 topic hz (cyclonedds) for MEASURE_SECS seconds.
echo "Running ros2 topic hz for ${MEASURE_SECS}s..."
HZ_OUTPUT=$(RMW_IMPLEMENTATION=rmw_cyclonedds_cpp timeout $((MEASURE_SECS + 2)) \
    ros2 topic hz "${TOPIC}" --window 50 --filter $((MEASURE_SECS - 2)) 2>/dev/null || true)

echo ""
echo "=== ros2 topic hz output ==="
echo "${HZ_OUTPUT}"
echo ""

REPORTED_HZ=$(echo "${HZ_OUTPUT}" | grep "average rate:" | tail -1 | awk '{print $3}')

if [[ -z "${REPORTED_HZ}" ]]; then
    echo "WARNING: no rate reported — topic may not have been received"
    exit 0
fi

ERROR_PCT=$(python3 -c "
reported = float('${REPORTED_HZ}')
target = float('${TARGET_HZ}')
pct = abs(reported - target) / target * 100.0
print(f'{pct:.1f}')
")

echo "Reported:   ${REPORTED_HZ} Hz"
echo "Error:      ${ERROR_PCT}%"

if python3 -c "exit(0 if float('${ERROR_PCT}') > 20.0 else 1)"; then
    echo "→ ros2cli#871 REPRODUCED: ros2 topic hz under-reports by ${ERROR_PCT}% with ${PAYLOAD_SIZE}B payload"
else
    echo "→ ros2cli#871 NOT reproduced (error ${ERROR_PCT}% < 20%)"
fi
