#!/usr/bin/env nu
# Build hiroz from the packaged .crate archives, with no sibling source tree,
# and assert that EMBEDDED_MSGS is not empty.
#
# A crates.io build unpacks each crate into its own versioned directory, so
# `../hiroz-codegen` does not exist. This script reproduces that layout, then
# reads the table that hiroz/build.rs generated.
#
# Usage: nu scripts/ci/check-embedded-msgs-packaged.nu                (from the repository root)
#        nu scripts/ci/check-embedded-msgs-packaged.nu self-test-layout

# The crates that a published `hiroz` pulls in: `hiroz` and every path
# dependency reachable through its normal and build dependencies.
def packaged-crates [packages: list<any>]: nothing -> list<string> {
    mut seen = []
    mut todo = ["hiroz"]
    while ($todo | is-not-empty) {
        let name = ($todo | first)
        $todo = ($todo | skip 1)
        if $name in $seen { continue }
        $seen = ($seen | append $name)
        let deps = (
            $packages | where name == $name | first | get dependencies
            | where {|d| $d.path? != null and $d.kind? != "dev" }
            | get name
        )
        $todo = ($todo | append $deps)
    }
    $seen
}

# What keeps `dir` from being the crates.io layout. The layout holds
# hiroz-codegen only as hiroz-codegen-<version>, never as a bare sibling.
def layout-problems [dir: string, version: string]: nothing -> list<string> {
    mut problems = []
    if not ($dir | path join $"hiroz-codegen-($version)" | path exists) {
        $problems = ($problems | append $"($dir)/hiroz-codegen-($version) is missing")
    }
    if ($dir | path join "hiroz-codegen" | path exists) {
        $problems = ($problems | append "a sibling directory named hiroz-codegen exists")
    }
    $problems
}

# Run `cmd` and stop with `what` and the tail of its output when it fails.
def run-or-fail [what: string, cmd: closure] {
    let out = (do $cmd | complete)
    print $"($what) rc=($out.exit_code)"
    if $out.exit_code != 0 {
        print ($out.stderr | lines | last 30 | str join "\n")
        error make {msg: $what}
    }
    $out
}

# Package the crates, unpack them into `work`, build a consumer of hiroz from
# the archives and return the number of EMBEDDED_MSGS entries. A failure is an
# error whose message names the step.
def check-packaged [work: string]: nothing -> int {
    let meta = (^cargo metadata --no-deps --format-version 1 | from json)
    let version = ($meta.packages | where name == hiroz | first | get version)
    let crates = (packaged-crates $meta.packages)
    let unpacked = ($work | path join crates)
    let consumer = ($work | path join consumer)
    mkdir $unpacked ($consumer | path join src)

    print $"HEAD=(try { ^git log --oneline -1 } catch { 'unknown' })"
    print $"VERSION=($version)"
    print $"CRATES=($crates | str join ' ')"

    let package_args = ($crates | each {|c| ["-p" $c] } | flatten)
    run-or-fail "package" { ^cargo package ...$package_args --no-verify --allow-dirty } | ignore

    for name in $crates {
        let archive = ($meta.target_directory | path join package $"($name)-($version).crate")
        if not ($archive | path exists) { error make {msg: $"missing ($archive)"} }
        ^tar -xzf $archive -C $unpacked
    }
    print $"unpacked ($crates | length) crates"

    let problems = (layout-problems $unpacked $version)
    if ($problems | is-not-empty) { error make {msg: $"layout: ($problems | str join '; ')"} }

    {
        package: {name: consumer, version: "0.0.0", edition: "2021"}
        workspace: {}
        dependencies: {hiroz: $"=($version)"}
        patch: {
            crates-io: (
                $crates | reduce --fold {} {|name, acc|
                    $acc | insert $name {path: ($unpacked | path join $"($name)-($version)")}
                }
            )
        }
    } | to toml | save --force ($consumer | path join Cargo.toml)
    "fn main() {}\n" | save --force ($consumer | path join src main.rs)

    # An isolated target dir, so a stale build script output cannot satisfy the check.
    let target = ($work | path join target)
    let build = (
        run-or-fail "build" {
            with-env {CARGO_TARGET_DIR: $target} { cd $consumer; ^cargo build -p hiroz -j4 }
        }
    )
    let warned = ($build.stderr | str contains "embedded schema fallback will be empty")
    if $warned { print "build script warned that the embedded schema fallback is empty" }

    let tables = (glob ($target | path join debug build "hiroz-*" out embedded_msgs.rs))
    if ($tables | is-empty) { error make {msg: "embedded_msgs.rs not found: UNKNOWN"} }
    let table = ($tables | sort-by --reverse {|p| ls --directory $p | get modified | first } | first)
    let text = (open --raw $table)
    let count = ($text | lines | where {|l| $l | str contains "include_str!" } | length)
    print $"TABLE=($table)"
    print $"EMBEDDED_MSGS entries=($count)"

    if $count == 0 or $warned or not ($text | str contains '"std_msgs/msg/String"') {
        error make {msg: "EMBEDDED_MSGS empty or incomplete"}
    }
    $count
}

def main [] {
    let work = (mktemp --directory | str trim)
    let outcome = (try { {entries: (check-packaged $work)} } catch {|e| {error: $e.msg} })
    rm --recursive --force $work
    if "error" in $outcome {
        print $"RESULT=FAIL \(($outcome.error))"
        exit 1
    }
    print "RESULT=PASS"
}

# Run the layout check on fake trees: a good one, one with a sibling and an empty one.
def "main self-test-layout" [] {
    let tmp = (mktemp --directory | str trim)
    mkdir ($tmp | path join good hiroz-codegen-1.2.3)
    mkdir ($tmp | path join bad hiroz-codegen-1.2.3) ($tmp | path join bad hiroz-codegen)
    mkdir ($tmp | path join empty)
    let good = (layout-problems ($tmp | path join good) "1.2.3")
    let sibling = (layout-problems ($tmp | path join bad) "1.2.3")
    let missing = (layout-problems ($tmp | path join empty) "1.2.3")
    rm --recursive --force $tmp
    if ($good | is-not-empty) { print "SELFTEST=FAIL (good layout rejected)"; exit 1 }
    if ($sibling | is-empty) { print "SELFTEST=FAIL (sibling not detected)"; exit 1 }
    if ($missing | is-empty) { print "SELFTEST=FAIL (missing crate not detected)"; exit 1 }
    print "SELFTEST=PASS"
}
