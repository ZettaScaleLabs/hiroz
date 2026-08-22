#!/bin/sh
# Install `hu` and its WASM plugins from a release.
#
#   curl -fsSL <base>/install-hu.sh | sh
#   ./install-hu.sh --offline ./downloaded-dir
#
# A token is needed only if the release host is private.
# When one is needed it is read from the environment only, and is NEVER
# embedded in this script. If you have no account, use --offline with files
# someone handed you — that path needs no network and no credentials.
#
# Environment:
#   HU_RELEASE_BASE   base URL of the release assets (overrides the default)
#   HU_VERSION        version to install (default: the latest published)
#   HU_RELEASE_TOKEN  API token, only needed if the release host is private
#   HU_PREFIX         install prefix (default: $HOME/.local)

set -eu

# Release attachments are served from
#   <host>/<owner>/<repo>/releases/download/<tag>/<file>
# and the tag for version X is vX. HU_RELEASE_BASE overrides the whole
# directory, which is what makes it possible to point at a smoke-test tag whose
# filenames carry a different version than its tag.
DEFAULT_HOST="https://github.com"
DEFAULT_REPO_PATH="ZettaScaleLabs/hiroz"
BASE="${HU_RELEASE_BASE:-}"
PREFIX="${HU_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
PLUGIN_DIR="$PREFIX/share/hu/plugins"
OFFLINE_DIR=""
TARGET=""
VERSION="${HU_VERSION:-}"

die() { printf 'install-hu: %s\n' "$*" >&2; exit 1; }
info() { printf 'install-hu: %s\n' "$*"; }

usage() {
    cat <<'EOF'
Usage: install-hu.sh [--offline DIR] [--version X.Y.Z] [--prefix DIR]

  --offline DIR   install from already-downloaded artifacts in DIR
                  (no network, no token required)
  --version       version to install
  --prefix        install prefix (default: $HOME/.local)
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --offline) OFFLINE_DIR="${2:-}"; [ -n "$OFFLINE_DIR" ] || die "--offline needs a directory"; shift 2 ;;
        --version) VERSION="${2:-}"; [ -n "$VERSION" ] || die "--version needs a value"; shift 2 ;;
        --prefix) PREFIX="${2:-}"; [ -n "$PREFIX" ] || die "--prefix needs a value"
                  BIN_DIR="$PREFIX/bin"; PLUGIN_DIR="$PREFIX/share/hu/plugins"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

# ------------------------------------------------------------ platform detect

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os/$arch" in
        Linux/x86_64)          echo "x86_64-unknown-linux-gnu" ;;
        Linux/aarch64|Linux/arm64) echo "aarch64-unknown-linux-gnu" ;;
        Darwin/arm64)          echo "aarch64-apple-darwin" ;;
        Darwin/x86_64)         die "macOS x86_64 is not published; build from source" ;;
        *)                     die "unsupported platform $os/$arch" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        die "need sha256sum or shasum to verify downloads; refusing to install unverified files"
    fi
}

# Verify FILE against the SHA256SUMS in DIR. A missing entry is a failure, not
# a pass — an unlisted file is exactly what a substituted file looks like.
verify() {
    _file="$1"; _sums="$2"
    _name="$(basename "$_file")"
    # Normalize the recorded name before comparing: `sha256sum ./*` writes
    # "./file", and binary mode writes "*file". Both are ordinary ways to
    # produce a SHA256SUMS, and neither should read as "no entry" — which is
    # a refusal, so being strict here fails closed on a valid download.
    _want="$(awk -v n="$_name" '
        { f = $2; sub(/^\.\//, "", f); sub(/^\*/, "", f); if (f == n) print $1 }
    ' "$_sums" | head -n1)"
    [ -n "$_want" ] || die "$_name has no entry in SHA256SUMS — refusing to install"
    _got="$(sha256_of "$_file")"
    if [ "$_want" != "$_got" ]; then
        die "checksum mismatch for $_name
  expected $_want
  got      $_got
This download is corrupt or has been altered. Nothing was installed."
    fi
}

# ------------------------------------------------------------------- download

# Public releases need no credential. A token is read only from the environment,
# never from a file: an installer that goes looking for credentials on disk is
# the wrong shape, and it embeds none of its own.
resolve_token() {
    if [ -n "${HU_RELEASE_TOKEN:-}" ]; then
        printf '%s' "$HU_RELEASE_TOKEN"
        return 0
    fi
    return 1
}

# Ask the release host which version is newest, so `curl ... | sh` works with
# no arguments. The header promised this default long before anything
# implemented it, and the installer died demanding HU_VERSION instead.
#
# /releases/latest excludes drafts and pre-releases, which is what we want: a
# bare install should never land on a rehearsal tag.
resolve_latest() {
    # Send the same credential as every other request. A private repository
    # answers /releases/latest with 401, so without this a no-argument install
    # fails here rather than at the asset it is authorised to fetch.
    if [ -n "${TOKEN:-}" ]; then
        set -- -H "Authorization: token $TOKEN"
    else
        set --
    fi
    curl -fsSL --connect-timeout 10 --max-time 60 "$@" \
        "https://api.github.com/repos/$DEFAULT_REPO_PATH/releases/latest" 2>/dev/null \
        | grep -m1 '"tag_name"' \
        | sed 's/.*"tag_name" *: *"//; s/".*//; s/^v//'
}

fetch() {
    _url="$1"; _dest="$2"
    # --fail so an HTTP error is an error: without it curl writes the 404 body
    # to disk and we would happily "install" an HTML page as a binary.
    # No `-k`. The release host presents a real, publicly-trusted certificate,
    # so verification succeeds normally — and skipping it here would undo the
    # point of the checksums below, since an attacker able to intercept the
    # download could serve their own SHA256SUMS alongside it.
    #
    # The Authorization header is sent only when a token was found. A public
    # release host needs none, and demanding one up front would make this
    # refuse to install from, say, a public GitHub release that anyone can
    # curl. Whether a credential is required is the *host's* business; this
    # only reports it if the download actually fails.
    if [ -n "$TOKEN" ]; then
        _ok=0
        curl -fsSL -H "Authorization: token $TOKEN" "$_url" -o "$_dest" || _ok=$?
    else
        _ok=0
        curl -fsSL "$_url" -o "$_dest" || _ok=$?
    fi
    if [ "$_ok" -ne 0 ]; then
        if [ -n "$TOKEN" ]; then
            die "failed to download $_url
If this is an auth failure, check that your token is valid for the release host."
        fi
        die "failed to download $_url
No credential was used. If the release host is private, set HU_RELEASE_TOKEN.
"
    fi
}

# --------------------------------------------------------------------- install

TMP=""
STAGE=""
# `return 0` is load-bearing: this runs as an EXIT trap, and under `set -e` a
# falsy last command here becomes the script's exit status. Without it an
# offline install (where TMP is empty) succeeded and still exited 1.
cleanup() {
    [ -n "$TMP" ] && rm -rf "$TMP"
    [ -n "$STAGE" ] && rm -rf "$STAGE"
    return 0
}
# EXIT only. A trap that returns 0 CONSUMES the signal, so sharing this handler
# with INT/TERM let a Ctrl-C between two commands delete the staging directories
# and leave the installer running against them. The signal traps therefore clean
# up and then exit, which re-runs cleanup through EXIT -- harmless, both removals
# are idempotent.
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

if [ -n "$OFFLINE_DIR" ]; then
    [ -d "$OFFLINE_DIR" ] || die "$OFFLINE_DIR is not a directory"
    SRC="$OFFLINE_DIR"
    info "offline install from $SRC"
else
    # A missing credential is not fatal here. Public release hosts need none,
    # and refusing up front would block installing from, say, a public GitHub
    # release. If the host does require one, `fetch` says so when the download
    # fails — which is also the only point at which we actually know.
    TOKEN="$(resolve_token || true)"

    TMP="$(mktemp -d)"
    SRC="$TMP"
    TARGET="$(detect_target)"

    if [ -z "$VERSION" ]; then
        # A caller who redirected the base has not told us WHICH release lives
        # there, and this project's API cannot answer for someone else's host.
        # Asking it anyway builds filenames from an unrelated version number
        # and 404s against a directory that was perfectly correct.
        [ -z "$BASE" ] || die "HU_RELEASE_BASE needs an explicit --version (or HU_VERSION):
  the newest-release lookup only speaks for this project's own releases."
        VERSION="$(resolve_latest || true)"
        [ -n "$VERSION" ] || die "could not determine the latest version from
  $DEFAULT_HOST/$DEFAULT_REPO_PATH
Pass one explicitly: --version X.Y.Z (or set HU_VERSION)."
        info "latest release is $VERSION"
    fi

    # A pre-release tag and its asset filenames do NOT carry the same version.
    # The tag is the full `v0.1.0-rc1`, but build-hu-release.nu names every
    # asset for the CORE version (`hu-0.1.0-...`), because an rc ships the same
    # crate as the release it rehearses. For a normal release the two strings
    # are identical, which is exactly why conflating them survived until the
    # first pre-release was cut and every file 404'd.
    CORE="${VERSION%%-*}"

    if [ -z "$BASE" ]; then
        BASE="$DEFAULT_HOST/$DEFAULT_REPO_PATH/releases/download/v$VERSION"
    fi

    info "downloading hu $VERSION for $TARGET"
    [ "$CORE" != "$VERSION" ] && info "  pre-release: assets are named for core version $CORE"
    info "  from $BASE"
    fetch "$BASE/SHA256SUMS" "$SRC/SHA256SUMS"
    fetch "$BASE/hu-$CORE-$TARGET.tar.gz" "$SRC/hu-$CORE-$TARGET.tar.gz"
    fetch "$BASE/hu-plugins-$CORE.tar.gz" "$SRC/hu-plugins-$CORE.tar.gz"
fi

[ -f "$SRC/SHA256SUMS" ] || die "SHA256SUMS not found in $SRC — refusing to install unverified files"

# Find the artifacts present in SRC.
#
# The offline path never reached detect_target, so this used to be a bare
# `ls | head -n1` -- lexical order. A directory holding a whole release sorts
# aarch64-apple-darwin first, so an x86_64 Linux user who downloaded every
# asset (which docs/tools/hu-install.md invites: "at least SHA256SUMS and the
# binary tarball") installed the macOS binary. It checksum-verified and exited
# 0, because that tarball really is in SHA256SUMS; the failure surfaced later
# as `Exec format error` with nothing pointing back here.
[ -n "$TARGET" ] || TARGET="$(detect_target)"
BIN_TAR="$(ls "$SRC"/hu-*-"$TARGET".tar.gz 2>/dev/null | head -n1 || true)"
if [ -z "$BIN_TAR" ]; then
    # Name what was looked for and what is present: on the offline path the
    # user assembled this directory themselves, so the actionable fact is
    # which target is missing, not that "no tarball" was found.
    found="$(ls "$SRC"/hu-*-*.tar.gz 2>/dev/null | grep -v -- '-plugins-' | sed 's|.*/|  |' || true)"
    # A full `if`, not `[ -n "$found" ] && die`: under `set -e` a falsy AND-list
    # is the shape that made a successful offline install exit 1 once already.
    if [ -n "$found" ]; then
        die "no hu tarball for $TARGET in $SRC. Present:
$found"
    fi
fi
PLUGIN_TAR="$(ls "$SRC"/hu-plugins-*.tar.gz 2>/dev/null | head -n1 || true)"
[ -n "$BIN_TAR" ] || die "no hu binary tarball found in $SRC"

verify "$BIN_TAR" "$SRC/SHA256SUMS"
[ -n "$PLUGIN_TAR" ] && verify "$PLUGIN_TAR" "$SRC/SHA256SUMS"

STAGE="$(mktemp -d)"

tar -xzf "$BIN_TAR" -C "$STAGE"
[ -f "$STAGE/hu" ] || die "binary tarball did not contain hu"

mkdir -p "$BIN_DIR" "$PLUGIN_DIR"
install -m 755 "$STAGE/hu" "$BIN_DIR/hu"
info "installed $BIN_DIR/hu"

if [ -n "$PLUGIN_TAR" ]; then
    tar -xzf "$PLUGIN_TAR" -C "$STAGE"
    for w in "$STAGE"/*.wasm; do
        [ -f "$w" ] || continue
        install -m 644 "$w" "$PLUGIN_DIR/$(basename "$w")"
        info "installed plugin $(basename "$w")"
    done
else
    info "no plugins tarball found — 'hu meter' and 'hu monitor' will not be available"
fi

# ---------------------------------------------------------------- post-install

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) info "note: $BIN_DIR is not on your PATH; add it to use 'hu' directly" ;;
esac

info "done. Verify with:"
info "  $BIN_DIR/hu --version"
info "  $BIN_DIR/hu plugin list"
