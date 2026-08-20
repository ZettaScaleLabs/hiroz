#!/usr/bin/env bash
# Test scripts/install-hu.sh against a synthetic release.
#
# Deliberately synthetic: this exercises the installer's own logic — checksum
# enforcement, refusal paths, exit status — and needs no cargo build, so it
# runs in seconds and can gate every PR. The real artifacts are covered
# end-to-end elsewhere.
#
# Every case here is a *refusal* except the first. An installer is only as
# good as what it declines to install, and a refusal path that has never been
# exercised is unverified, not safe.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER="$ROOT/scripts/install-hu.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0

check() {
    _name="$1"; _want="$2"; _got="$3"
    if [ "$_want" = "$_got" ]; then
        echo "ok   — $_name"
        pass=$((pass + 1))
    else
        echo "FAIL — $_name (wanted rc=$_want, got rc=$_got)"
        fail=$((fail + 1))
    fi
}

# Build a synthetic release. Second argument is the version, so lifecycle
# tests can install one version over another.
make_dist() {
    _d="$1"; _v="${2:-0.1.0}"
    mkdir -p "$_d/s" "$_d/p"
    printf '#!/bin/sh\necho "hu %s"\n' "$_v" > "$_d/s/hu"
    chmod +x "$_d/s/hu"
    echo LICENSE > "$_d/s/LICENSE"
    echo README > "$_d/s/README-install.md"
    # Unversioned inside the tarball, matching what build-hu-release.nu
    # stages — the filename is what discovery derives the subcommand from, so
    # a versioned name here would give `hu meter-0_1_0`.
    printf 'meter %s'   "$_v" > "$_d/p/hu_meter.wasm"
    printf 'monitor %s' "$_v" > "$_d/p/hu_monitor.wasm"
    tar -czf "$_d/hu-$_v-x86_64-unknown-linux-gnu.tar.gz" -C "$_d/s" hu LICENSE README-install.md
    tar -czf "$_d/hu-plugins-$_v.tar.gz" -C "$_d/p" hu_meter.wasm hu_monitor.wasm
    rm -rf "$_d/s" "$_d/p"
    (cd "$_d" && sha256sum ./*.tar.gz > SHA256SUMS)
}

run_install() {
    _home="$1"; shift
    mkdir -p "$_home"
    env -u HU_RELEASE_TOKEN HOME="$_home" HU_PREFIX="$_home/.local" \
        sh "$INSTALLER" "$@" > "$WORK/out.txt" 2>&1
    echo $?
}

# Same, but with a credential present. `run_install` strips HU_RELEASE_TOKEN so
# the no-credential paths are honest; this variant exists for the cases that
# are specifically about behaviour when a token IS set.
run_install_with_token() {
    _home="$1"; _tok="$2"; shift 2
    mkdir -p "$_home"
    env HU_RELEASE_TOKEN="$_tok" HOME="$_home" HU_PREFIX="$_home/.local" \
        sh "$INSTALLER" "$@" > "$WORK/out.txt" 2>&1
    echo $?
}

# 1. Happy path. This one caught a real bug: the EXIT trap's last command was
#    a falsy test when TMP was empty, so a successful offline install exited 1.
make_dist "$WORK/d1"
rc=$(run_install "$WORK/h1" --offline "$WORK/d1")
check "offline install succeeds" 0 "$rc"
[ -x "$WORK/h1/.local/bin/hu" ] \
    && check "hu binary installed" 0 0 \
    || check "hu binary installed" 0 1
[ -f "$WORK/h1/.local/share/hu/plugins/hu_meter.wasm" ] \
    && check "meter plugin installed" 0 0 \
    || check "meter plugin installed" 0 1

# 2. One corrupted byte must be refused, and nothing may be left behind.
make_dist "$WORK/d2"
printf 'X' | dd of="$WORK/d2/hu-0.1.0-x86_64-unknown-linux-gnu.tar.gz" \
    bs=1 seek=100 conv=notrunc status=none
rc=$(run_install "$WORK/h2" --offline "$WORK/d2")
check "corrupted tarball refused" 1 "$rc"
[ -e "$WORK/h2/.local/bin/hu" ] \
    && check "nothing installed after refusal" 0 1 \
    || check "nothing installed after refusal" 0 0

# 3. A file absent from SHA256SUMS is what a substituted file looks like.
make_dist "$WORK/d3"
grep -v 'hu-0.1.0-x86_64' "$WORK/d3/SHA256SUMS" > "$WORK/d3/t" && mv "$WORK/d3/t" "$WORK/d3/SHA256SUMS"
rc=$(run_install "$WORK/h3" --offline "$WORK/d3")
check "unlisted file refused" 1 "$rc"

# 4. No checksum file at all.
make_dist "$WORK/d4"
rm "$WORK/d4/SHA256SUMS"
rc=$(run_install "$WORK/h4" --offline "$WORK/d4")
check "missing SHA256SUMS refused" 1 "$rc"

# 5. Network path against an unreachable host must fail loudly, and when no
#    credential was used the message must say so — a private host is the most
#    likely reason. It must NOT refuse before trying: a public release host
#    needs no token, and demanding one up front would block installing from,
#    say, a public GitHub release.
UNREACHABLE="https://127.0.0.1:1/nope"

rc=$(HU_RELEASE_BASE="$UNREACHABLE" run_install "$WORK/h5" --version 0.1.0)
check "unreachable host fails" 1 "$rc"
grep -q "failed to download" "$WORK/out.txt" \
    && check "failure names the URL it could not fetch" 0 0 \
    || check "failure names the URL it could not fetch" 0 1
grep -q "HU_RELEASE_TOKEN" "$WORK/out.txt" \
    && check "no-credential hint mentions HU_RELEASE_TOKEN" 0 0 \
    || check "no-credential hint mentions HU_RELEASE_TOKEN" 0 1
[ -e "$WORK/h5/.local/bin/hu" ] \
    && check "nothing installed on download failure" 0 1 \
    || check "nothing installed on download failure" 0 0

# The hint must be conditional: with a token set, the message should be about
# the token possibly being wrong, not about there being none.
rc=$(HU_RELEASE_BASE="$UNREACHABLE" run_install_with_token "$WORK/h5b" dummy --version 0.1.0)
check "unreachable host fails with a token too" 1 "$rc"
grep -q "token is valid" "$WORK/out.txt" \
    && check "with a token, the message is about validity" 0 0 \
    || check "with a token, the message is about validity" 0 1

# ---------------------------------------------------------------- lifecycle
#
# Everything above installs into a fresh HOME. Real users do not: they install
# over an existing install, upgrade, and eventually remove. None of that had
# ever run.

# 6. Reinstalling the same version is idempotent — no duplicate plugins, and
#    the install still works afterwards.
make_dist "$WORK/d6"
H6="$WORK/h6"
rc=$(run_install "$H6" --offline "$WORK/d6"); check "first install" 0 "$rc"
rc=$(run_install "$H6" --offline "$WORK/d6"); check "reinstall succeeds" 0 "$rc"
n=$(find "$H6/.local/share/hu/plugins" -name '*.wasm' | wc -l)
[ "$n" -eq 2 ] \
    && check "reinstall leaves exactly 2 plugins" 0 0 \
    || { echo "  (found $n)"; check "reinstall leaves exactly 2 plugins" 0 1; }

# 7. Upgrading replaces the binary and the plugins rather than accumulating.
#    The plugin filenames are unversioned by design, so a new version must
#    overwrite; if a release ever shipped versioned names inside the tarball,
#    installs would silently pile up and every one but the newest would be
#    dead weight that discovery still sees.
make_dist "$WORK/d7" 0.2.0
rc=$(run_install "$H6" --offline "$WORK/d7"); check "upgrade over an install" 0 "$rc"
got=$("$H6/.local/bin/hu")
[ "$got" = "hu 0.2.0" ] \
    && check "upgrade replaced the binary" 0 0 \
    || { echo "  (binary says '$got')"; check "upgrade replaced the binary" 0 1; }
n=$(find "$H6/.local/share/hu/plugins" -name '*.wasm' | wc -l)
[ "$n" -eq 2 ] \
    && check "upgrade left no stale plugins" 0 0 \
    || { echo "  (found $n)"; check "upgrade left no stale plugins" 0 1; }
grep -q "0.2.0" "$H6/.local/share/hu/plugins/hu_meter.wasm" \
    && check "upgrade replaced the plugin contents" 0 0 \
    || check "upgrade replaced the plugin contents" 0 1

# 8. docs/tools/hu-install.md tells the reader that removing two paths
#    uninstalls hu. That is a promise: nothing may be written outside them,
#    or the documented uninstall silently leaves things behind.
stray=$(find "$H6" -type f \
    ! -path "$H6/.local/bin/hu" \
    ! -path "$H6/.local/share/hu/*" | head -5)
[ -z "$stray" ] \
    && check "install writes only where the docs say" 0 0 \
    || { echo "  (stray: $stray)"; check "install writes only where the docs say" 0 1; }

# 9. And the documented uninstall really does leave nothing.
rm -f "$H6/.local/bin/hu"
rm -rf "$H6/.local/share/hu"
left=$(find "$H6" -type f | head -5)
[ -z "$left" ] \
    && check "documented uninstall removes everything" 0 0 \
    || { echo "  (left: $left)"; check "documented uninstall removes everything" 0 1; }

# 10. The documented entry point is `curl ... | sh`, and until now every test
#     invoked the script as a file. Piping is not the same execution: the
#     script arrives on stdin, so `$0` is not a path, and anything that reads
#     stdin consumes the un-run remainder of itself. Feed it the way the docs
#     tell a user to.
H7="$WORK/h7"
mkdir -p "$H7"
make_dist "$WORK/d7"
if cat "$INSTALLER" | env -u HU_RELEASE_TOKEN HOME="$H7" HU_PREFIX="$H7/.local" \
        sh -s -- --offline "$WORK/d7" > "$WORK/pipe.txt" 2>&1; then
    check "installs when piped into sh, as the docs instruct" 0 0
else
    echo "  (output: $(tail -3 "$WORK/pipe.txt" | tr '\n' '|'))"
    check "installs when piped into sh, as the docs instruct" 0 1
fi
[ -x "$H7/.local/bin/hu" ] \
    && check "the piped install produced an executable hu" 0 0 \
    || check "the piped install produced an executable hu" 0 1

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
