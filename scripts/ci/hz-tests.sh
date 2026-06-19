#!/usr/bin/env bash
set -e
cargo build -p hiroz-union -p hiroz-meter --release -j4
cargo test -p hiroz-tests --test hz_accuracy --features hz-comparison-tests,jazzy --release -- --nocapture
bash scripts/ci/cyclone-hz-check.sh
