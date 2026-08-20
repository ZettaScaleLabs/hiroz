#!/usr/bin/env bash
set -e

# Unify the target dir across the main workspace and the separate plugins
# workspace. Must be absolute: hiroz-tests binaries run with CWD at the crate
# root, so a relative path in the PATH entry below would misresolve.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
# Normalize a caller-provided relative CARGO_TARGET_DIR to absolute.
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) CARGO_TARGET_DIR="$(pwd)/$CARGO_TARGET_DIR"; export CARGO_TARGET_DIR ;;
esac

# Build hu (the plugin host) and the WASM plugins. Plugins are excluded from
# the workspace, so build each via --manifest-path. Debug, not --release: cost
# is dominated by the serial test run below, so release compile time is pure
# overhead.
cargo build -p hiroz-union
PLUGIN_DIR="crates/hiroz-union/plugins"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-meter/Cargo.toml"
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-monitor/Cargo.toml"
# Build hu-plugin-template too so the suite loads it through the real host
# (`hu plugin validate`/`list`), catching WIT/ABI regressions a compile-only
# check would miss.
cargo build --target wasm32-wasip2 --manifest-path "${PLUGIN_DIR}/hu-plugin-template/Cargo.toml"

# Expose the WASM output directory as HU_PLUGIN_PATH so that `hu meter` /
# `hu monitor` can load the compiled plugins during tests.
TARGET_DIR="${CARGO_TARGET_DIR}"
export HU_PLUGIN_PATH="${TARGET_DIR}/wasm32-wasip2/debug"

# Tests spawn `hu` by name (Command::new("hu")) — put the built binary on PATH.
export PATH="${TARGET_DIR}/debug:${PATH}"

# NOTE: HIROZ_MSG_PATH is deliberately NOT set here. `hu meter pub` can resolve
# a schema either from `.msg` files on disk (via HIROZ_MSG_PATH) or by live
# discovery from a node on the topic. The disk path is covered by
# `test_hu_meter_pub_empty_topic_from_disk`, which sets HIROZ_MSG_PATH itself on
# the `hu` subprocess; leaving it unset for the rest of the suite keeps the
# live-discovery fallback (`test_hu_meter_pub_publishes_to_live_topic`) honest.

# Run single-threaded: each test spawns a router + hiroz node + `hu` subprocess
# (JIT wasmtime), and two at once on the 2-core runner over-subscribe the cores
# and starve graph discovery into a timeout. Wall-clock ≈ parallel here anyway.
cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy -- --test-threads=1
cargo test -p hiroz-tests --test hu_monitor --features hu-monitor-tests,jazzy -- --test-threads=1

# ---------------------------------------------------------------------------
# `hu plugin install` SUCCESS paths.
#
# crates/hiroz-union/tests/plugin_install.rs covers the refusals (404, checksum
# mismatch, non-component bytes, a WIT world this hu does not host, an unknown
# name, no registry). It cannot cover the success paths: a real WASM component
# is needed to serve, and that crate cannot build one -- the plugins are a
# separate, excluded, wasm32-wasip2 workspace. Its module doc claimed this
# script covered them instead. It did not; nothing did. This is that coverage,
# placed here because this is where genuine plugins exist.
#
# Local loopback only, no network: python3 -m http.server over a fixture dir.
echo "== hu plugin install: success paths =="
PI_TMP="$(mktemp -d)"
trap 'rm -rf "$PI_TMP"; [ -n "${PI_SRV:-}" ] && kill "$PI_SRV" 2>/dev/null || true' EXIT

WASM="${HU_PLUGIN_PATH}/hu_meter.wasm"
[ -f "$WASM" ] || { echo "FAIL: no built plugin at $WASM"; exit 1; }

mkdir -p "$PI_TMP/srv" "$PI_TMP/home"
cp "$WASM" "$PI_TMP/srv/hu_meter-9.9.9.wasm"
# A `.sha256` sidecar is the checksum source the docs describe for a URL
# install, so serve one and prove it is honoured rather than ignored.
( cd "$PI_TMP/srv" && sha256sum hu_meter-9.9.9.wasm | cut -d' ' -f1 > hu_meter-9.9.9.wasm.sha256 )
# The index must declare the world this hu hosts, or install refuses -- which
# is the guard plugin_install.rs already proves fires.
HOST_WORLD="$(grep -o 'hu:plugin@[0-9.]*' crates/hiroz-union/src/plugin/install.rs | head -1)"
cat > "$PI_TMP/srv/hu-plugins-9.9.9.json" <<JSON
{"schema":1,"hu_version":"9.9.9","wit_world":"${HOST_WORLD}",
 "plugins":[{"name":"meter","file":"hu_meter-9.9.9.wasm","version":"9.9.9",
 "sha256":"$(cat "$PI_TMP/srv/hu_meter-9.9.9.wasm.sha256")",
 "world":"hu-cli-plugin","description":"fixture"}]}
JSON

( cd "$PI_TMP/srv" && python3 -m http.server 8791 --bind 127.0.0.1 >/dev/null 2>&1 ) &
PI_SRV=$!
sleep 2
curl -fsS -o /dev/null "http://127.0.0.1:8791/hu_meter-9.9.9.wasm" \
  || { echo "FAIL: fixture server did not serve the plugin"; exit 1; }

# Only `.wasm` files. `installed.json` -- the install database -- lives in this
# same directory by design (install.rs db_path), so listing everything makes
# "nothing left after uninstall" impossible to satisfy.
pi_installed() { ls "$PI_TMP/home/.local/share/hu/plugins"/*.wasm 2>/dev/null | xargs -r -n1 basename | tr "\n" " "; }

# Every failure here costs a full CI round trip to observe -- there is no hu
# binary on a dev box to reproduce it. So each one prints the whole state at
# once rather than the single assertion that tripped.
pi_dump() {
  echo "  --- state ---"
  echo "  plugins dir: $(ls -la "$PI_TMP/home/.local/share/hu/plugins" 2>&1 | tr "\n" "|")"
  echo "  plugin list: $(env -u HU_PLUGIN_PATH HOME="$PI_TMP/home" hu plugin list 2>&1 | tr "\n" "|")"
  echo "  served:      $(ls "$PI_TMP/srv" 2>&1 | tr "\n" " ")"
}

# 1. Install by URL. The sidecar checksum must be used, not skipped.
env -u HU_PLUGIN_PATH -u HU_PLUGIN_REGISTRY HOME="$PI_TMP/home" \
  hu plugin install "http://127.0.0.1:8791/hu_meter-9.9.9.wasm" \
  || { echo "FAIL: install by URL"; pi_dump; exit 1; }
case "$(pi_installed)" in
  *hu_meter*) echo "  ok  installed by URL: $(pi_installed)" ;;
  *) echo "FAIL: URL install left nothing: '$(pi_installed)'"; pi_dump; exit 1 ;;
esac

# 2. Install by registry NAME, resolved through the served index. Fresh HOME so
#    this proves the registry path, not a leftover from case 1.
rm -rf "$PI_TMP/home"; mkdir -p "$PI_TMP/home"
env -u HU_PLUGIN_PATH HOME="$PI_TMP/home" \
  HU_PLUGIN_REGISTRY="http://127.0.0.1:8791/hu-plugins-9.9.9.json" \
  hu plugin install meter \
  || { echo "FAIL: install by registry name"; pi_dump; exit 1; }
case "$(pi_installed)" in
  *hu_meter*|*meter*) echo "  ok  installed by name: $(pi_installed)" ;;
  *) echo "FAIL: registry install left nothing: '$(pi_installed)'"; pi_dump; exit 1 ;;
esac

# 3. The installed plugin must be listed AND loadable. `plugin list` reads filenames
#    and never opens a component, so listing it proves only that a file is there.
env -u HU_PLUGIN_PATH HOME="$PI_TMP/home" hu plugin list | grep -q meter \
  || { echo "FAIL: installed plugin not listed"; pi_dump; exit 1; }
#    Then OPEN what was installed. `hu meter` is the wrong probe: the host
#    connects to a Zenoh router before dispatching to a plugin, so with no
#    router it fails on the connection and never reaches the component --
#    which says nothing about the install. `plugin validate` returns early in
#    main() before any session exists, and loads the file as a component, so it
#    tests the installed bytes and needs no infrastructure.
PI_WASM="$PI_TMP/home/.local/share/hu/plugins/hu_meter.wasm"
PI_OUT="$(env -u HU_PLUGIN_PATH HOME="$PI_TMP/home" hu plugin validate "$PI_WASM" 2>&1)" || {
  echo "FAIL: installed plugin does not load as a component"
  echo "  output:   $PI_OUT"
  echo "  dir:      $(ls -la "$PI_TMP/home/.local/share/hu/plugins" 2>&1 | tr '\n' '|')"
  echo "  list:     $(env -u HU_PLUGIN_PATH HOME="$PI_TMP/home" hu plugin list 2>&1 | tr '\n' '|')"
  exit 1
}
echo "  ok  installed plugin loads as a component"

# 4. Uninstall round trip.
env -u HU_PLUGIN_PATH HOME="$PI_TMP/home" hu plugin uninstall meter \
  || { echo "FAIL: uninstall"; pi_dump; exit 1; }
[ -z "$(pi_installed)" ] || { echo "FAIL: uninstall left '$(pi_installed)'"; pi_dump; exit 1; }
echo "  ok  uninstall removed it"
