#!/usr/bin/env bash
set -e
cargo build -p hiroz-union -p hiroz-meter --release -j4
cargo test -p hiroz-tests --features hz-comparison-tests,jazzy --release
