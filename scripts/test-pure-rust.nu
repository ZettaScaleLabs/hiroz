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
    # Three exclusions, each run or covered elsewhere:
    #
    # - `hiroz-tests`: run separately below with `ros-msgs,jazzy`. Its gated
    #   suites are `#![cfg(feature = "ros-msgs")]` and compile to empty binaries
    #   reporting `0 passed` without them. No ROS install needed -- hiroz-msgs
    #   bundles the definitions.
    # - `rmw-zenoh-rs`: its build script generates bindings from ROS C headers,
    #   which this job does not have. The ROS jobs lint it via `-F rmw`.
    # - `shm_size_estimation`: needs a `/dev/shm` segment sized for a
    #   PointCloud2; a runner cannot allocate it and `zenoh-shm` fails with
    #   ENOMEM before any hiroz code runs. Covered by the `test-shm` step.
    #
    # None were in `default-members`, so none ran here before `--workspace`.
    run-cmd "cargo nextest run --no-fail-fast --workspace --exclude hiroz-tests --exclude rmw-zenoh-rs -E 'not binary(shm_size_estimation)'"
    run-cmd "cargo nextest run --no-fail-fast -p hiroz-tests --features ros-msgs,jazzy"
}

def check-bundled-msgs [] {
    log-step "Check hiroz-msgs with bundled messages"
    run-cmd "cargo check -p hiroz-msgs"
    run-cmd "cargo check -p hiroz-msgs --features bundled_msgs"
    run-cmd "cargo check -p hiroz-msgs --features common_interfaces"
    run-cmd "cargo check -p hiroz-msgs --features all_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features std_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features geometry_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features sensor_msgs"
    run-cmd "cargo check -p hiroz-msgs --no-default-features --features nav_msgs"
}

def check-hu [] {
    log-step "Check hiroz-union"
    run-cmd "cargo check -p hiroz-union"
    run-cmd "cargo clippy -p hiroz-union -- -D warnings"
    log-step "Build WASM plugins (wasm32-wasip2)"
    # Needs the wasm32-wasip2 sysroot: CI uses `.#pureRust-ci`; locally enter
    # `.#pureRust-wasm` (the default `.#pureRust` shell omits it to stay lean).
    run-cmd "cargo build --manifest-path crates/hiroz-union/plugins/Cargo.toml --target wasm32-wasip2 --workspace"
    # Standalone build to mirror a third-party plugin author's setup.
    run-cmd "cargo build --manifest-path crates/hiroz-union/plugins/hu-plugin-template/Cargo.toml --target wasm32-wasip2"
}

def clippy-hiroz-py [] {
    log-step "Clippy (hiroz-py)"
    run-cmd "cargo clippy -p hiroz-py --all-targets -- -D warnings"
}

def clippy-tests [] {
    log-step "Clippy (hiroz-tests, interop features)"
    # Every test-gating feature must be listed or its file is never linted;
    # hu-meter-tests / hu-monitor-tests gate the plugin suites (~2.3k lines).
    run-cmd "cargo clippy -p hiroz-tests --all-targets --features ros-interop,hu-meter-tests,hu-monitor-tests,jazzy -- -D warnings"
}

def check-rustdoc-links [] {
    log-step "Rustdoc links (cargo doc)"
    # `cargo doc` reports unresolved intra-doc links as *warnings* and still
    # exits 0, so the exit code proves nothing -- the diagnostics have to be
    # matched. Both spellings are checked because rustdoc emits the prose form
    # ("unresolved link to `X`") and, depending on invocation, the lint name.
    let r = (^cargo doc --no-deps -p hiroz --quiet | complete)
    let w = ($r.stderr | lines | where {|it| ($it =~ 'unresolved link') or ($it =~ 'broken_intra_doc_links')})
    if ($w | is-not-empty) {
        print ($w | str join (char newline))
        error make {msg: 'rustdoc: unresolved intra-doc links'}
    }
}

def check-python-stubs [] {
    log-step "Generated Python stubs are up to date"
    # The stubs under crates/hiroz-msgs/python/hiroz_msgs_py/types/ are
    # generated from the .msg/.srv assets and committed. Nothing used to check
    # that the committed copy still matched the generator, so an asset change
    # without a rebuild-and-commit went unnoticed -- which is how six
    # rcl_interfaces classes fell out of the checked-in copy.
    #
    # `touch build.rs` forces the generator to run even when cargo considers
    # the crate up to date; without it a warm target dir makes this a no-op
    # that passes without generating anything.
    #
    # The directory is emptied first so that *deletions* are caught too. The
    # generator only writes files for packages it currently emits -- it never
    # removes one -- so dropping a package's assets would otherwise leave its
    # orphaned stub tracked and unchanged, and the check would pass. Emptying
    # turns that into a visible ` D` entry. Verified safe: a build from an
    # empty directory reproduces exactly the committed set (13 of 13), so this
    # cannot ask for a legitimately-committed stub to be deleted.
    #
    # If the build below fails, the stubs are left deleted in the working
    # tree; `git checkout -- <stub_dir>` restores them.
    let stub_dir = "crates/hiroz-msgs/python/hiroz_msgs_py/types"
    rm -f ...(glob $"($stub_dir)/*.py")
    touch crates/hiroz-msgs/build.rs
    run-cmd "cargo build -j4 -p hiroz-msgs --features python_registry"

    # `git status --porcelain`, not `git diff`: diff reports only tracked
    # files, so a stub for a newly-added package would be generated, left
    # untracked, and silently pass.
    let drift = (^git status --porcelain -- $stub_dir | complete)
    if ($drift.stdout | str trim | is-not-empty) {
        print ($drift.stdout | str trim)
        print (^git diff -- $stub_dir | complete | get stdout)
        error make {
            msg: $"generated Python stubs are stale -- run `cargo build -p hiroz-msgs --features python_registry` and commit ($stub_dir)"
        }
    }
    print $"Generated Python stubs match the message assets."
}

def check-ffi-header [] {
    log-step "Generated FFI header is up to date"
    # `crates/hiroz-go/hiroz/hiroz_ffi.h` is generated by cbindgen from
    # `crates/hiroz/src/ffi/` (see crates/hiroz/build.rs) and committed, because
    # cgo needs it at build time. Nothing checked that the committed copy still
    # matched the generator, and it had drifted by 52 lines -- including a field
    # `namespace_` added to `hiroz_context_config_t`. cgo sizes its struct from
    # this header, so Go allocated 80 bytes where Rust reads 88: an
    # out-of-bounds read of `cfg.namespace` on every advanced-config
    # `ContextBuilder.Build()`. See #270.
    #
    # cbindgen is checked explicitly rather than left to build.rs, which
    # degrades its absence to a non-fatal `cargo:warning=` (build.rs:55-61).
    # Without this the build would succeed, write nothing, and the check would
    # report on whatever was already in the tree.
    if (which cbindgen | is-empty) {
        error make {
            msg: ("cbindgen not found, so the FFI header cannot be regenerated and this check "
                + "would pass without verifying anything. Install it (`cargo install cbindgen`, "
                + "or enter the `.#pureRust` dev shell) and re-run.")
        }
    }

    # Printed because cbindgen's output is version-dependent -- 0.29.4 emits a
    # `CDR_HEADER_LE` constant that 0.29.3 does not -- so a stale-header report
    # can also mean "generated by a different cbindgen". The flake pins the
    # authoritative one; a developer whose PATH prefers another (e.g. an older
    # `cargo install` copy in ~/.cargo/bin) sees which was used here rather
    # than an unexplained diff.
    print $"cbindgen: (^cbindgen --version | str trim)"

    # Deleted first for the same reason the Python-stub check empties its
    # directory: a build that generates nothing then leaves the committed copy
    # in place and passes. With the file absent, that failure mode surfaces as
    # a ` D` entry instead.
    #
    # If the build below fails, the header is left deleted in the working tree;
    # `git checkout -- <header>` restores it.
    let header = "crates/hiroz-go/hiroz/hiroz_ffi.h"
    rm -f $header
    # Forces build.rs to rerun even when cargo considers the crate fresh --
    # otherwise a warm target dir makes this a no-op that passes.
    touch crates/hiroz/build.rs
    run-cmd "cargo build -j4 -p hiroz --features ffi"

    let drift = (^git status --porcelain -- $header | complete)
    if ($drift.stdout | str trim | is-not-empty) {
        print ($drift.stdout | str trim)
        print (^git diff -- $header | complete | get stdout)
        error make {
            msg: $"generated FFI header is stale -- run `cargo build -p hiroz --features ffi` and commit ($header)"
        }
    }
    print $"Generated FFI header matches the Rust FFI surface."
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
    # `hiroz-tests` has no featureless configuration (enforced by its build
    # script), so name the features even though shm_example itself is ungated.
    run-cmd "cargo test --package hiroz-tests --test shm_example --features ros-msgs,jazzy"
}

# ============================================================================
# Test Suite Configuration
# ============================================================================

def get-test-map [] {
    {
        clippy-workspace: { clippy-workspace }
        run-tests: { run-tests }
        check-bundled-msgs: { check-bundled-msgs }
        check-hu: { check-hu }
        check-examples: { check-examples }
        check-rustdoc-links: { check-rustdoc-links }
        check-python-stubs: { check-python-stubs }
        check-ffi-header: { check-ffi-header }
        check-distro-features: { check-distro-features }
        clippy-hiroz-py: { clippy-hiroz-py }
        clippy-tests: { clippy-tests }
        test-shm: { test-shm }
    }
}

def get-test-pipeline [] {
    [
        "clippy-workspace"
        "run-tests"
        "check-bundled-msgs"
        "check-hu"
        "check-examples"
        "check-rustdoc-links"
        "check-python-stubs"
        "check-ffi-header"
        "check-distro-features"
        "clippy-hiroz-py"
        "clippy-tests"
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
