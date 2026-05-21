#!/usr/bin/env nu

# Go Binding Health Test Suite
# Tests the ros-z-go and ros-z-codegen-go packages to ensure they build and run correctly

use lib/common.nu *

# ============================================================================
# Configuration
# ============================================================================

const CODEGEN_DIR = "crates/ros-z-codegen-go"
const RUNTIME_DIR = "crates/ros-z-go"

# ============================================================================
# Helpers
# ============================================================================

# Check if the Rust FFI library is available for linking
def has-ffi-library [] {
    ("target/release/libros_z.a" | path exists) or ("target/debug/libros_z.a" | path exists)
}

# ============================================================================
# Test Functions
# ============================================================================

# Check if Go is installed and get version
def check-go-installation [] {
    log-step "Checking Go installation"

    try {
        let version = (go version | complete)
        if $version.exit_code != 0 {
            print "❌ Go is not installed or not in PATH"
            exit 1
        }
        print $"   Go version: ($version.stdout | str trim)"
        log-success "Go installation found"
    } catch {
        print "❌ Go is not installed or not in PATH"
        exit 1
    }
}

# Test ros-z-codegen-go (code generator)
def test-codegen [] {
    log-step "Testing ros-z-codegen-go (code generator)"

    # Format check
    log-step "Running gofmt on code generator"
    cd $CODEGEN_DIR
    let fmt_result = (gofmt -l . | complete)
    if ($fmt_result.stdout | str trim | is-empty) {
        log-success "Code generator is properly formatted"
    } else {
        print "⚠️  Code generator has formatting issues:"
        print $fmt_result.stdout
        print "   Run 'gofmt -w .' in crates/ros-z-codegen-go to fix"
    }
    cd ../..

    # Build
    log-step "Building code generator"
    cd $CODEGEN_DIR
    let build_result = (go build -v ./... | complete)
    if $build_result.exit_code != 0 {
        print "❌ Code generator build failed:"
        print $build_result.stderr
        exit 1
    }
    log-success "Code generator builds successfully"
    cd ../..

    # Run tests
    log-step "Running code generator tests"
    cd $CODEGEN_DIR
    let test_result = (go test -v ./... | complete)
    if $test_result.exit_code != 0 {
        print "❌ Code generator tests failed:"
        print $test_result.stdout
        print $test_result.stderr
        exit 1
    }
    print $test_result.stdout
    log-success "Code generator tests pass"
    cd ../..
}

# Test ros-z-go runtime library
def test-runtime [] {
    log-step "Testing ros-z-go (runtime library)"

    # Format check
    log-step "Running gofmt on runtime library"
    cd $RUNTIME_DIR
    let fmt_result = (gofmt -l . | complete)
    if ($fmt_result.stdout | str trim | is-empty) {
        log-success "Runtime library is properly formatted"
    } else {
        print "⚠️  Runtime library has formatting issues:"
        print $fmt_result.stdout
        print "   Run 'gofmt -w .' in crates/ros-z-go to fix"
    }
    cd ../..

    # Pure Go serialization tests (always available)
    log-step "Running testdata tests (serialization logic)"
    cd $RUNTIME_DIR
    let test_result = (go test -v ./testdata | complete)
    if $test_result.exit_code != 0 {
        print "❌ Testdata tests failed:"
        print $test_result.stdout
        print $test_result.stderr
        exit 1
    }
    print $test_result.stdout
    log-success "Testdata tests pass"
    cd ../..

    # FFI tests (requires compiled Rust library)
    if (has-ffi-library) {
        let lib_dir = if ("target/release/libros_z.a" | path exists) {
            $"(pwd)/target/release"
        } else {
            $"(pwd)/target/debug"
        }
        log-step "Running rosz FFI tests (QoS, types, handlers, subscribers)"
        cd $RUNTIME_DIR
        let ffi_result = (with-env {CGO_LDFLAGS: $"-L($lib_dir)"} {
            go test -v -count=1 ./rosz/ | complete
        })
        if $ffi_result.exit_code != 0 {
            print "❌ rosz FFI tests failed:"
            print $ffi_result.stdout
            print $ffi_result.stderr
            exit 1
        }
        print $ffi_result.stdout
        log-success "rosz FFI tests pass"
        cd ../..
    } else {
        print ""
        log-warning "Skipping rosz FFI tests (Rust library not built)"
        print "   Build with: cargo build -p ros-z --features ffi --release"
    }
}

# Check if zenohd is on PATH (required for integration tests)
def has-zenohd [] {
    (which zenohd | length) > 0
}

# Check if Rust example binaries are built (needed for Go↔Rust interop tests)
def has-rust-examples [] {
    (("target/release/examples/z_pubsub" | path exists) and ("target/release/examples/z_srvcli" | path exists))
}

# Run integration tests (requires zenohd + libros_z.a)
# Covers: Go↔Go pubsub/service/action and Go↔Rust interop tests
def test-integration [--race] {
    log-step "Running Go integration tests"

    if not (has-ffi-library) {
        log-warning "Skipping integration tests (Rust library not built)"
        print "   Build with: just -f crates/ros-z-go/justfile build-rust"
        return
    }

    if not (has-zenohd) {
        log-warning "zenohd not on PATH — tests will use compiled zenoh_router example instead"
    }

    let lib_dir = if ("target/release/libros_z.a" | path exists) {
        $"(pwd)/target/release"
    } else {
        $"(pwd)/target/debug"
    }

    # Report Go↔Rust test status
    if (has-rust-examples) {
        log-step "Rust example binaries found — Go↔Rust interop tests will run"
    } else {
        log-warning "Rust example binaries not found — Go↔Rust tests will be skipped"
        print "   Build with: just -f crates/ros-z-go/justfile build-rust-examples"
    }

    cd $RUNTIME_DIR
    let flags = if $race { ["-race"] } else { [] }
    let result = (with-env {CGO_LDFLAGS: $"-L($lib_dir)"} {
        go test -v -tags integration ...$flags -count=1 -timeout 300s ./interop_tests/... | complete
    })
    cd ../..

    if $result.exit_code != 0 {
        print "❌ Integration tests failed:"
        print $result.stdout
        print $result.stderr
        exit 1
    }
    print $result.stdout
    log-success "Integration tests pass"
}

# Test building examples
def test-examples [] {
    log-step "Testing Go examples"

    if not (has-ffi-library) {
        log-warning "Skipping examples (Rust FFI library not built)"
        print "   Build with: cargo build -p ros-z --features ffi --release"
        return
    }

    # Examples that live in the parent module (use generated messages)
    let ffi_examples = [
        "publisher"
        "subscriber"
        "subscriber_channel"
        "service_client"
        "service_server"
        "service_client_errors"
        "action_client"
        "action_client_errors"
        "action_server"
    ]

    # These examples need generated message packages to compile
    let needs_generated = ("crates/ros-z-go/generated" | path exists)

    if not $needs_generated {
        log-warning "Generated messages not found — examples that import them will be skipped"
        print "   To generate: make codegen"
    }

    cd $RUNTIME_DIR

    for example in $ffi_examples {
        log-step $"Building example: ($example)"
        let result = (go build -v $"./examples/($example)" | complete)
        if $result.exit_code != 0 {
            if (not $needs_generated) and ($result.stderr | str contains "generated") {
                let msg = $"   Skipped ($example) - needs generated messages"
                log-warning $msg
            } else {
                print $"❌ Example ($example) build failed:"
                print $result.stderr
                exit 1
            }
        } else {
            log-success $"Example ($example) builds"
        }
    }

    cd ../..

    # production_service has its own go.mod — test separately
    if ("crates/ros-z-go/examples/production_service/go.mod" | path exists) {
        log-step "Building example: production_service (separate module)"
        cd "crates/ros-z-go/examples/production_service"
        let result = (go build -v ./... | complete)
        if $result.exit_code != 0 {
            print "⚠️  production_service build failed (may need local replace directives):"
            print $result.stderr
        } else {
            log-success "Example production_service builds"
        }
        cd ../../../..
    }
}

# Run go vet for static analysis
# go vet does not link, so no FFI library is required — even for CGO packages
def test-vet [] {
    log-step "Running go vet (static analysis)"

    # Vet code generator
    log-step "Vetting code generator"
    cd $CODEGEN_DIR
    let vet_result = (go vet ./... | complete)
    if $vet_result.exit_code != 0 {
        print "❌ Code generator failed go vet:"
        print $vet_result.stderr
        exit 1
    }
    log-success "Code generator passes go vet"
    cd ../..

    # Vet runtime library packages
    # rosz uses CGO but go vet does not link — no libros_z.a needed
    log-step "Vetting rosz package"
    cd $RUNTIME_DIR
    let vet_rosz = (go vet ./rosz/... | complete)
    if $vet_rosz.exit_code != 0 {
        print "❌ rosz package failed go vet:"
        print $vet_rosz.stderr
        exit 1
    }
    log-success "rosz package passes go vet"

    log-step "Vetting testdata package"
    let vet_testdata = (go vet ./testdata/ | complete)
    if $vet_testdata.exit_code != 0 {
        print "❌ testdata package failed go vet:"
        print $vet_testdata.stderr
        exit 1
    }
    log-success "testdata package passes go vet"

    # Vet generated message packages if they exist
    if ("generated" | path exists) {
        log-step "Vetting generated packages"
        let vet_gen = (go vet ./generated/... | complete)
        if $vet_gen.exit_code != 0 {
            print "❌ generated packages failed go vet:"
            print $vet_gen.stderr
            exit 1
        }
        log-success "generated packages pass go vet"
    }
    cd ../..
}

# ============================================================================
# Main Script
# ============================================================================

def main [
    --codegen-only    # Only test the code generator
    --runtime-only    # Only test the runtime library
    --examples-only   # Only test the examples
    --vet-only        # Only run go vet
    --ffi-only        # Only run the rosz FFI tests
    --integration     # Run integration tests (requires zenohd + libros_z.a)
    --race            # Enable race detector in all Go test runs
] {
    log-header "Go Binding Health Test Suite"

    check-go-installation

    if (has-ffi-library) {
        log-success "Rust FFI library found — full test coverage enabled"
    } else {
        log-warning "Rust FFI library not found — some tests will be skipped"
        print "   Build with: just -f crates/ros-z-go/justfile build-rust"
    }

    if $integration {
        if (has-zenohd) {
            print $"   zenohd found — integration tests will run"
        } else {
            log-warning "zenohd not on PATH — integration tests will be skipped"
            print "   Install: cargo install zenohd"
        }
        if (has-rust-examples) {
            print $"   Rust examples found — Go↔Rust tests will run"
        } else {
            print $"   Rust examples not built — Go↔Rust tests will be skipped"
            print "   Build: just -f crates/ros-z-go/justfile build-rust-examples"
        }
    }
    print ""

    if $codegen_only {
        test-codegen
    } else if $runtime_only {
        test-runtime
    } else if $examples_only {
        test-examples
    } else if $vet_only {
        test-vet
    } else if $ffi_only {
        test-runtime  # includes FFI tests when library is present
    } else if $integration {
        test-integration --race=$race
    } else {
        test-codegen
        test-runtime
        test-examples
        test-vet
    }

    log-success "All Go binding health checks passed!"
}
