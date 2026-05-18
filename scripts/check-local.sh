#!/usr/bin/env bash
# Local pre-submission check script for hiroz contributors
# Run this before opening a PR to catch issues early

set -e  # Exit on first error

FAILED=0
CHECKS_RUN=0

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "========================================="
echo "Running hiroz Pre-Submission Checks"
echo "========================================="
echo ""

run_check() {
    local name="$1"
    local cmd="$2"
    CHECKS_RUN=$((CHECKS_RUN + 1))

    echo -e "${YELLOW}[$CHECKS_RUN]${NC} $name"
    if eval "$cmd"; then
        echo -e "${GREEN}✓${NC} $name passed"
        echo ""
    else
        echo -e "${RED}✗${NC} $name failed"
        echo ""
        FAILED=$((FAILED + 1))
    fi
}

# 1. Check formatting
run_check "Formatting (cargo fmt)" "cargo fmt --check"

# 2. Clippy lints
run_check "Clippy (all targets)" "cargo clippy --all-targets -- -D warnings"

# 3. Build workspace
run_check "Build (cargo build)" "cargo build"

# 4. Run tests (using nextest like CI does)
if command -v cargo-nextest &> /dev/null; then
    run_check "Tests (cargo nextest)" "cargo nextest run"
else
    # Fallback: run unit/integration tests but skip doctests
    run_check "Tests (cargo test)" "cargo test --lib --tests"
fi

# 5. Documentation tests (if mdbook is available)
if command -v mdbook &> /dev/null; then
    run_check "Documentation tests (mdbook)" "mdbook test book -L ./target/debug/deps"
else
    echo -e "${YELLOW}⚠${NC} Skipping mdbook tests (mdbook not installed)"
    echo "   Install with: cargo install mdbook"
    echo ""
fi

# 6. Build all examples
run_check "Examples build (cargo build --examples)" "cargo build --examples"

# 7. hiroz-console clippy (if nushell available)
if command -v nu &> /dev/null; then
    run_check "Console clippy (check-console)" "nu scripts/test-pure-rust.nu check-console"
else
    echo -e "${YELLOW}⚠${NC} Skipping console clippy (nushell not installed)"
    echo ""
fi

# 8. SHM tests (if nushell available)
if command -v nu &> /dev/null; then
    run_check "SHM tests (test-shm)" "nu scripts/test-pure-rust.nu test-shm"
else
    echo -e "${YELLOW}⚠${NC} Skipping SHM tests (nushell not installed)"
    echo ""
fi

# 9. Distro feature flags (if nushell available)
if command -v nu &> /dev/null; then
    run_check "Distro feature flags (check-distro-features)" "nu scripts/test-pure-rust.nu check-distro-features"
else
    echo -e "${YELLOW}⚠${NC} Skipping distro feature flag checks (nushell not installed)"
    echo ""
fi

# 10. Rustdoc link check
check_rustdoc_links() {
    local warnings
    warnings=$(cargo doc --no-deps -p hiroz --quiet 2>&1 | grep -E "unresolved link|broken_intra_doc_links" || true)
    if [ -n "$warnings" ]; then
        echo "$warnings"
        return 1
    fi
}
run_check "Rustdoc links (cargo doc)" "check_rustdoc_links"

# 11. Example coverage check (if nushell is available)
if command -v nu &> /dev/null; then
    run_check "Example coverage (check-example-coverage.nu)" "nu scripts/check-example-coverage.nu"
else
    echo -e "${YELLOW}⚠${NC} Skipping example coverage check (nushell not installed)"
    echo ""
fi

# Summary
echo "========================================="
echo "Summary"
echo "========================================="
echo "Checks run: $CHECKS_RUN"

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}All checks passed!${NC} ✓"
    echo ""
    echo "You're ready to:"
    echo "1. Commit your changes"
    echo "2. Push to your fork"
    echo "3. Open a PR (as a draft initially)"
    exit 0
else
    echo -e "${RED}$FAILED check(s) failed${NC} ✗"
    echo ""
    echo "Please fix the issues above before submitting your PR."
    exit 1
fi
