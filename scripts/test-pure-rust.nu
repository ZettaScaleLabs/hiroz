#!/usr/bin/env nu

# Pure Rust Test Suite - No ROS dependencies required
# This script tests hiroz in a pure Rust environment using bundled message definitions

use lib/common.nu *

# ============================================================================
# Test Functions
# ============================================================================

def clippy-workspace [] {
    log-step "Clippy (default workspace)"
    run-cmd "cargo clippy --all-targets -- -D warnings"
}

def run-tests [] {
    # Treat warnings as errors
    $env.RUSTFLAGS = "-D warnings"

    log-step "Run tests"
    run-cmd "cargo nextest run --no-fail-fast"
}

def check-bundled-msgs [] {
    log-step "Check hiroz-msgs with bundled messages"
    run-cmd "cargo check -p hiroz-msgs"
    run-cmd "cargo check -p hiroz-msgs --features bundled_msgs"
    run-cmd "cargo check -p hiroz-msgs --features common_interfaces"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features std_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features geometry_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features sensor_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features nav_msgs"
}

def check-console [] {
    log-step "Check hiroz-union plugins"
    run-cmd "cargo check -p hiroz-union -p hiroz-meter -p hiroz-monitor"
    run-cmd "cargo clippy -p hiroz-union -p hiroz-meter -p hiroz-monitor -- -D warnings"
}

def clippy-hiroz-py [] {
    log-step "Clippy (hiroz-py)"
    run-cmd "cargo clippy -p hiroz-py --all-targets -- -D warnings"
}

def check-examples [] {
    log-step "Check all examples (cargo check --examples)"
    run-cmd "cargo check --examples"
}

def check-distro-features [] {
    log-step "Check distro feature flags"
    run-cmd "cargo check -p hiroz --no-default-features --features humble"
    run-cmd "cargo check -p hiroz --no-default-features --features jazzy"
    run-cmd "cargo check -p hiroz --no-default-features --features rolling"
    run-cmd "cargo check -p hiroz --no-default-features --features kilted"
    run-cmd "cargo check -p hiroz --no-default-features --features lyrical"
}

def test-shm [] {
    log-step "Test SHM functionality"

    # Library unit tests (ShmConfig, ShmProviderBuilder)
    run-cmd "cargo test --package hiroz --lib shm"
    # Integration-style unit tests (pub/sub with SHM)
    run-cmd "cargo test --package hiroz --test shm"
    # Integration tests (validate shm_pointcloud2 example)
    run-cmd "cargo test --package hiroz-tests --test shm_example"
}

# ============================================================================
# Test Suite Configuration
# ============================================================================

def get-test-map [] {
    {
        clippy-workspace: { clippy-workspace }
        run-tests: { run-tests }
        check-bundled-msgs: { check-bundled-msgs }
        check-console: { check-console }
        check-examples: { check-examples }
        check-distro-features: { check-distro-features }
        clippy-hiroz-py: { clippy-hiroz-py }
        test-shm: { test-shm }
    }
}

def get-test-pipeline [] {
    [
        "clippy-workspace"
        "run-tests"
        "check-bundled-msgs"
        "check-console"
        "check-examples"
        "check-distro-features"
        "clippy-hiroz-py"
        "test-shm"
    ]
}

# ============================================================================
# Main Entry Point
# ============================================================================

# Run pure Rust test suite (no ROS dependencies)
#
# Examples:
#   ./test-pure-rust.nu                      # Run all tests
#   ./test-pure-rust.nu clippy-workspace     # Run specific test
#   ./test-pure-rust.nu --list               # List available test functions
def main [
    --list                # List available test functions
    ...tests: string      # Specific test functions to run (optional)
] {
    if $list {
        print "Available test functions:"
        get-test-pipeline | each { |name| print $"  - ($name)" }
        return
    }

    let test_map = get-test-map
    let pipeline = get-test-pipeline

    let tests_to_run = if ($tests | is-empty) { $pipeline } else { $tests }

    # Validate test names
    for test_name in $tests_to_run {
        if $test_name not-in $pipeline {
            error make {
                msg: $"Test function '($test_name)' not found"
                label: {
                    text: "Use './test-pure-rust.nu --list' to see available tests"
                    span: (metadata $test_name).span
                }
            }
        }
    }

    log-header "Pure Rust Test Suite (No ROS Required)"

    run-test-pipeline $tests_to_run { |test_name|
        do ($test_map | get $test_name)
    }

    print "\n================================================"
    log-success "All pure Rust tests passed!"
    print "================================================"
}
