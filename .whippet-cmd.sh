#!/usr/bin/env bash
# Temporary: hz saturation benchmark — restore after job completes.
set -euo pipefail
exec cargo nextest run -p hiroz-tests \
  --features hz-comparison-tests,jazzy \
  --test hz_accuracy \
  test_hz_python_saturation \
  --no-capture
