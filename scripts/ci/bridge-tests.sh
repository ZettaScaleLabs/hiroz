#!/usr/bin/env bash
set -e
cargo build -p hiroz-bridge --no-default-features --features cross-distro -j4
cargo test -p hiroz-tests --features bridge-interop-tests,jazzy -- --test-threads=1
