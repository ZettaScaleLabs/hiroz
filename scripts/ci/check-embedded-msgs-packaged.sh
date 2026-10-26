#!/usr/bin/env bash
# Build hiroz from the packaged .crate archives, with no sibling source tree,
# and assert that EMBEDDED_MSGS is not empty.
#
# A crates.io build unpacks each crate into its own versioned directory, so
# `../hiroz-codegen` does not exist. This script reproduces that layout, then
# reads the table that hiroz/build.rs generated.
#
# Usage: scripts/ci/check-embedded-msgs-packaged.sh   (from the repository root)
#        scripts/ci/check-embedded-msgs-packaged.sh --self-test-layout
set -u

# The layout check, as a function so that it can be tested on a fake tree.
# Args: crates directory, version. Returns 0 when the layout is the crates.io
# one: hiroz-codegen exists only as hiroz-codegen-<version>, never unversioned.
check_layout() {
  local dir="$1" ver="$2"
  if [ ! -d "$dir/hiroz-codegen-$ver" ]; then
    echo "layout: $dir/hiroz-codegen-$ver is missing"; return 1
  fi
  if [ -e "$dir/hiroz-codegen" ]; then
    echo "layout: a sibling directory named hiroz-codegen exists"; return 1
  fi
  return 0
}

if [ "${1:-}" = "--self-test-layout" ]; then
  T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
  mkdir -p "$T/good/hiroz-codegen-1.2.3" "$T/bad/hiroz-codegen-1.2.3" "$T/bad/hiroz-codegen" "$T/empty"
  check_layout "$T/good" 1.2.3 >/dev/null || { echo "SELFTEST=FAIL (good layout rejected)"; exit 1; }
  check_layout "$T/bad" 1.2.3 >/dev/null && { echo "SELFTEST=FAIL (sibling not detected)"; exit 1; }
  check_layout "$T/empty" 1.2.3 >/dev/null && { echo "SELFTEST=FAIL (missing crate not detected)"; exit 1; }
  echo "SELFTEST=PASS"; exit 0
fi

ROOT="$(pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) CARGO_TARGET_DIR="$ROOT/$CARGO_TARGET_DIR"; export CARGO_TARGET_DIR ;;
esac

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/crates" "$WORK/consumer/src"
CRATES="hiroz-cdr hiroz-schema hiroz-codegen hiroz-derive hiroz-protocol hiroz"

echo "HEAD=$(git log --oneline -1)"
echo "VERSION=$VERSION"

PKG_ARGS=""
for name in $CRATES; do PKG_ARGS="$PKG_ARGS -p $name"; done
# shellcheck disable=SC2086
cargo package $PKG_ARGS --no-verify --allow-dirty > "$WORK/package.log" 2>&1
rc=$?
echo "package rc=$rc"
if [ "$rc" -ne 0 ]; then tail -30 "$WORK/package.log"; echo "RESULT=FAIL (package)"; exit 1; fi

n=0
for name in $CRATES; do
  c="$CARGO_TARGET_DIR/package/$name-$VERSION.crate"
  [ -f "$c" ] || { echo "RESULT=FAIL (missing $c)"; exit 1; }
  tar -xzf "$c" -C "$WORK/crates" || { echo "RESULT=FAIL (unpack $c)"; exit 1; }
  n=$((n + 1))
done
echo "unpacked $n crates"

# The packaged hiroz must have no sibling named hiroz-codegen.
if ! out="$(check_layout "$WORK/crates" "$VERSION")"; then
  echo "$out"; echo "RESULT=FAIL (layout)"; exit 1
fi

{
  echo '[package]'
  echo 'name = "consumer"'
  echo 'version = "0.0.0"'
  echo 'edition = "2021"'
  echo '[workspace]'
  echo '[dependencies]'
  echo "hiroz = \"=$VERSION\""
  echo '[patch.crates-io]'
  for d in "$WORK"/crates/*/; do
    name="$(basename "$d")"
    name="${name%-$VERSION}"
    echo "$name = { path = \"$d\" }"
  done
} > "$WORK/consumer/Cargo.toml"
echo 'fn main() {}' > "$WORK/consumer/src/main.rs"

# Isolated target dir, so a stale build script output cannot satisfy the check.
export CARGO_TARGET_DIR="$WORK/target"
(cd "$WORK/consumer" && cargo build -p hiroz -j4 > "$WORK/build.log" 2>&1)
rc=$?
echo "build rc=$rc"
if [ "$rc" -ne 0 ]; then tail -40 "$WORK/build.log"; echo "RESULT=FAIL (build)"; exit 1; fi

if grep -q 'embedded schema fallback will be empty' "$WORK/build.log"; then
  echo "build script warned that the embedded schema fallback is empty"
  EMPTY_WARNING=1
else
  EMPTY_WARNING=0
fi

TABLE="$(ls -t "$WORK"/target/debug/build/hiroz-*/out/embedded_msgs.rs 2>/dev/null | head -1)"
if [ -z "$TABLE" ]; then echo "RESULT=FAIL (embedded_msgs.rs not found: UNKNOWN)"; exit 1; fi
COUNT="$(grep -c 'include_str!' "$TABLE")"
echo "TABLE=$TABLE"
echo "EMBEDDED_MSGS entries=$COUNT"

if [ "$COUNT" -gt 0 ] && grep -q '"std_msgs/msg/String"' "$TABLE" && [ "$EMPTY_WARNING" -eq 0 ]; then
  echo "RESULT=PASS"
  exit 0
fi
echo "RESULT=FAIL (EMBEDDED_MSGS empty or incomplete)"
exit 1
