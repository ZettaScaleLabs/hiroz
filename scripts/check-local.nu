#!/usr/bin/env nu

# Local pre-submission check script for hiroz contributors
# Run this before opening a PR to catch issues early
#
# Suites:
#   lint  — fmt + clippy only (~30s, fast-fail gate)
#   full  — everything: lint + build + tests + docs (default)

use lib/common.nu *

# Run a single check and return status
def run-check [name: string, cmd: string, num: int] {
    let prefix = if (is-ci) { $"[($num)]" } else { $"(ansi yellow)[($num)](ansi reset)" }
    print $"($prefix) ($name)"

    let result = try {
        nu -c $cmd
        true
    } catch {
        false
    }

    if $result {
        log-success $"($name) passed"
        print ""
        {passed: true}
    } else {
        let icon = if (is-ci) { "✗" } else { $"(ansi red)✗(ansi reset)" }
        print $"($icon) ($name) failed\n"
        {passed: false}
    }
}

def summarize [all_results: list] {
    let failed_count = $all_results | where passed == false | length
    let checks_run = $all_results | length

    log-header "Summary"
    print $"Checks run: ($checks_run)"

    if $failed_count == 0 {
        log-success "All checks passed!"
        exit 0
    } else {
        let icon = if (is-ci) { "✗" } else { $"(ansi red)✗(ansi reset)" }
        let plural = if $failed_count == 1 { "check" } else { "checks" }
        print $"($icon) ($failed_count) ($plural) failed\n"
        print "Please fix the issues above before submitting your PR."
        exit 1
    }
}

def main [--suite: string = "full"] {
    # Lint suite: fmt + clippy only. Fast-fail gate before slow container builds.
    if $suite == "lint" {
        log-header "Running hiroz Lint Checks"
        let checks = [
            {name: "Formatting (cargo fmt)", cmd: "cargo fmt --check"},
            {name: "Clippy (all targets)", cmd: "cargo clippy --all-targets -- -D warnings"},
        ]
        let results = $checks | enumerate | each {|item|
            run-check $item.item.name $item.item.cmd ($item.index + 1)
        }
        summarize $results
        return
    }

    # Full suite
    log-header "Running hiroz Pre-Submission Checks"

    let checks = [
        {name: "Formatting (cargo fmt)", cmd: "cargo fmt --check"},
        {name: "Clippy (all targets)", cmd: "cargo clippy --all-targets -- -D warnings"},
        {name: "Build (cargo build)", cmd: "cargo build --examples"},
        {name: "Tests", cmd: (if (which cargo-nextest | is-not-empty) {
            "cargo nextest run"
        } else {
            "cargo test --lib --tests"
        })},
    ]

    let results = $checks | enumerate | each {|item|
        run-check $item.item.name $item.item.cmd ($item.index + 1)
    }

    # Documentation tests (if mdbook is available)
    let mdbook_result = if (which mdbook | is-not-empty) {
        run-check "Documentation tests (mdbook)" "mdbook test book -L ./target/debug/deps" ($checks | length | $in + 1)
    } else {
        log-warning "Skipping mdbook tests (mdbook not installed)"
        print "   Install with: cargo install mdbook\n"
        {passed: true}
    }

    # Example coverage check
    let example_result = if (which nu | is-not-empty) {
        run-check "Example coverage" "nu scripts/check-example-coverage.nu" ($checks | length | $in + 2)
    } else {
        log-warning "Skipping example coverage check (nushell not installed)"
        print ""
        {passed: true}
    }

    summarize ($results | append [$mdbook_result, $example_result])
}
