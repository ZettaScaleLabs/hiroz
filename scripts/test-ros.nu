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
    let first = (do -i { run-cmd $cmd --distro $distro | complete })

    # Always surface the runner's own output. This used to capture with
    # `complete` and never print, so a passing job logged the command and then
    # "All ROS 2 <distro> tests passed!" with nothing in between.
    print $first.stdout
    print $first.stderr

    # If tests failed, retry with trace logging for detailed diagnostics.
    # This is CRITICAL for debugging interop issues - shows type hashes, key
    # expressions, service calls.
    let decided = if $first.exit_code != 0 {
        print "\n⚠️  ROS interop tests failed. Retrying with trace logging..."
        $env.RUST_LOG = "hiroz=trace,rmw_zenoh_cpp=debug,warn"
        let retry = (do -i { run-cmd $cmd --distro $distro | complete })
        print $retry.stdout
        print $retry.stderr
        if $retry.exit_code != 0 {
            error make {msg: $"ROS interop tests failed. Command: ($cmd)"}
        }
        # Gate the run that decided the outcome, not the discarded attempt.
        $retry
    } else {
        $first
    }

    let out = ([$decided.stdout, $decided.stderr] | str join "\n")

    # An exit code of 0 is necessary but not sufficient. Three things have to
    # hold, and each closes a distinct way this job has read as green while
    # proving nothing.

    # 1. A summary exists at all.
    if (($out | lines | where {|l| $l =~ 'tests run:'}) | is-empty) {
        error make {
            msg: $"ROS interop run produced no nextest summary line, so it is unknown whether any test ran. Command: ($cmd)"
        }
    }

    # 2. The *interop* suites ran -- not merely `hiroz-tests`.
    #
    # The previous version asserted a non-zero total, then reported it as
    # "N ROS interop tests". That total is the whole package: on a healthy run
    # it reads 125, of which only ~41 are interop. Deleting every interop test
    # would still have printed a confident number. Count the interop binaries
    # by name instead, and require each to be present.
    #
    # The list is distro-dependent. `type_description_interop.rs` is
    # `#![cfg(not(ros_humble))]` -- Humble has no type description service to
    # interoperate with -- so requiring it there would fail a healthy run.
    # Every other suite must be present on every distro.
    let interop_suites = ([
        pubsub_interop
        service_interop
        action_interop
        parameter_interop
        demo_nodes
    ] | append (if $distro == "humble" { [] } else { [type_description_interop] }))
    let counts = ($interop_suites | each {|suite|
        {
            suite: $suite
            n: ($out | lines | where {|l| $l =~ $"hiroz-tests::($suite) "} | length)
        }
    })
    let missing = ($counts | where n == 0 | get suite)
    if ($missing | is-not-empty) {
        error make {
            msg: $"ROS interop suites produced no tests: ($missing | str join ', '). A pass here would be vacuous. Command: ($cmd)"
        }
    }

    # 3. No test self-skipped for a missing ros2 CLI.
    #
    # Each interop test returns early -- still passing, still counted -- when
    # `check_ros2_available` is false. That is a pass with no interop in it.
    let skipped = ($out | lines | where {|l| $l =~ 'ros2 CLI not available'})
    if ($skipped | is-not-empty) {
        error make {
            msg: $"ROS interop tests self-skipped because the ros2 CLI was unavailable; they report as passes but exercised nothing. Command: ($cmd)"
        }
    }

    let total = ($counts | get n | math sum)
    print $"\n($total) ROS interop tests ran against rmw_zenoh_cpp:"
    for c in $counts { print $"    ($c.suite): ($c.n)" }
}
# ============================================================================
# Test Suite Configuration
# ============================================================================

# Assert that the RMW graph APIs report the ROS type name, not the DDS wire form.
#
# The graph stores the wire form, because liveliness tokens carry it. Every RMW
# query must demangle on the way out, as rmw_zenoh_cpp does in graph_cache.cpp.
# A regression makes `ros2 topic list -t` print `std_msgs::msg::dds_::String_`
# instead of `std_msgs/msg/String`.
#
# Not in the default pipeline: it needs rmw_zenoh_rs built and on the RMW search
# path, which only the rmw_zenoh_rs workflow sets up. Run it by name there.
def graph-type-names [] {
    log-step "Graph queries report the ROS type name"

    let topic = "/chatter"
    let expected = "std_msgs/msg/String"

    # Unset, the RMW emits 18446744073709551615 as the domain. That builds a
    # different key expression, which looks exactly like the defect under test.
    $env.ROS_DOMAIN_ID = ($env.ROS_DOMAIN_ID? | default "0")
    $env.RMW_IMPLEMENTATION = ($env.RMW_IMPLEMENTATION? | default "rmw_zenoh_rs")
    print $"  RMW_IMPLEMENTATION=($env.RMW_IMPLEMENTATION) ROS_DOMAIN_ID=($env.ROS_DOMAIN_ID)"

    # rmw_zenoh peers need a router. Nothing else in this process starts one,
    # and without it the talker cannot open a session and exits at once.
    ^pkill -9 zenohd | ignore
    sleep 500ms
    let router = job spawn { ^ros2 run rmw_zenoh_cpp rmw_zenohd }
    sleep 2sec

    let talker = job spawn { ^ros2 run demo_nodes_cpp talker }

    mut listing = ""
    for _ in 1..30 {
        $listing = (^ros2 topic list -t | complete | get stdout)
        if ($listing | str contains $topic) { break }
        sleep 1sec
    }

    # A job that already exited is gone, and `job kill` on it throws. Guard both,
    # or a dead talker reports "Job N not found" and hides the real diagnosis.
    try { job kill $talker }
    try { job kill $router }
    ^pkill -9 zenohd | ignore

    print "  --- ros2 topic list -t ---"
    print $listing

    # Prove the probe saw something before trusting any assertion about it. An
    # empty listing would satisfy "no wire form" while proving nothing.
    if not ($listing | str contains $topic) {
        error make { msg: $"($topic) never appeared, so this run proves nothing. Check that the router started and that ($env.RMW_IMPLEMENTATION) is on the RMW search path." }
    }
    if not ($listing | str contains $expected) {
        error make { msg: $"the graph did not report ($expected)" }
    }
    if ($listing | str contains "dds_::") {
        error make { msg: "the graph leaked the DDS wire form" }
    }

    log-success $"the graph reports ($expected) and no wire form"
}

def get-test-map [] {
    {
        clippy-rmw: { clippy-rmw }
        run-ros-interop: { run-ros-interop }
        graph-type-names: { graph-type-names }
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
        let pipeline = get-test-pipeline
        print "Available test functions:"
        get-test-map | columns | each { |name|
            let mark = if $name in $pipeline { "" } else { "  (not in the default run)" }
            print $"  - ($name)($mark)"
        }
        return
    }

    validate-distro $distro
    $env.DISTRO = $distro

    let test_map = get-test-map
    let pipeline = get-test-pipeline

    let tests_to_run = if ($tests | is-empty) { $pipeline } else { $tests }

    # Validate against the map, not the pipeline. The map is every runnable
    # test; the pipeline is only the default order. A test registered in the
    # map but left out of the pipeline must still be runnable by name.
    for test_name in $tests_to_run {
        if $test_name not-in ($test_map | columns) {
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
