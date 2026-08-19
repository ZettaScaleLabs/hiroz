#!/bin/sh
# Test scripts/ci/apt-retry.sh against stub commands.
#
#   scripts/ci/test-apt-retry.sh
#
# It needs no apt and no network. Stubs on PATH replace the apt command and
# sleep, so the backoff costs no time. A fixture tree replaces /etc/apt, so
# the test checks the mirror rewrite.
set -u

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SUT="$HERE/apt-retry.sh"
WORK="${APT_RETRY_TEST_DIR:-$HERE/../../_tmp/apt-retry-test}"

rm -rf "$WORK"
mkdir -p "$WORK/bin"
BIN="$WORK/bin"
PATH="$BIN:$PATH"
export PATH

# The sleep stub removes the 15s/30s/45s backoff. So the test proves the
# retry count, not the delays.
printf '#!/bin/sh\nexit 0\n' > "$BIN/sleep"
# fallback_mirror runs `apt-get update`. Keep it out of the call count.
printf '#!/bin/sh\nexit 0\n' > "$BIN/apt-get"
printf '#!/bin/sh\necho x >> "$CALLS"\nexit 0\n' > "$BIN/stub-ok"
printf '#!/bin/sh\necho x >> "$CALLS"\nexit 1\n' > "$BIN/stub-fail"
# It succeeds only after the switch, so it separates the two attempts.
printf '#!/bin/sh\necho x >> "$CALLS"\n[ -f "$APT_RETRY_FLAG" ]\n' > "$BIN/stub-until-switch"
chmod +x "$BIN"/*

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

# Each case gets its own flag path and call log, so no case sees another's state.
setup() { # case name
    CASE="$WORK/$1"
    mkdir -p "$CASE"
    CALLS="$CASE/calls"
    : > "$CALLS"
    APT_RETRY_FLAG="$CASE/flag"
    APT_RETRY_ROOT="$CASE/root"
    export CALLS APT_RETRY_FLAG APT_RETRY_ROOT
}

calls() { wc -l < "$CALLS" | tr -d ' '; }
# grep -c prints 0 and exits 1 on no match. An `|| echo 0` here would
# add a second zero.
switches() { grep -c 'switching to the generic' "$CASE/err" 2>/dev/null; }

run() { # runs the SUT, capturing stderr and rc
    "$SUT" "$@" > "$CASE/out" 2> "$CASE/err"
    echo $?
}

echo "== no arguments =="
setup no-args
RC=$(run)
check "no args rc" 2 "$RC"
check "no args names usage" 1 "$(grep -c 'usage' "$CASE/err")"
check "no args runs nothing" 0 "$(calls)"

echo "== command succeeds =="
setup ok
RC=$(run stub-ok)
check "ok rc" 0 "$RC"
check "ok runs once" 1 "$(calls)"
check "ok never switches" 0 "$(switches)"

echo "== command always fails =="
setup fail
RC=$(run stub-fail)
check "fail rc" 1 "$RC"
check "fail runs four times per mirror" 8 "$(calls)"
check "fail switches once" 1 "$(switches)"
check "fail names both mirrors" 1 "$(grep -c 'failed on both mirrors' "$CASE/err")"

echo "== fails on the fast mirror, succeeds on the generic one =="
setup recover
RC=$(run stub-until-switch)
check "recover rc" 0 "$RC"
check "recover runs four times, then once" 5 "$(calls)"
check "recover switches once" 1 "$(switches)"

echo "== a job that already switched does not switch again =="
setup already
mkdir -p "$(dirname "$APT_RETRY_FLAG")"
: > "$APT_RETRY_FLAG"
RC=$(run stub-fail)
check "already rc" 1 "$RC"
check "already switches zero times" 0 "$(switches)"
check "already still retries both rounds" 8 "$(calls)"

echo "== the switch rewrites both sources file shapes =="
setup rewrite
mkdir -p "$APT_RETRY_ROOT/etc/apt/sources.list.d"
AZ=http://azure.archive.ubuntu.com/ubuntu
echo "deb $AZ noble main" > "$APT_RETRY_ROOT/etc/apt/sources.list"
printf 'Types: deb\nURIs: %s\n' "$AZ" > "$APT_RETRY_ROOT/etc/apt/sources.list.d/ubuntu.sources"
run stub-fail > /dev/null
check "sources.list loses the azure host" 0 \
    "$(grep -c 'azure' "$APT_RETRY_ROOT/etc/apt/sources.list")"
check "sources.list gains the generic host" 1 \
    "$(grep -c 'http://archive.ubuntu.com/ubuntu' "$APT_RETRY_ROOT/etc/apt/sources.list")"
check "deb822 loses the azure host" 0 \
    "$(grep -c 'azure' "$APT_RETRY_ROOT/etc/apt/sources.list.d/ubuntu.sources")"
check "deb822 gains the generic host" 1 \
    "$(grep -c 'http://archive.ubuntu.com/ubuntu' "$APT_RETRY_ROOT/etc/apt/sources.list.d/ubuntu.sources")"

echo
echo "apt-retry: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
