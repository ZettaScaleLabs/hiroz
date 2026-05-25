#!/usr/bin/env nu

# Python (hiroz-py) Test Suite
# Tests Python bindings for hiroz

use lib/common.nu *

# ============================================================================
# Test Functions
# ============================================================================

# --- Setup ---

def setup-nix-env [] {
    log-step "Setting up Nix environment"

    if (is-ci) {
        print "  Using pre-configured CI environment"
    } else if (in-nix-shell) {
        print $"  Already in nix develop shell for (get-distro)"
    } else {
        print $"  Using nix develop for (get-distro)"
    }
}

def setup-venv [] {
    log-step "Set up Python virtual environment"

    # Create venv if missing or if bin/python is a broken symlink (stale cache)
    let python_bin = "crates/hiroz-py/.venv/bin/python"
    let venv_ok = ("crates/hiroz-py/.venv" | path exists) and ($python_bin | path exists)
    if not $venv_ok {
        run-cmd "cd crates/hiroz-py; rm -rf .venv && python -m venv .venv" --shell bash --distro (get-distro)
        print "  Created new virtual environment"
    } else {
        print "  Virtual environment exists"
    }

    # Build hiroz-msgs with python_registry feature to generate Python types
    run-cmd "cargo build -p hiroz-msgs --features python_registry" --shell bash --distro (get-distro)
    print "  Generated Python message types"

    # Install hiroz-msgs-py (pure Python message definitions)
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && pip install -e ../hiroz-msgs/python/" --shell bash --distro (get-distro)
    print "  Installed hiroz-msgs-py (message types)"

    # Install hiroz-py in editable mode using maturin
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && RUSTFLAGS='-D warnings' maturin develop" --shell bash --distro (get-distro)
    print "  Installed hiroz-py (Rust bindings)"
}

# --- Linting Functions ---

def lint-ruff [] {
    log-step "Ruff linting (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && ruff check tests/ examples/ --output-format=github" --shell bash --distro (get-distro)
}

def format-ruff [] {
    log-step "Ruff format check (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && ruff format --check tests/ examples/" --shell bash --distro (get-distro)
}

def format-ruff-fix [] {
    log-step "Ruff format fix (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && ruff format tests/ examples/" --shell bash --distro (get-distro)
}

# --- Type Checking Functions ---

def typecheck-mypy [] {
    log-step "MyPy type checking (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && mypy tests/ examples/ --ignore-missing-imports" --shell bash --distro (get-distro)
}

# --- Build Functions ---

def build-package [] {
    log-step "Build Python package (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && maturin build" --shell bash --distro (get-distro)
}

def clippy [] {
    log-step "Clippy (hiroz-py)"
    run-cmd "cargo clippy -p hiroz-py --all-targets -- -D warnings" --shell bash --distro (get-distro)
}

# --- Test Functions ---

def run-pytest [] {
    log-step "Run pytest (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && python -m pytest tests/ -v" --shell bash --distro (get-distro)
}

def run-pytest-coverage [] {
    log-step "Run pytest with coverage (hiroz-py)"
    run-cmd "cd crates/hiroz-py; source .venv/bin/activate && python -m pytest tests/ --cov=hiroz_py --cov-report=term-missing --cov-report=html --cov-fail-under=80" --shell bash --distro (get-distro)
}

def run-examples [] {
    log-step "Run Python examples (hiroz-py)"

    if not ("crates/hiroz-py/examples" | path exists) {
        print "  Skipping: no examples directory found"
        return
    }

    # FIXME: workaround the timeout exit code
    # run-cmd "cd crates/hiroz-py; source .venv/bin/activate && timeout 2 python examples/talker.py" --shell bash --distro (get-distro)
}

def run-python-interop [] {
    log-step "Run Python interop tests (hiroz-tests)"
    run-cmd "source crates/hiroz-py/.venv/bin/activate && cargo test --features python-interop -p hiroz-tests --test python_interop -- --nocapture" --shell bash --distro (get-distro)
}

# --- Cleanup Functions ---

def cleanup-python [] {
    if (is-ci) {
        log-step "Cleaning up Python artifacts"

        try {
            rm -rf crates/hiroz-py/.pytest_cache
            rm -rf crates/hiroz-py/.mypy_cache
            rm -rf crates/hiroz-py/.ruff_cache
            rm -rf crates/hiroz-py/__pycache__
            rm -rf crates/hiroz-py/python/**/__pycache__
            rm -rf crates/hiroz-py/htmlcov
            rm -rf crates/hiroz-py/.coverage
            rm -rf crates/hiroz-py/dist
            rm -rf crates/hiroz-py/build
            rm -rf crates/hiroz-py/*.egg-info
        }

        df -h
    }
}

# ============================================================================
# Test Suite Configuration
# ============================================================================

def get-test-map [] {
    {
        setup-venv: { setup-venv }
        clippy: { clippy }
        lint-ruff: { lint-ruff }
        format-ruff: { format-ruff }
        format-ruff-fix: { format-ruff-fix }
        build-package: { build-package }
        typecheck-mypy: { typecheck-mypy }
        run-pytest: { run-pytest }
        run-pytest-coverage: { run-pytest-coverage }
        run-python-interop: { run-python-interop }
        run-examples: { run-examples }
        cleanup-python: { cleanup-python }
    }
}

def get-test-pipeline [] {
    [
        "setup-venv"
        "clippy"
        "lint-ruff"
        "format-ruff"
        "build-package"
        "typecheck-mypy"
        "run-pytest"
        "run-python-interop"
        "run-examples"
        "cleanup-python"
    ]
}

# ============================================================================
# Main Entry Point
# ============================================================================

# Run Python test suite
#
# Examples:
#   ./test-python.nu                           # Run all tests with default distro (jazzy)
#   ./test-python.nu --distro humble           # Run all tests for humble
#   ./test-python.nu --distro jazzy lint-ruff  # Run specific tests
#   ./test-python.nu --list                    # List available test functions
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
        if $test_name not-in $pipeline and $test_name not-in ["format-ruff-fix", "run-pytest-coverage"] {
            error make {
                msg: $"Test function '($test_name)' not found"
                label: {
                    text: "Use './test-python.nu --list' to see available tests"
                    span: (metadata $test_name).span
                }
            }
        }
    }

    log-header "Python (hiroz-py) Test Suite" $distro
    setup-nix-env

    run-test-pipeline $tests_to_run { |test_name|
        do ($test_map | get $test_name)
    }

    print "\n================================================"
    log-success $"All Python hiroz-py tests passed for ($distro | str upcase)!"
    print "================================================"
}
