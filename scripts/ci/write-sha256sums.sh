#!/bin/sh
# Write SHA256SUMS over every file in a directory, then verify it.
#
#   scripts/ci/write-sha256sums.sh dist
#
# One implementation, because there were two and they drifted: the release job
# used GNU-only `find -printf` and a bare `sha256sum`, so it would have failed
# on any host without coreutils.
#
# Two traps this exists to hold:
#   - The file list is captured BEFORE the redirect. A redirect creates its
#     target first, so `find ... > SHA256SUMS` lists SHA256SUMS itself with the
#     hash of an empty file, and `-c` then fails an otherwise perfect set.
#   - `shasum -a 256` emits and checks the same format as `sha256sum`, which
#     matters because install-hu.sh reads this file with the same fallback.
set -eu

DIR="${1:?usage: write-sha256sums.sh <dir>}"
[ -d "$DIR" ] || { echo "write-sha256sums: $DIR is not a directory" >&2; exit 1; }

if command -v sha256sum > /dev/null 2>&1; then
    SUM="sha256sum"
else
    SUM="shasum -a 256"
fi
echo "write-sha256sums: using $SUM in $DIR"

cd "$DIR"
rm -f SHA256SUMS
# `-exec basename` rather than `-printf`, which is GNU-only.
files=$(find . -maxdepth 1 -type f -exec basename {} \; | sort)
[ -n "$files" ] || { echo "write-sha256sums: $DIR holds no files" >&2; exit 1; }
printf '%s\n' "$files" | xargs $SUM > SHA256SUMS

echo "write-sha256sums: covered $(wc -l < SHA256SUMS) files"
cat SHA256SUMS
# Must verify clean, which also proves it does not list itself.
$SUM -c SHA256SUMS
