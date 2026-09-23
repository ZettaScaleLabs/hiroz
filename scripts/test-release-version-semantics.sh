#!/bin/sh
# Release version semantics: a tag is THREE different strings, and every
# release channel has to use the right one at each site.
#
#   release identity   what the tag/release/download path is called   0.2.0-rc1
#   asset core         what the FILENAMES carry                       0.2.0
#   binary version     what `hu --version` prints                     hu 0.2.0
#
# They coincide for a normal release and diverge for a pre-release, because
# `build-hu-release.nu` names every asset for the crate version: an rc ships the
# same crate as the release it rehearses. That divergence is why a pre-release
# is the only tag shape that can catch this class of defect — and, since
# `push: tags: v*` is the GitHub release workflow's only trigger, an rc tag is
# also its only rehearsal vehicle.
#
# This test needs no runner, no network and no build. It runs in seconds and is
# the check that would have caught F13 on either channel.
#
#   ./scripts/test-release-version-semantics.sh

set -u

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
GH="$ROOT/.github/workflows/release.yml"
INSTALLER="$ROOT/scripts/install-hu.sh"

PASS=0
FAIL=0

ok()   { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad()  { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$1"; }

check() { # description, expected, actual
    if [ "$2" = "$3" ]; then ok "$1"; else
        bad "$1"
        printf '         expected: %s\n         actual:   %s\n' "$2" "$3"
    fi
}

# grep_must / grep_must_not take a file, a description and a BRE. `--` guards
# patterns that begin with a dash (`--version`, `--no-validate`).
grep_must() {
    if grep -q -e "$3" -- "$1"; then ok "$2"; else bad "$2 (pattern absent: $3)"; fi
}
grep_must_not() {
    if grep -q -e "$3" -- "$1"; then bad "$2 (pattern present: $3)"; else ok "$2"; fi
}

# ---------------------------------------------------------------------------
# 1. The semantics themselves, table-driven.
#
# `strip` mirrors the workflows: the tag prefix comes off, then the release
# identity is split at the first '-' to get the asset core.
# ---------------------------------------------------------------------------

release_id() { # tag, prefix
    printf '%s' "${1#"$2"}"
}
asset_core() { # release identity
    printf '%s' "${1%%-*}"
}
is_prerelease() { # release identity
    # Semver: everything after the first `-` is the pre-release identifier.
    # This must stay identical to the expression release.yml uses, or the
    # table below asserts a rule the workflow does not implement.
    case "$1" in
        *-*) printf 'true' ;;
        *)   printf 'false' ;;
    esac
}

# tag | prefix | expected release id | expected asset core | expected hu --version | expected prerelease
TABLE='
v0.2.0|v|0.2.0|0.2.0|hu 0.2.0|false
v0.2.0-rc1|v|0.2.0-rc1|0.2.0|hu 0.2.0|true
v0.2.0-beta2|v|0.2.0-beta2|0.2.0|hu 0.2.0|true
hu-v0.1.0|hu-v|0.1.0|0.1.0|hu 0.1.0|false
hu-v0.1.0-rc1|hu-v|0.1.0-rc1|0.1.0|hu 0.1.0|true
'

echo "== version semantics per tag =="
printf '%s\n' "$TABLE" | while IFS='|' read -r tag prefix want_id want_core want_ver want_pre; do
    [ -n "$tag" ] || continue
    got_id=$(release_id "$tag" "$prefix")
    got_core=$(asset_core "$got_id")
    got_pre=$(is_prerelease "$got_id")
    printf '%s -> id=%s core=%s pre=%s\n' "$tag" "$got_id" "$got_core" "$got_pre"
    [ "$got_id" = "$want_id" ]        || { echo "  FAIL release identity"; exit 1; }
    [ "$got_core" = "$want_core" ]    || { echo "  FAIL asset core"; exit 1; }
    [ "hu $got_core" = "$want_ver" ]  || { echo "  FAIL binary version"; exit 1; }
    [ "$got_pre" = "$want_pre" ]      || { echo "  FAIL prerelease flag"; exit 1; }
done
if [ $? -eq 0 ]; then
    ok "table rows derive id/core/version/prerelease as documented"
else
    bad "a table row derived the wrong id/core/version/prerelease (see above)"
fi

echo
echo "== the two tag shapes differ in exactly the documented way =="
FINAL_ID=$(release_id v0.2.0 v);      FINAL_CORE=$(asset_core "$FINAL_ID")
RC_ID=$(release_id v0.2.0-rc1 v);     RC_CORE=$(asset_core "$RC_ID")

check "a normal tag: identity equals core" "$FINAL_ID" "$FINAL_CORE"
if [ "$RC_ID" = "$RC_CORE" ]; then
    bad "a pre-release tag: identity must NOT equal core"
else
    ok "a pre-release tag: identity ($RC_ID) differs from core ($RC_CORE)"
fi
check "both tags name the same assets" "$FINAL_CORE" "$RC_CORE"
check "both tags produce the same binary version" "hu $FINAL_CORE" "hu $RC_CORE"

echo
echo "== the asset set an rc tag actually produces =="
for f in "hu-$RC_CORE-x86_64-unknown-linux-gnu.tar.gz" \
         "hu_meter-$RC_CORE.wasm" \
         "hu_monitor-$RC_CORE.wasm" \
         "hu-plugins-$RC_CORE.tar.gz" \
         "hu-plugins-$RC_CORE.json"; do
    case "$f" in
        *"$RC_ID"*) bad "asset $f must not carry the pre-release suffix" ;;
        *)          ok "asset named for the core version: $f" ;;
    esac
done

# ---------------------------------------------------------------------------
# 2. The workflows must USE those two strings at the right sites. Part 1 alone
#    would stay green if a workflow reverted to one variable for both, so these
#    contract checks are what make this a detector rather than a self-test.
# ---------------------------------------------------------------------------

echo
echo "== .github/workflows/release.yml =="
grep_must     "$GH" "defines HU_CORE alongside HU_VERSION" 'HU_CORE='
grep_must     "$GH" "plugin verify loop checks hu_meter-\$HU_CORE" 'hu_meter-\$HU_CORE\.wasm'
grep_must_not "$GH" "plugin verify loop never checks a HU_VERSION-named asset" 'hu_[a-z]*-\$HU_VERSION'
grep_must_not "$GH" "plugins tarball/index never checked under HU_VERSION" 'hu-plugins-\$HU_VERSION'
grep_must     "$GH" "release-install asserts the CORE version" 'test "\$got" = "hu \$CORE"'
grep_must_not "$GH" "release-install does not assert the tag version" 'test "\$got" = "hu \$VER"'
grep_must     "$GH" "binary packaging arms the tag-vs-crate guard" '--version "\$HU_VERSION"'
grep_must     "$GH" "plugin build arms the tag-vs-crate guard" 'plugins-only --version "\$HU_VERSION"'
grep_must_not "$GH" "plugin build no longer skips component validation" '--no-validate'
grep_must     "$GH" "a host-native hu is built so validation can run" 'cargo build --release --bin hu --package hiroz-union'
grep_must     "$GH" "the release step sets prerelease from the tag shape" 'prerelease: '
grep_must     "$GH" "publish-crates is guarded against pre-release tags" "if: \${{ !contains(github.ref_name, '-') }}"
grep_must     "$GH" "prerelease is derived from the semver hyphen, not an rc/alpha/beta list" "prerelease: \${{ contains(github.ref_name, '-') }}"

echo
echo "== scripts/install-hu.sh =="
grep_must     "$INSTALLER" "splits the requested version into a core version" 'CORE="\${VERSION%%-\*}"'
grep_must     "$INSTALLER" "downloads core-named assets" 'hu-\$CORE-\$TARGET\.tar\.gz'
# The offline path used to skip detect_target entirely and take `ls | head -n1`,
# so a directory holding a whole release installed the alphabetically-first
# target -- aarch64-apple-darwin -- on an x86_64 Linux host, with exit 0.
grep_must     "$INSTALLER" "the offline path selects the tarball by platform" 'hu-\*-"\$TARGET"\.tar\.gz'
grep_must_not "$INSTALLER" "no lexical tarball pick remains" 'ls "\$SRC"/hu-\*-\*\.tar\.gz 2>/dev/null | grep -v -- .-plugins-. | head -n1'

# ---------------------------------------------------------------------------
# 3. What the release PUBLISHES, and what it verifies afterwards.
#
#    Two defects live here and neither is a version-semantics bug, but they
#    share this file because they share its property: they are contract facts
#    about the release workflow that can be checked with no runner, no network
#    and no build.
#
#    D1 — docs/tools/hu-install.md's first instruction is
#         `curl -fsSL <base>/install-hu.sh | sh`, and install-hu.sh was never
#         published. build-hu-release.nu writes six assets; the installer is
#         not one of them, and the release job uploaded `dist/**`.
#
#    D7 — the post-publish check stopped at `hu plugin list | grep meter`.
#         Plugin discovery reads FILENAMES ONLY and never opens the component,
#         so an empty file named hu_meter.wasm passed every assertion this
#         channel made.
# ---------------------------------------------------------------------------

# Line number of the first line matching a BRE, or 0. Used for the ordering
# assertion below: staging AFTER the checksums are assembled would publish an
# unlisted asset, and no grep for either line alone can see that.
line_of() { # file, BRE
    awk -v pat="$2" 'index($0, pat) { print NR; exit }' "$1" 2>/dev/null || true
}

echo
echo "== D1: the documented installer URL is actually published =="
grep_must "$GH" "the release job stages install-hu.sh into dist/" \
    'cp scripts/install-hu.sh dist/install-hu.sh'
grep_must "$GH" "staging parse-checks the installer before publishing it" \
    'sh -n scripts/install-hu.sh'
grep_must "$GH" "SHA256SUMS coverage of install-hu.sh is asserted" \
    'install-hu\.sh is not covered by SHA256SUMS'
grep_must "$GH" "the published installer URL is fetched back and compared" \
    'curl -fsSL "\$HU_RELEASE_BASE/install-hu.sh"'
grep_must "$GH" "the fetched installer is diffed against the tagged source" \
    'diff -u scripts/install-hu.sh fetched-install-hu.sh'

_stage=$(line_of "$GH" 'cp scripts/install-hu.sh dist/install-hu.sh')
_sums=$(line_of "$GH" 'Assemble SHA256SUMS over the complete asset set')
if [ -n "${_stage:-}" ] && [ -n "${_sums:-}" ] && [ "$_stage" -lt "$_sums" ] 2>/dev/null; then
    ok "install-hu.sh is staged BEFORE SHA256SUMS is assembled (line $_stage < $_sums)"
else
    bad "install-hu.sh must be staged before SHA256SUMS (stage=${_stage:-none} sums=${_sums:-none})"
fi

# The asset this publishes must parse. Pure syntax check — nothing is executed.
if sh -n "$INSTALLER" 2>/dev/null; then
    ok "scripts/install-hu.sh parses (sh -n)"
else
    bad "scripts/install-hu.sh does not parse (sh -n)"
fi

echo
echo "== D7: the release is checked against its own docs, not just filenames =="
grep_must "$GH" "runs the docs-reproduction suite after installing from the release" \
    'nu scripts/test-hu-docs-repro.nu'
grep_must "$GH" "the suite is pointed at the installed-from-release HOME" \
    '--home "\$HUHOME"'
grep_must "$GH" "--require-traffic, so an empty graph fails instead of passing" \
    '--require-traffic'
grep_must "$GH" "a traffic fixture is built from source, not taken from the artifact" \
    'cargo build --release --example z_pubsub -p hiroz'
grep_must "$GH" "a router is started from the INSTALLED binary" \
    '"\$HUHOME/.local/bin/hu" router'
grep_must "$GH" "the suite exit status is captured, not piped away" \
    'test "\$rc" -eq 0'


# ---------------------------------------------------------------------------
# 4. The derivation itself, by VALUE
#
# Everything above greps for variable NAMES. That is not enough, and this
# section exists because it was measured not to be: reintroducing F13 verbatim
# --- `HU_CORE=${V%%-*}` -> `HU_CORE=$V`, one variable for both strings --- left
# the suite at 42 passed, 0 failed. Every downstream grep still matched, because
# `hu_meter-$HU_CORE.wasm` is textually unchanged. A name-based check cannot see
# a changed value.
#
# So: lift the real derivation lines out of the workflow and RUN them, then
# assert on what they produce. This targets the TAG path only.

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Evaluate the Nth derivation block, 1-based. `release.yml` carries one per
# release producer -- `build-binaries` and `build-hu-plugins` today -- and they
# are independent copies. A `grep -m1` here validated only the first, so the
# second could regress to `HU_CORE=$V` and this test would stay green.
derive_nth() { # file, ref-var-name, ref-value, assignment-pattern, n
    _f="$1"; _var="$2"; _ref="$3"; _assign="$4"; _n="$5"
    {
        grep -- "$_assign" "$_f"          | sed -n "${_n}p"
        grep 'echo "HU_VERSION=' "$_f"    | sed -n "${_n}p"
        grep 'echo "HU_CORE=' "$_f"       | sed -n "${_n}p"
    } | sed 's/^[[:space:]]*//' > "$WORK/derive.sh"
    : > "$WORK/env"
    env "$_var=$_ref" GITHUB_ENV="$WORK/env" sh "$WORK/derive.sh" >/dev/null 2>&1 || true
    cat "$WORK/env"
}

# How many derivation blocks the file has. Asserted below, so a producer added
# without a matching check is a failure rather than a silent gap.
derivation_count() { # file, assignment-pattern
    grep -c -- "$2" "$1"
}

# Every derivation block must agree. A release whose binary leg and plugin leg
# disagree about the core version publishes assets that cannot install together.
check_derivation() { # label, file, ref-var, ref, assign-pattern, want_version, want_core
    _total="$(derivation_count "$2" "$5")"
    if [ "$_total" -lt 1 ]; then
        bad "$1 (no derivation block matched $5)"
        return
    fi
    _i=1
    while [ "$_i" -le "$_total" ]; do
        check_one_derivation "$1 [block $_i/$_total]" "$2" "$3" "$4" "$5" "$6" "$7" "$_i"
        _i=$((_i + 1))
    done
}

check_one_derivation() { # label, file, ref-var, ref, assign-pattern, want_version, want_core, n
    _out="$(derive_nth "$2" "$3" "$4" "$5" "$8")"
    _v="$(printf '%s\n' "$_out" | sed -n 's/^HU_VERSION=//p' | head -n1)"
    _c="$(printf '%s\n' "$_out" | sed -n 's/^HU_CORE=//p' | head -n1)"
    if [ "$_v" = "$6" ] && [ "$_c" = "$7" ]; then
        ok "$1 (HU_VERSION=$_v HU_CORE=$_c)"
    else
        bad "$1 — got HU_VERSION='$_v' HU_CORE='$_c', wanted '$6' / '$7'"
    fi
}

echo "== the workflows' own derivation, evaluated =="
check_derivation "GitHub: a pre-release tag splits release id from asset core" \
    "$GH" GITHUB_REF_NAME v0.2.0-rc1 'V="${GITHUB_REF_NAME#v}"' 0.2.0-rc1 0.2.0
check_derivation "GitHub: a normal tag makes the two identical" \
    "$GH" GITHUB_REF_NAME v0.2.0 'V="${GITHUB_REF_NAME#v}"' 0.2.0 0.2.0


echo
echo "== every workspace crate inherits one version =="
# `cargo publish --workspace` takes each crate's version from its own manifest,
# not from the tag. A crate carrying a literal version is one the release can
# silently leave behind: it either republishes a version crates.io already has
# (failing the job) or ships a number that disagrees with the tag. hiroz,
# hiroz-protocol and hiroz-union each carried one.
#
# The nested plugins workspace under crates/hiroz-union/plugins is `exclude`d
# and its versions are cosmetic -- release assets are named for the workspace
# version -- so it is deliberately not covered here.
#
# Read the `members` list rather than globbing `crates/*/Cargo.toml`. That glob
# is one level deep, so it never saw `crates/hiroz/examples/protobuf_demo` --
# a real workspace member, and the one member whose Cargo.lock entry the 0.2.0
# release left at 0.1.0. A check that cannot see a member cannot vouch for it.
members() { # print each member path listed in the root Cargo.toml
    awk '
        /^members[[:space:]]*=[[:space:]]*\[/ { inside = 1; next }
        inside && /^\]/                       { exit }
        inside {
            if (match($0, /"[^"]+"/))
                print substr($0, RSTART + 1, RLENGTH - 2)
        }
    ' "$ROOT/Cargo.toml"
}

MEMBERS=$(members)
MEMBER_COUNT=$(printf '%s\n' "$MEMBERS" | grep -c .)
# Guard the parser itself: an awk change that stops matching would leave every
# loop below iterating over nothing and reporting a clean pass.
if [ "$MEMBER_COUNT" -ge 10 ]; then
    ok "parsed $MEMBER_COUNT workspace members from Cargo.toml"
else
    bad "parsed only $MEMBER_COUNT workspace members -- the members parser is broken"
fi

stray=""
for m in $MEMBERS; do
    f="$ROOT/$m/Cargo.toml"
    [ -f "$f" ] || { bad "member $m has no Cargo.toml"; continue; }
    v=$(grep -m1 '^version' "$f" 2>/dev/null || true)
    case "$v" in
        *workspace*) ;;
        "")          ;;
        *)           stray="$stray $m" ;;
    esac
done
if [ -z "$stray" ]; then
    ok "no crate carries a literal version"
else
    bad "these crates carry a literal version instead of version.workspace:$stray"
fi

echo
echo "== Cargo.lock agrees with the workspace version =="
# The 0.2.0 release bumped 11 of the 12 members in Cargo.lock and left
# protobuf_demo at 0.1.0, because the lock was edited by pattern rather than
# regenerated. Nothing noticed: no job passes --locked, so cargo silently
# rewrote that line on every build and every run left the tree dirty.
#
# This compares the lock against the manifests directly, so it needs no cargo,
# no network and no build -- the same properties as the rest of this suite.
WS_VERSION=$(awk '
    /^\[workspace\.package\]/ { inside = 1; next }
    inside && /^\[/           { exit }
    inside && /^version[[:space:]]*=/ {
        if (match($0, /"[^"]+"/)) { print substr($0, RSTART + 1, RLENGTH - 2); exit }
    }
' "$ROOT/Cargo.toml")
check "the workspace declares a version" 1 "$([ -n "$WS_VERSION" ] && echo 1 || echo 0)"

# The lock keys on the package name, which need not equal the directory name.
lock_version() { # package name
    awk -v want="$1" '
        $0 == "name = \"" want "\"" { found = 1; next }
        found && /^version[[:space:]]*=/ {
            if (match($0, /"[^"]+"/)) { print substr($0, RSTART + 1, RLENGTH - 2) }
            exit
        }
    ' "$ROOT/Cargo.lock"
}

lock_stray=""
lock_checked=0
for m in $MEMBERS; do
    f="$ROOT/$m/Cargo.toml"
    [ -f "$f" ] || continue
    name=$(awk '/^name[[:space:]]*=/ { if (match($0, /"[^"]+"/)) { print substr($0, RSTART + 1, RLENGTH - 2); exit } }' "$f")
    [ -n "$name" ] || { bad "member $m declares no package name"; continue; }
    got=$(lock_version "$name")
    lock_checked=$((lock_checked + 1))
    if [ -z "$got" ]; then
        lock_stray="$lock_stray $name(absent)"
    elif [ "$got" != "$WS_VERSION" ]; then
        lock_stray="$lock_stray $name($got)"
    fi
done
check "every member was looked up in Cargo.lock" "$MEMBER_COUNT" "$lock_checked"
if [ -z "$lock_stray" ]; then
    ok "all $lock_checked members are at $WS_VERSION in Cargo.lock"
else
    bad "Cargo.lock disagrees with the workspace version $WS_VERSION for:$lock_stray"
    printf '         run `cargo metadata --offline >/dev/null` and commit Cargo.lock\n'
fi

echo
echo "== the release is not public until it verifies =="
# Ordering, not just presence. This used to publish immediately and verify
# afterwards, so a failed check left a public release with nothing to de-list
# it. GitHub will not serve draft assets for a download test, so the reachable
# shape is publish -> verify -> withdraw on failure.
# Both the smoke test and the release job must checksum through the shared
# script. They were two inline copies until they drifted: the release job's used
# GNU-only `find -printf` and a bare `sha256sum`, so it would have failed on any
# host without coreutils. Nothing asserted the assembly existed at all.
SUMS_CALLS=$(grep -c 'write-sha256sums\.sh' "$GH")
check "both jobs checksum through the shared script" 2 "$SUMS_CALLS"
grep_must_not "$GH" "no inline checksum assembly remains" 'xargs sha256sum'

grep_must     "$GH" "the release is created as a draft" 'draft: true'
grep_must     "$GH" "a job promotes the draft" '\-\-draft=false'
grep_must     "$GH" "a job withdraws it again" '\-\-draft=true'
# Not a bare `failure()`. That is true when ANY ancestor fails, and the withdraw
# job's ancestors reach back to build-binaries -- so a failed build made it try
# to withdraw a release that was never created, going red under a caption that
# reads as a bad release left live. Pin the property, not the wording: it must
# withdraw only what publish-release actually published.
grep_must_not "$GH" "the withdraw job does not use a bare failure()" 'if: \${{ failure() }}'
grep_must     "$GH" "the withdraw job requires a successful publish" \
    "needs.publish-release.result == 'success'"
# `!= 'success'`, not `== 'failure'`. GitHub reports a cancelled job as
# `cancelled` and a skipped one as `skipped`, and promotion has already happened
# by then -- so equality on 'failure' leaves an unverified release public with
# the *Latest* badge, which is the answer install-hu.sh's resolve_latest gives.
grep_must     "$GH" "the withdraw job fires on any non-success download test" \
    "needs.smoke-test-release-install.result != 'success'"
grep_must_not "$GH" "the withdraw job does not test equality with failure" \
    "needs.smoke-test-release-install.result == 'failure'"
grep_must     "$GH" "the download test waits for the promotion" 'needs: \[publish-release\]'
grep_must     "$GH" "crates.io is gated behind the download test" 'needs: \[smoke-test-release-install\]'

# The rehearsal script must give its tag a message. A bare `git tag` produces a
# SIGNED tag for anyone with `tag.gpgsign = true`, and a signed tag needs a
# message -- so the script died with `fatal: no tag message?` before pushing
# anything, on exactly the machines that cut releases. RELEASING.md names this
# script as the documented rehearsal, so that failure removed the only
# documented way to test a release without cutting one.
RW="scripts/test-release-workflow.nu"
grep_must     "$RW" "the rehearsal tag carries a message" '\^git tag -a -m'
grep_must_not "$RW" "the rehearsal does not create a bare tag" '\^git tag \$tag$'
echo
echo "-- $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
