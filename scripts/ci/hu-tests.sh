#!/usr/bin/env bash
set -e
cargo build -p hiroz-union -p hiroz-meter --release -j4
cargo test -p hiroz-tests --test hu_meter --features hu-meter-tests,jazzy --release -j4 2>&1
