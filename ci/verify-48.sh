#!/usr/bin/env bash
# Both-directions proof for the inline panic guard.
#
# Direction 1 (GREEN): the two tests pass as shipped.
# Direction 2 (RED):   ci/revert-48.patch removes the catch_unwind from
#                      local_only_shim's else arm, and
#                      a_panicking_callback_on_a_remote_sample_does_not_stop_delivery
#                      must FAIL while the control still passes.
#
# The control passing in direction 2 is what separates "the guard was removed"
# from "this configuration cannot deliver at all".
#
# Read the RESULT line. The runner is `set -e` and this script ends in `echo`,
# so the job reports success either way.

set +e
mkdir -p _tmp

echo "HEAD=$(git rev-parse HEAD)"
FEATURES="ros-msgs,jazzy"
T=panic_guard_inline
PANIC_TEST=a_panicking_callback_on_a_remote_sample_does_not_stop_delivery
CONTROL_TEST=delivery_continues_without_a_panic

# The file is gated `#![cfg(panic = "unwind")]`. Under an aborting profile it
# compiles to nothing and both directions would report 0 tests -- which must
# read as SKIPPED, never as passed.
run_both() {
    cargo test -p hiroz-tests --features "$FEATURES" --test "$T" -- --test-threads=1 \
        > "$1" 2>&1
    echo $?
}

echo "=== direction 1: as shipped ==="
GREEN=$(run_both _tmp/g48.log)
tail -20 _tmp/g48.log
G_PANIC=$(grep -c "^test $PANIC_TEST ... ok" _tmp/g48.log)
G_CTRL=$(grep -c "^test $CONTROL_TEST ... ok" _tmp/g48.log)
G_RAN=$(grep -cE "^test (a_panicking|delivery_continues)" _tmp/g48.log)
echo "GREEN=$GREEN G_RAN=$G_RAN G_PANIC_OK=$G_PANIC G_CTRL_OK=$G_CTRL"

if [ "$G_RAN" -eq 0 ]; then
    echo "RESULT48 VERDICT=SKIPPED reason=no_tests_ran_check_panic_strategy"
    echo SCRIPT_DONE
    exit 0
fi

git apply ci/revert-48.patch && APPLIED=yes || APPLIED=no
echo "REVERT_APPLIED=$APPLIED"
if [ "$APPLIED" != "yes" ]; then
    echo "RESULT48 VERDICT=ABORT reason=revert_would_not_apply"
    echo SCRIPT_DONE
    exit 0
fi

echo "=== direction 2: guard removed from the inline branch ==="
RED=$(run_both _tmp/r48.log)
tail -30 _tmp/r48.log
R_PANIC_OK=$(grep -c "^test $PANIC_TEST ... ok" _tmp/r48.log)
R_CTRL_OK=$(grep -c "^test $CONTROL_TEST ... ok" _tmp/r48.log)
R_RAN=$(grep -cE "^test (a_panicking|delivery_continues)" _tmp/r48.log)
echo "RED=$RED R_RAN=$R_RAN R_PANIC_OK=$R_PANIC_OK R_CTRL_OK=$R_CTRL_OK"

git apply -R ci/revert-48.patch
echo "REVERT_REVERSED_DIRTY=$(git status --porcelain crates/ | wc -l)"

# A compile failure in direction 2 gives RED!=0 with R_RAN=0. Reading that as
# "the detector fired" is the trap rules/verification.md records: cargo prints
# `error: test failed` when tests fail, so a non-zero status proves nothing.
if [ "$R_RAN" -eq 0 ]; then
    echo "RESULT48 VERDICT=INCONCLUSIVE reason=reverted_build_ran_no_tests green=$GREEN red=$RED"
    echo SCRIPT_DONE
    exit 0
fi

if [ "$GREEN" -eq 0 ] && [ "$R_PANIC_OK" -eq 0 ] && [ "$R_CTRL_OK" -eq 1 ]; then
    V=PROVEN
elif [ "$GREEN" -eq 0 ] && [ "$R_PANIC_OK" -eq 1 ]; then
    V=DETECTOR_DEAD
else
    V=INCONCLUSIVE
fi

echo "RESULT48 VERDICT=$V GREEN=$GREEN RED=$RED \
G_PANIC_OK=$G_PANIC G_CTRL_OK=$G_CTRL R_PANIC_OK=$R_PANIC_OK R_CTRL_OK=$R_CTRL_OK \
HEAD=$(git rev-parse HEAD)"
echo SCRIPT_DONE
