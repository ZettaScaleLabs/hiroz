#!/usr/bin/env bash
# Check that cliff.toml marks a breaking commit in both template loops.
# Usage: scripts/test-cliff-breaking.sh [CLIFF_TOML]
# Needs git-cliff on PATH, or GIT_CLIFF set to its path. Needs no token and no network.
# Failure lines carry the marker R1ASSERT and the test name.
set -u
t=cliff_marks_a_breaking_commit
here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cfg=${1:-$here/cliff.toml}
cliff=${GIT_CLIFF:-git-cliff}
command -v "$cliff" > /dev/null || { echo "R1ASSERT $t UNKNOWN: git-cliff not found"; exit 2; }
scratch=$(mktemp -d) || exit 2
trap 'rm -rf "$scratch"' EXIT
cd "$scratch" || exit 2
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
git init -q . || exit 2
for m in 'fix(a): ordinary scoped' 'fix: ordinary unscoped' 'fix(x)!: breaking scoped' 'fix!: breaking unscoped' 'chore!: breaking chore' 'ci(c)!: breaking ci' 'build!: breaking build' 'chore: plain chore'; do
  git commit -q --allow-empty -m "$m" || exit 2
done
git commit -q --allow-empty -m 'fix(f): footer break' -m 'BREAKING CHANGE: x' || exit 2
out=$("$cliff" --config "$cfg" --unreleased --offline 2>&1); rc=$?
echo "$out"
echo "CLIFF=$rc"
[ "$rc" = 0 ] || { echo "R1ASSERT $t UNKNOWN: git-cliff exited $rc"; exit 2; }
fail=0
line() { printf '%s\n' "$out" | grep -F -- "$1" | head -1; }
for want in 'Breaking scoped' 'Breaking unscoped' 'Footer break' 'Breaking chore' 'Breaking ci' 'Breaking build'; do
  l=$(line "$want")
  [ -n "$l" ] || { echo "R1ASSERT $t UNKNOWN: no line for '$want'"; exit 2; }
  case "$l" in *BREAKING*) echo "MARK_OK $want" ;; *) echo "R1ASSERT $t FAIL: no breaking mark on '$want'"; fail=1 ;; esac
done
for ctl in 'Ordinary scoped' 'Ordinary unscoped'; do
  l=$(line "$ctl")
  [ -n "$l" ] || { echo "R1ASSERT $t UNKNOWN: no line for '$ctl'"; exit 2; }
  case "$l" in *BREAKING*) echo "R1ASSERT $t FAIL: mark on ordinary '$ctl'"; fail=1 ;; *) echo "CONTROL_OK $ctl" ;; esac
done
# the same verdict must hold for a relative config path and a relative GIT_CLIFF path
if [ -z "${R1_CLIFF_INNER:-}" ]; then
  rel=$(mktemp -d) || exit 2
  mkdir "$rel/bin" && cp "$cfg" "$rel/cliff.toml" && ln -s "$(command -v "$cliff")" "$rel/bin/git-cliff" || exit 2
  relout=$(cd "$rel" && R1_CLIFF_INNER=1 GIT_CLIFF=./bin/git-cliff bash "$here/scripts/test-cliff-breaking.sh" cliff.toml 2>&1); relrc=$?
  rm -rf "$rel"
  if [ "$relrc" = 0 ]; then echo "CONTROL_OK relative paths"; else
    printf "%s\n" "$relout" | grep -v "^TEST=" | tail -5
    echo "R1ASSERT $t FAIL: relative config or GIT_CLIFF path gives rc=$relrc"; fail=1
  fi
fi
# a skipped type stays skipped unless the commit is breaking
if printf '%s\n' "$out" | grep -qF 'Plain chore'; then echo "R1ASSERT $t FAIL: plain chore appears"; fail=1; else echo "CONTROL_OK Plain chore skipped"; fi
[ "$fail" = 0 ] && echo "TEST=0 $t" || echo "TEST=1 $t"
exit "$fail"
