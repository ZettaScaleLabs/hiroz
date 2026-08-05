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
    # `hiroz-tests` is linted separately with its features: under plain
    # `--workspace` it is selected with none, so clippy would lint a set of
    # empty files and report success.
    run-cmd "cargo clippy --all-targets --workspace --exclude hiroz-tests -F rmw -- -D warnings"
    run-cmd $"cargo clippy --all-targets -p hiroz-tests --features ros-interop,($distro) -- -D warnings"
}

def run-ros-interop [] {
    log-step "Run interop tests (requires rmw_zenoh_cpp)"

    # Check if ros2 is available
    if (which ros2 | is-empty) {
        print "  Skipping: ros2 CLI not available"
        return
    }

    # Fail fast if the `run` verb plugin (ros2run) isn't installed in this
    # devshell. `ros2 --version` and `ros2 pkg prefix <pkg>` both succeed
    # even when `ros2run` is missing, so a package-set regression here was
    # previously invisible until individual interop tests spawning
    # `ros2 run ...` either hung for their full discovery timeout or, worse,
    # silently false-passed (some only assert "the process eventually
    # exited", which an instant `ros2: error: invalid choice: 'run'` also
    # satisfies). Checking it once, up front, turns that into an immediate,
    # unambiguous failure instead of a 60s+ timeout deep in an unrelated test.
    let run_verb_check = (do -i { run-cmd "ros2 run --help" | complete })
    if $run_verb_check.exit_code != 0 {
        error make {
            msg: "ros2 CLI is missing the 'run' verb (ros2run package not installed in this devshell) -- every `ros2 run ...`-based interop test would hang or false-pass. Add ros2run (and ros2launch) to the devshell's ROS package set."
        }
    }

    $env.RMW_IMPLEMENTATION = "rmw_zenoh_cpp"
    $env.RUSTFLAGS = "-D warnings"

    let distro = get-distro
    let cmd = if $distro == "humble" {
        "cargo nextest run -p hiroz-tests --profile interop --no-default-features --features ros-interop,humble"
    } else {
        $"cargo nextest run -p hiroz-tests --profile interop --features ros-interop,($distro)"
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

    # Always surface the runner's own output.
    #
    # This used to capture with `complete` and then never print, so a passing
    # ROS job logged the nextest command, produced not one line of test output,
    # and printed "All ROS 2 <distro> tests passed!". That banner was
    # unfalsifiable: nextest exits 0 when it runs *zero* tests, and each interop
    # test additionally returns early (still passing) when `check_ros2_available`
    # says no. Nothing in the log distinguished "57 interop tests passed against
    # rmw_zenoh_cpp" from "the binary matched no tests".
    print $result.stdout
    print $result.stderr

    # If tests failed, retry with trace logging for detailed diagnostics
    # This is CRITICAL for debugging interop issues - shows type hashes, key expressions, service calls
    if $result.exit_code != 0 {
        print "\n⚠️  ROS interop tests failed. Retrying with trace logging..."
        $env.RUST_LOG = "hiroz=trace,rmw_zenoh_cpp=debug,warn"
        run-cmd $cmd --distro $distro
    }

    # An exit code of 0 is necessary but not sufficient — require evidence that
    # tests actually ran. nextest's last line is
    # `Summary [  12.345s] 57 tests run: 57 passed, 0 skipped`.
    let summary = ([$result.stdout, $result.stderr] | str join "\n" | lines
        | where {|l| $l =~ 'tests run:' })

    if ($summary | is-empty) {
        error make {
            msg: $"ROS interop run produced no nextest summary line, so it is unknown whether any test ran. Command: ($cmd)"
        }
    }

    let ran = ($summary | last | parse --regex '(?<n>\d+) tests run' | get n.0 | into int)
    if $ran == 0 {
        error make {
            msg: $"ROS interop run executed 0 tests -- a vacuous pass, not a pass. Command: ($cmd)"
        }
    }
    print $"\n($ran) ROS interop tests ran against rmw_zenoh_cpp."
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
        # Reverted an earlier attempt to run run-ros-interop first: that made
        # things worse, not better -- a pre-existing RCL-interop test with its
        # own established retry-loop pattern (test_hiroz_add_two_ints_server_to_rcl_client,
        # untouched by this branch) started hard-timing-out (60s) with the
        # reordered pipeline, when it had never done so with clippy-rmw first.
        # That result disproves the "clippy-rmw causes CPU starvation"
        # hypothesis this reorder was based on -- back to the original order.
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
