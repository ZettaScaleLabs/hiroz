#!/usr/bin/env bash
# Run hiroz-bridge integration tests on the bridge-interop-ci devshell.
#
# Cross-distro tests (Humble ↔ Jazzy) always run.
# Cross-DDS build is attempted; skipped cleanly if cyclors native lib is absent.
set -euo pipefail

echo "=== hu-bridge integration tests ==="
echo ""

# ── Cross-distro build ────────────────────────────────────────────────────────
echo "--- Build: cross-distro (no cyclors) ---"
RUSTFLAGS="" cargo build -p hiroz-bridge --no-default-features --features cross-distro --release -j4

# ── Cross-distro tests ────────────────────────────────────────────────────────
echo ""
echo "--- Tests: bridge_interop (Humble ↔ Jazzy) ---"
RUSTFLAGS="" cargo test -p hiroz-tests --test bridge_interop \
    --features bridge-interop-tests,jazzy --release -- --nocapture

# ── Cross-DDS build (optional) ───────────────────────────────────────────────
echo ""
echo "--- Build: cross-dds (requires cyclors / libddsc) ---"

# cyclors links against libddsc — check it's present.
CYCLONE_LIB=$(
    set +e
    IFS=: read -ra _dirs <<< "${LD_LIBRARY_PATH:-}"
    for _d in "${_dirs[@]:-}"; do
        [[ -z "$_d" ]] && continue
        for _f in "$_d"/libddsc*.so*; do
            [[ -f "$_f" ]] && echo "$_f" && exit 0
        done
    done
    exit 0
)

if [[ -z "$CYCLONE_LIB" ]]; then
    echo "libddsc not found in LD_LIBRARY_PATH — skipping cross-DDS build"
    echo ""
    echo "=== bridge tests done (cross-distro only) ==="
    exit 0
fi

echo "cyclors: available ($CYCLONE_LIB)"
RUSTFLAGS="" cargo build -p hiroz-bridge --no-default-features --features cross-dds --release -j4
echo "cross-DDS build: OK"

echo ""
echo "=== bridge tests done (cross-distro + cross-DDS build) ==="
