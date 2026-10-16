#!/bin/sh
# Test scripts/ci/check-buffer-skips.sh against fixture logs.
#
#   scripts/ci/test-check-buffer-skips.sh
#
# It needs no ROS, no cargo and no network. The fixtures hold the line a skipped buffer cell
# prints, and the lines a nextest run prints around it.
set -u

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SUT="$HERE/check-buffer-skips.sh"
WORK="${CHECK_BUFFER_SKIPS_TEST_DIR:-$HERE/../../_tmp/check-buffer-skips-test}"

rm -rf "$WORK"
mkdir -p "$WORK"

PASS=0
FAIL=0

check() { # label, expected, actual
    if [ "$2" = "$3" ]; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        echo "FAIL: $1: expected '$2', got '$3'" >&2
    fi
}

SKIP_LINE='Skipping joined_subscribers_receive_from_a_released_publisher: no buffer-aware rmw (version 0.10.4)'

printf 'running 1 test\n%s\ntest result: ok. 1 passed\n' "$SKIP_LINE" > "$WORK/skipped.log"
# nextest indents the captured output of a passing test.
printf '        PASS [   0.5s] hiroz-tests::x y\n  stdout ───\n    %s\n' "$SKIP_LINE" > "$WORK/skipped-indented.log"
printf 'running 1 test\ntest result: ok. 1 passed\n' > "$WORK/clean.log"
# A cell for a message with no buffer field says nothing about the rmw.
printf 'Skipping other_cell: no ros2 on PATH\n' > "$WORK/other-skip.log"

run() { # args; sets RC, OUT, ERR
    "$SUT" "$@" > "$WORK/out" 2> "$WORK/err"
    RC=$?
    OUT=$(cat "$WORK/out")
    ERR=$(cat "$WORK/err")
}

echo "== lyrical, a skipped buffer cell =="
run lyrical "$WORK/skipped.log"
check "lyrical skip rc" 1 "$RC"
check "lyrical skip names the cell" 1 "$(printf '%s\n%s\n' "$OUT" "$ERR" | grep -c 'joined_subscribers_receive_from_a_released_publisher')"
check "lyrical skip is an error annotation" 1 "$(printf '%s\n%s\n' "$OUT" "$ERR" | grep -c '^::error')"

echo "== lyrical, the skip line is indented =="
run lyrical "$WORK/skipped-indented.log"
check "lyrical indented skip rc" 1 "$RC"

echo "== other legs, a skipped buffer cell =="
for d in humble jazzy kilted; do
    run "$d" "$WORK/skipped.log"
    check "$d skip rc" 0 "$RC"
    check "$d skip is a warning annotation" 1 "$(printf '%s\n%s\n' "$OUT" "$ERR" | grep -c '^::warning')"
    check "$d skip raises no error annotation" 0 "$(printf '%s\n%s\n' "$OUT" "$ERR" | grep -c '^::error')"
done

echo "== no skip =="
run lyrical "$WORK/clean.log"
check "lyrical clean rc" 0 "$RC"
check "lyrical clean annotates nothing" 0 "$(printf '%s\n%s\n' "$OUT" "$ERR" | grep -c '^::')"
run jazzy "$WORK/clean.log"
check "jazzy clean rc" 0 "$RC"
check "jazzy clean annotates nothing" 0 "$(printf '%s\n%s\n' "$OUT" "$ERR" | grep -c '^::')"

echo "== a skip of another kind is not a buffer skip =="
run lyrical "$WORK/other-skip.log"
check "other skip rc" 0 "$RC"

echo "== the log is read from stdin when the argument is - =="
"$SUT" lyrical - < "$WORK/skipped.log" > /dev/null 2>&1
check "stdin skip rc" 1 "$?"
"$SUT" lyrical - < "$WORK/clean.log" > /dev/null 2>&1
check "stdin clean rc" 0 "$?"

echo "== a log that is missing or empty is unknown, not clean =="
run lyrical "$WORK/missing.log"
check "missing log rc" 2 "$RC"
: > "$WORK/empty.log"
run lyrical "$WORK/empty.log"
check "empty log rc" 2 "$RC"

echo "== bad arguments =="
run
check "no args rc" 2 "$RC"
"$SUT" > /dev/null 2>&1
check "no args direct rc" 2 "$?"

echo
echo "check-buffer-skips: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
