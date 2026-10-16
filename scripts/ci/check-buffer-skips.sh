#!/bin/sh
# Fail a CI leg that skipped a buffer cell.
#
#   scripts/ci/check-buffer-skips.sh <distro> <log|->
#
# A buffer cell in `hiroz-tests` needs a buffer-aware `rmw_zenoh_cpp`. Where the library has no
# buffer mode, the cell prints `Skipping <cell>: no buffer-aware rmw (<reason>)` and returns, and
# cargo counts that as a pass. This script reads the run log. On the lyrical leg a skipped buffer
# cell is an error, because that leg must run the cell. On every other leg it is a warning
# annotation, because an older rmw cannot run the cell.
#
# The log is a file, or `-` for standard input. A missing or empty log is an error: an absent
# `Skipping` line proves nothing when there is no log to read.
#
# nextest hides the output of a passing test unless the profile sets `success-output`. The
# `interop` profile in `.config/nextest.toml` sets it, so the skip line reaches the log.
set -u

usage() {
    echo "usage: check-buffer-skips.sh <distro> <log|->" >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
DISTRO=$1
LOG=$2

if [ "$LOG" = "-" ]; then
    TEXT=$(cat)
else
    [ -f "$LOG" ] || { echo "check-buffer-skips: no log at $LOG" >&2; exit 2; }
    TEXT=$(cat "$LOG")
fi
[ -n "$TEXT" ] || { echo "check-buffer-skips: the log is empty, so it is unknown whether a cell skipped" >&2; exit 2; }

SKIPS=$(printf '%s\n' "$TEXT" | grep -E 'Skipping .*: no buffer-aware rmw' | sed 's/^[[:space:]]*//')
[ -n "$SKIPS" ] || exit 0

COUNT=$(printf '%s\n' "$SKIPS" | wc -l | tr -d ' ')
printf '%s\n' "$SKIPS" | while IFS= read -r line; do
    echo "check-buffer-skips: $line" >&2
done

if [ "$DISTRO" = "lyrical" ]; then
    echo "::error title=buffer cell skipped::$COUNT buffer cell(s) skipped on the lyrical leg: no buffer-aware rmw. A skipped cell passes in cargo's tally, so this leg proves nothing about them."
    exit 1
fi
echo "::warning title=buffer cell skipped::$COUNT buffer cell(s) skipped on the $DISTRO leg: no buffer-aware rmw."
exit 0
