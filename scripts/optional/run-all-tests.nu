#!/usr/bin/env nu

# Master test orchestration script for hiroz type-description branch
# Runs all test suites with proper logging and AI-friendly output

use ../lib/common.nu *

# Test suite configuration
def get-test-suites [] {
    [
        {
            name: "test-pure-rust.nu"
            script: "./scripts/test-pure-rust.nu"
            description: "Pure Rust tests (no ROS required)"
            env: {}  # Debug logging auto-enabled on failure
        }
        {
            name: "test-ros.nu"
            script: "./scripts/test-ros.nu"
            description: "ROS interop tests"
            env: {}  # Trace logging auto-enabled on failure
        }
        # NOTE: Optional validation scripts moved to scripts/optional/
        # Not run in CI, only for local validation when needed
        # See scripts/optional/README.md for usage
    ]
}

# Ensure log directory exists
def ensure-log-dir [] {
    let log_dir = "_tmp/test-logs"
    if not ($log_dir | path exists) {
        mkdir $log_dir
    }
    $log_dir
}

# Run a single test suite with logging
def run-test-suite [suite: record] {
    let log_dir = ensure-log-dir
    let log_file = $"($log_dir)/($suite.name).log"
    let timestamp = (date now | format date "%Y-%m-%d %H:%M:%S")

    # Print test header
    print $"\n(ansi blue)→(ansi reset) Running ($suite.name)"
    print $"  Description: ($suite.description)"
    print $"  Log: ($log_file)"

    # Write log header
    let env_json = ($suite.env | to json)
    let log_header = ([
        "=============================================="
        $"Test: ($suite.name)"
        $"Started: ($timestamp)"
        $"Description: ($suite.description)"
        $"Environment: ($env_json)"
        "=============================================="
        ""
    ] | str join "\n")

    $log_header | save -f $log_file

    # Merge environment variables
    let test_env = ($suite.env | transpose key value | reduce -f {} {|it, acc|
        $acc | insert $it.key $it.value
    })

    # Run the test and capture output
    let start_time = (date now)

    let result = (
        do -i {
            with-env $test_env {
                nu -c $suite.script
                | complete
            }
        }
    )

    let end_time = (date now)
    let duration = ($end_time - $start_time) | into duration | into string

    # Append output to log file
    $result.stdout | save -a $log_file
    if ($result.stderr | str length) > 0 {
        "\n=== STDERR ===\n" | save -a $log_file
        $result.stderr | save -a $log_file
    }

    # Append footer to log file
    let footer_time = ($end_time | format date "%Y-%m-%d %H:%M:%S")
    let log_footer = ([
        ""
        "=============================================="
        $"Finished: ($footer_time)"
        $"Duration: ($duration)"
        $"Exit Code: ($result.exit_code)"
        "=============================================="
    ] | str join "\n")

    $log_footer | save -a $log_file

    # Print concise status
    if $result.exit_code == 0 {
        print $"(ansi green)✅(ansi reset) ($suite.name) - PASSED ($duration)"
        { name: $suite.name, status: "PASSED", duration: $duration, log: $log_file }
    } else {
        print $"(ansi red)❌(ansi reset) ($suite.name) - FAILED ($duration)"
        print $"  Check log for details: ($log_file)"
        { name: $suite.name, status: "FAILED", duration: $duration, log: $log_file }
    }
}

# Print summary of test results
def print-summary [results: list<record>] {
    print "\n================================================"
    print "Test Summary"
    print "================================================"

    for result in $results {
        match $result.status {
            "PASSED" => { print $"(ansi green)✅(ansi reset) ($result.name) - PASSED" }
            "FAILED" => { print $"(ansi red)❌(ansi reset) ($result.name) - FAILED" }
            "SKIPPED" => { print $"(ansi yellow)⏸️ (ansi reset) ($result.name) - SKIPPED" }
        }
    }

    print "================================================"

    let failed = ($results | where status == "FAILED" | length)
    let passed = ($results | where status == "PASSED" | length)
    let skipped = ($results | where status == "SKIPPED" | length)

    if $failed > 0 {
        print $"(ansi red)Tests failed!(ansi reset) ($failed) failed, ($passed) passed, ($skipped) skipped"
        print "\nNext steps:"
        print "1. Read log files in _tmp/test-logs/ for details"
        print "2. Fix the failing tests"
        print "3. Rerun ./scripts/run-all-tests.nu"
        print "\nFailed tests:"
        for result in ($results | where status == "FAILED") {
            print $"  - ($result.name) - log: ($result.log)"
        }
        false
    } else {
        print $"(ansi green)All tests passed!(ansi reset) ($passed) passed, ($skipped) skipped"
        true
    }
}

# Display summary of existing test logs
def show-summary [
    --tail: int = 50  # Number of lines to show from each log
    --failed-only     # Only show failed test logs
] {
    let log_dir = "_tmp/test-logs"

    if not ($log_dir | path exists) {
        print $"(ansi red)Error:(ansi reset) Log directory not found: ($log_dir)"
        print "Run ./scripts/optional/run-all-tests.nu first"
        exit 1
    }

    let log_files = (ls $log_dir | where type == file and name =~ '\.log$')

    if ($log_files | is-empty) {
        print $"(ansi yellow)No test logs found in ($log_dir)(ansi reset)"
        print "Run ./scripts/optional/run-all-tests.nu to generate logs"
        exit 0
    }

    print "================================================"
    print "Test Summary"
    print "================================================\n"

    for file in $log_files {
        let content = (open $file.name)
        let filename = ($file.name | path basename)

        # Extract exit code from log footer
        let exit_code = (
            $content
            | lines
            | where $it =~ "Exit Code:"
            | last
            | str trim
            | split row ": "
            | get 1
            | into int
        )

        let status = if $exit_code == 0 {
            $"(ansi green)✅ PASSED(ansi reset)"
        } else {
            $"(ansi red)❌ FAILED(ansi reset)"
        }

        # Skip if failed-only and this test passed
        if $failed_only and $exit_code == 0 {
            continue
        }

        print $"($status) ($filename)"
        print $"  Path: ($file.name)"
        print $"  Exit Code: ($exit_code)"

        # Show tail of log for failed tests or if requested
        if $exit_code != 0 or not $failed_only {
            print "\n  Last ($tail) lines:"
            print "  ----------------------------------------"
            $content
            | lines
            | last $tail
            | each { |line| print $"  ($line)" }
            print "  ----------------------------------------\n"
        } else {
            print ""
        }
    }

    print "================================================"
    print "Use --failed-only to see only failed tests"
    print $"Use --tail <N> to show more/fewer lines (current: ($tail))"
    print "================================================"
}

# Main entry point
def main [
    --summary        # Show summary of existing test logs (don't run tests)
    --tail: int = 50 # Lines to show in summary (used with --summary)
    --failed-only    # Only show failed tests in summary (used with --summary)
    --suite: string  # Run only specific test suite (optional)
    --skip-ros       # Skip ROS interop tests
    --continue       # Continue on failure instead of stopping
] {
    # If --summary flag is set, just show summary and exit
    if $summary {
        show-summary --tail $tail --failed-only=$failed_only
        return
    }
    let log_dir = ensure-log-dir
    print "================================================"
    print "Running All hiroz Tests"
    print "================================================"
    print $"Log directory: ($log_dir)"

    let suites = get-test-suites

    # Filter suites based on options
    let suites_to_run = if ($suite | is-not-empty) {
        $suites | where name == $suite
    } else if $skip_ros {
        $suites | where name != "test-ros.nu"
    } else {
        $suites
    }

    if ($suites_to_run | is-empty) {
        print $"(ansi red)Error:(ansi reset) No test suites match the criteria"
        exit 1
    }

    # Run test suites
    mut results = []
    mut stop_on_failure = not $continue

    for suite in $suites_to_run {
        let result = run-test-suite $suite
        $results = ($results | append $result)

        if $stop_on_failure and ($result.status == "FAILED") {
            print $"\n(ansi yellow)⚠️ (ansi reset) Stopping due to test failure"
            # Mark remaining tests as skipped
            let remaining = ($suites_to_run | skip ($results | length))
            for s in $remaining {
                $results = ($results | append {
                    name: $s.name
                    status: "SKIPPED"
                    duration: "0s"
                    log: ""
                })
            }
            break
        }
    }

    # Print summary and exit
    let success = print-summary $results
    print "================================================"

    if $success {
        exit 0
    } else {
        exit 1
    }
}
