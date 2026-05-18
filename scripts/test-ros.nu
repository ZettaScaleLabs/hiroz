#!/usr/bin/env nu

# ROS-Specific Test Suite
# This script tests components that require ROS 2 environment:
# - interop tests: Communication with native ROS 2 nodes via rmw_zenoh_cpp

use lib/common.nu *

# ============================================================================
# Test Functions - ROS-Dependent Components Only
# ============================================================================

def clippy-rmw [] {
    let distro = get-distro

    # rmw-zenoh-rs requires Iron+ (not supported on Humble)
    if $distro == "humble" {
        log-step "Clippy (rmw feature) - SKIPPED for Humble"
        print "  ℹ️  rmw-zenoh-rs requires ROS 2 Iron or later"
        print "  ℹ️  Humble users: use hiroz core library or rmw_zenoh_cpp"
        return
    }

    log-step "Clippy (rmw feature)"
    run-cmd "cargo clippy --all-targets --workspace -F rmw -- -D warnings"
}

def run-ros-interop [] {
    log-step "Run interop tests (requires rmw_zenoh_cpp)"

    # Check if ros2 is available
    if (which ros2 | is-empty) {
        print "  Skipping: ros2 CLI not available"
        return
    }

    $env.RMW_IMPLEMENTATION = "rmw_zenoh_cpp"
    $env.RUSTFLAGS = "-D warnings"

    let distro = get-distro
    let cmd = if $distro == "humble" {
        "cargo nextest run -p hiroz-tests --no-default-features --features ros-interop,humble"
    } else {
        $"cargo nextest run -p hiroz-tests --features ros-interop,($distro)"
    }

    # Pre-build with the same features nextest will use, so the build cache is
    # settled before nextest records binary paths in its list phase. Without
    # this, a stale fingerprint from the preceding clippy-rmw step (different
    # feature set) triggers a concurrent recompile that replaces the binary
    # while nextest is already running → double-spawn / "No such file" failure.
    let prebuild_cmd = if $distro == "humble" {
        "cargo build -j4 -p hiroz-tests --tests --no-default-features --features ros-interop,humble"
    } else {
        $"cargo build -j4 -p hiroz-tests --tests --features ros-interop,($distro)"
    }
    run-cmd $prebuild_cmd

    # Try without verbose logging first (faster)
    let result = (do -i { run-cmd $cmd --distro $distro | complete })

    # If tests failed, retry with trace logging for detailed diagnostics
    # This is CRITICAL for debugging interop issues - shows type hashes, key expressions, service calls
    if $result.exit_code != 0 {
        print "\n⚠️  ROS interop tests failed. Retrying with trace logging..."
        $env.RUST_LOG = "hiroz=trace,rmw_zenoh_cpp=debug,warn"
        run-cmd $cmd --distro $distro
    }
}

# ============================================================================
# Test Suite Configuration
# ============================================================================

def get-test-map [] {
    {
        clippy-rmw: { clippy-rmw }
        run-ros-interop: { run-ros-interop }
    }
}

def get-test-pipeline [] {
    [
        "clippy-rmw"
        "run-ros-interop"
    ]
}

# ============================================================================
# Main Entry Point
# ============================================================================

# Run ROS-specific test suite (interop tests)
#
# Examples:
#   ./test-ros.nu                           # Run all tests with default distro (jazzy)
#   ./test-ros.nu --distro humble           # Run all tests for humble
#   ./test-ros.nu --distro jazzy run-ros-interop  # Run specific test
#   ./test-ros.nu --list                    # List available test functions
def main [
    --list                       # List available test functions
    --distro: string = "jazzy"   # ROS distro to test (humble, jazzy)
    ...tests: string             # Specific test functions to run (optional)
] {
    if $list {
        print "Available test functions:"
        get-test-pipeline | each { |name| print $"  - ($name)" }
        return
    }

    validate-distro $distro
    $env.DISTRO = $distro

    let test_map = get-test-map
    let pipeline = get-test-pipeline

    let tests_to_run = if ($tests | is-empty) { $pipeline } else { $tests }

    # Validate test names
    for test_name in $tests_to_run {
        if $test_name not-in $pipeline {
            error make {
                msg: $"Test function '($test_name)' not found"
                label: {
                    text: "Use './test-ros.nu --list' to see available tests"
                    span: (metadata $test_name).span
                }
            }
        }
    }

    log-header "ROS 2 Interop Tests" $distro

    run-test-pipeline $tests_to_run { |test_name|
        do ($test_map | get $test_name)
    }

    print "\n================================================"
    log-success $"All ROS 2 ($distro | str upcase) tests passed!"
    print "================================================"
}
