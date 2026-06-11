#!/usr/bin/env bash
# Runs every gate and prints a RED/GREEN summary. Exit code is the number of
# failed gates (0 == all GREEN), which docker compose propagates via
# --exit-code-from gates.
# Source ROS before `set -u`: ament setup scripts reference unset variables.
source /opt/ros/jazzy/setup.bash
set -uo pipefail

# Make sure no stale ros2 daemon with a different environment interferes.
ros2 daemon stop >/dev/null 2>&1 || true

results=()
fail_count=0
for gate in /gates/0*.sh; do
  name=$(basename "$gate" .sh)
  echo "=== Running gate: ${name} ==="
  if bash "$gate"; then
    results+=("GREEN  ${name}")
  else
    results+=("RED    ${name}")
    fail_count=$((fail_count + 1))
  fi
  echo
done

echo "=== Gate summary ==="
printf '%s\n' "${results[@]}"

exit "$fail_count"
