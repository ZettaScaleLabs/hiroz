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
PAYLOAD_SIZE=500000
MEASURE_SECS=12

# Force CycloneDDS to use unicast via loopback — multicast is often blocked in CI.
export CYCLONEDDS_URI='<CycloneDDS><Domain><General><Interfaces><NetworkInterface name="lo" multicast="false"/></Interfaces></General></Domain></CycloneDDS>'

echo "=== ros2cli#871 reproduction check (rmw_cyclonedds_cpp) ==="
echo "Payload:    ${PAYLOAD_SIZE} bytes"
echo "Target:     ${TARGET_HZ} Hz"
echo ""

if ! command -v ros2 &>/dev/null; then
    echo "ERROR: ros2 CLI not found"
    exit 1
fi

# Detect via LD_LIBRARY_PATH — ros2 pkg list is unreliable with buildEnv-merged ament indexes.
# The jazzy nix-ros-overlay pin on whippet may not include rmw_cyclonedds_cpp; skip cleanly.
# Uses a subshell with set +e to avoid pipefail/errexit interactions with glob misses.
CYCLONE_LIB=$(
    set +e
    IFS=: read -ra _dirs <<< "${LD_LIBRARY_PATH:-}"
    for _d in "${_dirs[@]:-}"; do
        [[ -z "$_d" ]] && continue
        for _f in "$_d"/librmw_cyclonedds*.so*; do
            [[ -f "$_f" ]] && echo "$_f" && exit 0
        done
    done
    exit 0
)
if [[ -z "$CYCLONE_LIB" ]]; then
    echo "rmw_cyclonedds_cpp: not in nix environment (librmw_cyclonedds* absent from LD_LIBRARY_PATH) — skipping"
    exit 0
fi

echo "rmw_cyclonedds_cpp: available ($CYCLONE_LIB)"

# Write payload and publisher script to temp files.
# Passing a large string as a shell argument hits ARG_MAX (~2MB on Linux).
# Using a heredoc with `python3 -` is unreliable in nix devshells — the
# background process stdin is disconnected, causing python to read EOF and exit.
PAYLOAD_FILE=$(mktemp)
PY_SCRIPT=$(mktemp --suffix=.py)
trap "rm -f '${PAYLOAD_FILE}' '${PY_SCRIPT}'; kill \${PUB_PID:-} 2>/dev/null || true" EXIT
python3 -c "
import sys
payload = 'x' * ${PAYLOAD_SIZE}
sys.stdout.write(payload)
" > "${PAYLOAD_FILE}"

cat > "${PY_SCRIPT}" << 'PYEOF'
import sys, time, rclpy
from rclpy.node import Node
from std_msgs.msg import String

topic, payload_file, rate_hz = sys.argv[1], sys.argv[2], float(sys.argv[3])
with open(payload_file) as f:
    payload = f.read()

rclpy.init()
node = Node('cyclone_hz_pub')
pub = node.create_publisher(String, topic, 10)
interval = 1.0 / rate_hz
while rclpy.ok():
    msg = String()
    msg.data = payload
    pub.publish(msg)
    time.sleep(interval)
PYEOF

RMW_IMPLEMENTATION=rmw_cyclonedds_cpp python3 "${PY_SCRIPT}" "${TOPIC}" "${PAYLOAD_FILE}" "${TARGET_HZ}" 2>&1 &
PUB_PID=$!

echo "Publisher started (PID ${PUB_PID}), waiting 2s for discovery..."
sleep 2

if ! kill -0 "${PUB_PID}" 2>/dev/null; then
    echo "ERROR: publisher process died during startup — skipping cyclone check"
    exit 0
fi

# Measure with ros2 topic hz (cyclonedds) for MEASURE_SECS seconds.
# --kill-after=2: send SIGKILL 2s after SIGTERM in case rclpy ignores SIGTERM.
echo "Running ros2 topic hz for ${MEASURE_SECS}s..."
HZ_OUTPUT=$(RMW_IMPLEMENTATION=rmw_cyclonedds_cpp \
    timeout --kill-after=2 $((MEASURE_SECS + 2)) \
    ros2 topic hz "${TOPIC}" --window 50 2>/dev/null || true)

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
