#!/usr/bin/env nu

# Doc-build checker for the feedback-loop agent (Phase A).
#
# Runs cargo build/test checks, an internal cross-reference scan, and a
# rustdoc compile-test for rust,no_run blocks in the book chapters.
# Outputs a machine-readable result table to stdout and exits non-zero if any
# check fails.
#
# Usage (from repo root):
#   nu scripts/check-doc-build.nu
#   nu scripts/check-doc-build.nu --cargo /nix/store/.../bin/cargo
#   nu scripts/check-doc-build.nu --json    # JSON output for agents

use lib/common.nu *

const CARGO_CHECKS = [
    { name: "build zenoh_router",     cmd: "cargo build --example zenoh_router" }
    { name: "build z_pubsub",         cmd: "cargo build --example z_pubsub" }
    { name: "build z_srvcli",         cmd: "cargo build --example z_srvcli" }
    { name: "build z_custom_message", cmd: "cargo build --example z_custom_message" }
    { name: "test hiroz-cdr",         cmd: "cargo test -p hiroz-cdr" }
    { name: "test hiroz-codegen",     cmd: "cargo test -p hiroz-codegen" }
]

# Check all ./xxx.md cross-references in book/src/chapters/ resolve to real files.
def check-internal-links [] {
    let chapter_dir = "book/src/chapters"
    mut broken = []

    for chapter_path in (ls $chapter_dir | where name =~ '\.md$' | get name) {
        let content = (open $chapter_path)
        # Match links of the form (./foo.md) or (./foo.md#anchor)
        let raw_links = ($content | parse --regex '\(\.\/([^)#]+\.md)[^)]*\)' | get capture0)
        for link in $raw_links {
            let target = $"($chapter_dir)/($link)"
            if not ($target | path exists) {
                $broken = ($broken | append $"(($chapter_path | path basename)): broken link → ./($link)")
            }
        }
    }

    let passed = ($broken | length) == 0
    let msg = if $passed { "" } else { $broken | str join "\n" }
    {passed: $passed, stderr: $msg}
}

# Compile-test rust,no_run blocks in book chapters using rustdoc.
# Requires the workspace to be built first (target/debug/deps/ must exist).
def check-book-snippets [cargo: string] {
    let deps_dir = "target/debug/deps"
    let rustdoc = $"($cargo | path dirname)/rustdoc"

    # Find most recently modified rlibs for the crates needed by book snippets
    let hiroz_rlibs = (ls $deps_dir | where name =~ 'libhiroz-.*\.rlib$' | sort-by modified -r)
    let zenoh_rlibs = (ls $deps_dir | where name =~ 'libzenoh-.*\.rlib$' | sort-by modified -r)

    if ($hiroz_rlibs | length) == 0 {
        return {passed: false, stderr: "libhiroz.rlib not found — run cargo build first"}
    }

    let hiroz_rlib = ($hiroz_rlibs | get name | first)
    let zenoh_rlib = if ($zenoh_rlibs | length) > 0 { $zenoh_rlibs | get name | first } else { "" }

    mut all_passed = true
    mut errors = []
    mut total_tests = 0
    mut total_failed = 0

    for chapter in (ls "book/src/chapters" | where name =~ '\.md$' | get name) {
        mut args = [
            "--test" "--edition" "2021"
            "-L" $deps_dir
            "--extern" $"hiroz=($hiroz_rlib)"
        ]
        if $zenoh_rlib != "" {
            $args = ($args | append ["--extern" $"zenoh=($zenoh_rlib)"])
        }
        $args = ($args | append [$chapter])
        let final_args = $args

        let result = (do -i { (^$rustdoc ...$final_args | complete) })

        # Count test results from output
        let combined = $result.stdout + $result.stderr
        let result_line = ($combined | lines | where $it =~ "test result" | first | default "")
        if $result_line != "" {
            let passed_n = ($result_line | parse --regex '(\d+) passed' | get capture0 | first | default "0" | into int)
            let failed_n = ($result_line | parse --regex '(\d+) failed' | get capture0 | first | default "0" | into int)
            $total_tests += $passed_n + $failed_n
            $total_failed += $failed_n
        }

        if $result.exit_code != 0 {
            $all_passed = false
            let err = ($combined | lines | where $it =~ "FAILED|^error" | str join "\n")
            $errors = ($errors | append $err)
        }
    }

    let msg = if $all_passed {
        $"($total_tests) snippets compiled OK"
    } else {
        $errors | str join "\n"
    }
    {passed: $all_passed, stderr: (if $all_passed { "" } else { $msg }), summary: $msg}
}

# Run doc-build Phase A checks
#
# Examples:
#   nu scripts/check-doc-build.nu
#   nu scripts/check-doc-build.nu --cargo /nix/store/.../bin/cargo
#   nu scripts/check-doc-build.nu --json
def main [
    --cargo: string = "cargo"   # Path to cargo binary
    --json                      # Output JSON instead of table
] {
    print --stderr "\n==================================================\nDoc-Build Phase A Checks\n==================================================\n"

    mut results = []

    # --- Cargo checks ---
    for check in $CARGO_CHECKS {
        let cmd = $"($cargo) ($check.cmd | str replace 'cargo ' '')"

        print --stderr $"→ ($check.name)"
        print --stderr $"  ($cmd)"

        let outcome = do -i {
            (^$cargo ...(($check.cmd | str replace 'cargo ' '') | split row ' ')
                | complete)
        }

        let passed = $outcome.exit_code == 0
        let status = if $passed { "PASS" } else { "FAIL" }
        let icon   = if $passed { "✅" } else { "❌" }

        print --stderr $"  ($icon) ($status)\n"

        if not $passed {
            let tail = ($outcome.stderr | lines | last 10 | str join "\n")
            print --stderr $"  Error output:\n($tail)\n"
        }

        $results = ($results | append {
            check:  $check.name
            cmd:    $cmd
            status: $status
            passed: $passed
            stderr: (if $passed { "" } else { $outcome.stderr | lines | last 10 | str join "\n" })
        })
    }

    # --- Internal link check ---
    print --stderr "→ internal links (book/src/chapters/)"
    let link_result = check-internal-links
    let link_status = if $link_result.passed { "PASS" } else { "FAIL" }
    let link_icon   = if $link_result.passed { "✅" } else { "❌" }
    print --stderr $"  ($link_icon) ($link_status)\n"
    if not $link_result.passed {
        print --stderr $"  Broken links:\n($link_result.stderr)\n"
    }
    $results = ($results | append {
        check:  "internal links"
        cmd:    "book/src/chapters/ cross-reference scan"
        status: $link_status
        passed: $link_result.passed
        stderr: $link_result.stderr
    })

    # --- Book snippet compile test ---
    print --stderr "→ book snippets (rust,no_run compile check)"
    let snippet_result = check-book-snippets $cargo
    let snippet_status = if $snippet_result.passed { "PASS" } else { "FAIL" }
    let snippet_icon   = if $snippet_result.passed { "✅" } else { "❌" }
    print --stderr $"  ($snippet_icon) ($snippet_status): ($snippet_result.summary)\n"
    if not $snippet_result.passed {
        print --stderr $"  Errors:\n($snippet_result.stderr)\n"
    }
    $results = ($results | append {
        check:  "book snippets"
        cmd:    "rustdoc --test book/src/chapters/*.md"
        status: $snippet_status
        passed: $snippet_result.passed
        stderr: (if $snippet_result.passed { "" } else { $snippet_result.stderr })
    })

    # --- cargo doc metric (informational, non-failing) ---
    print --stderr "→ cargo doc (warning count, informational)"
    let doc_outcome = do -i { (^$cargo doc --no-deps -p hiroz | complete) }
    let doc_warn_match = ($doc_outcome.stderr | parse --regex 'generated (\d+) warning' | get capture0 | first | default "?")
    let doc_warnings = if $doc_warn_match != "?" { $doc_warn_match | into int } else { -1 }
    print --stderr $"  ℹ️  rustdoc warnings: ($doc_warnings)\n"

    # Summary table
    let passed_count = ($results | where passed == true | length)
    let total = ($results | length)
    let all_passed = $passed_count == $total

    if $json {
        # Clean JSON to stdout; all progress was on stderr
        print ({
            passed:  $passed_count
            total:   $total
            ok:      $all_passed
            rustdoc_warnings: $doc_warnings
            checks:  ($results | select check status stderr)
        } | to json)
    } else {
        print "\n─────────────────────────────────────────"
        print $"Phase A Results: ($passed_count)/($total) passed  |  rustdoc warnings: ($doc_warnings)"
        print "─────────────────────────────────────────"
        $results | select check status | print
        print ""

        if $all_passed {
            log-success "All Phase A checks passed"
        } else {
            let failed = ($results | where passed == false | get check)
            print $"❌ Failed checks: ($failed | str join ', ')"
        }
    }

    if not $all_passed {
        exit 1
    }
}
