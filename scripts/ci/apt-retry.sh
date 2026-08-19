#!/bin/sh
# Run an apt command so an unreachable mirror does not fail the job.
#
#   apt-retry.sh apt-get update
#   apt-retry.sh apt-get install -y foo bar
#
# Retries the command with backoff, then once against the generic mirror. The
# workflow sets Acquire::Retries, which covers a single file with no delay.
# hiroz#308 has the measurement behind each layer.
set -u

# Refuse to do nothing quietly: with no command, `until "$@"` succeeds and this
# would exit 0 having run no apt at all.
[ "$#" -gt 0 ] || { echo "apt-retry: usage: apt-retry.sh <command> [args...]" >&2; exit 2; }

# One switch per job; without the flag a later apt call repeats it.
# Both paths are overridable so test-apt-retry.sh drives a fixture tree.
FLAG="${APT_RETRY_FLAG:-/var/tmp/.apt-fell-back}"
ROOT="${APT_RETRY_ROOT:-}"

# Undo the workflow mirror step. Same two file shapes it handles.
fallback_mirror() {
    echo "apt-retry: switching to the generic Ubuntu mirror" >&2
    if [ -f "$ROOT/etc/apt/sources.list" ]; then
        sed -i 's|http://azure\.archive\.ubuntu\.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' \
            "$ROOT/etc/apt/sources.list"
    fi
    # Guard with `ls`: a non-matching glob expands to the literal pattern.
    if ls "$ROOT"/etc/apt/sources.list.d/*.sources > /dev/null 2>&1; then
        sed -i 's|http://azure\.archive\.ubuntu\.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' \
            "$ROOT"/etc/apt/sources.list.d/*.sources
    fi
}

attempt() {
    n=0
    until "$@"; do
        n=$((n + 1))
        [ "$n" -ge 4 ] && return 1
        echo "apt-retry: attempt $n failed; retrying in $((n * 15))s" >&2
        sleep $((n * 15))
    done
    return 0
}

attempt "$@" && exit 0

if [ ! -f "$FLAG" ]; then
    fallback_mirror
    : > "$FLAG"
    # Best effort; the caller fails on its own terms if the index is unusable.
    apt-get update > /dev/null 2>&1 || true
fi

attempt "$@" && exit 0
echo "apt-retry: '$*' failed on both mirrors" >&2
exit 1
