#!/bin/sh
# Run an apt command so that an unreachable mirror does not fail the job.
#
#   apt-retry.sh apt-get update
#   apt-retry.sh apt-get install -y foo bar
#
# `Unable to connect to azure.archive.ubuntu.com` is a long-standing,
# non-deterministic failure on GitHub-hosted runners. People have reported it
# against actions/runner-images since 2019. It is not an incident that ends, so
# there is nothing to wait out. In one day it hit every distro in the interop
# matrix, let go for a whole run, then came back through a rerun.
#
# Three layers. Each earned its place: a measurement showed the one before it
# was not enough.
#
#   Acquire::Retries      set by the workflow. Retries a single file, with no
#                         delay. Covers a momentary blip. With it alone,
#                         `Install ROS dependencies` still failed after 38s.
#   backoff, here         retries the whole command: 15s, 30s, 45s. With this
#                         alone, `Install system dependencies` ran 127 seconds
#                         -- every attempt and every sleep -- and still failed.
#   generic mirror, here  the endpoints the workflow repoints away from. Slow,
#                         and reachable when the fast one is not.
#
# Both later layers have since rescued a real job: the backoff on lyrical, the
# mirror switch on jazzy after roughly four minutes of an unreachable host.
#
# Retrying here beats rerunning the job. A rerun throws away the container pull
# and the cache restore that preceded the failure. It also teaches everyone to
# ignore red, because the job goes green often enough.
set -u

# One switch per job. Without the flag a later apt call repeats it and reports
# it again as though it were news.
FLAG=/var/tmp/.apt-fell-back

# Undo what the workflow's mirror step did. The two file shapes match it: the
# legacy one-line sources.list and the newer deb822 *.sources.
fallback_mirror() {
    echo "apt-retry: switching to the generic Ubuntu mirror" >&2
    if [ -f /etc/apt/sources.list ]; then
        sed -i 's|http://azure\.archive\.ubuntu\.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' \
            /etc/apt/sources.list
    fi
    # Guard with `ls`: with nullglob off a non-matching glob expands to the
    # literal pattern, so do not rely on the match itself.
    if ls /etc/apt/sources.list.d/*.sources > /dev/null 2>&1; then
        sed -i 's|http://azure\.archive\.ubuntu\.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' \
            /etc/apt/sources.list.d/*.sources
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
    # Best effort. The caller's own command fails on its own terms if the index
    # is unusable, and that failure is the more informative one.
    apt-get update > /dev/null 2>&1 || true
fi

attempt "$@" && exit 0
echo "apt-retry: '$*' failed on both mirrors" >&2
exit 1
