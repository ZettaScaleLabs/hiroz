# Contributing to hiroz

Thank you for your interest in contributing to hiroz!

## Quick Start

```bash
# Clone and setup
git clone https://github.com/ZettaScaleLabs/hiroz.git
cd hiroz
nix develop  # or: install Rust via rustup

# Build and test
cargo build
cargo test

# Run all pre-submission checks
./scripts/check-local.sh
```

## Development Setup

**With Nix (Recommended):**

```bash
nix develop
# or: direnv allow
```

**Without Nix:**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
python3 -m pip install mkdocs-material
```

## Pre-Submission Checks

Run before opening a PR:

```bash
./scripts/check-local.sh
```

This runs:

- `cargo fmt --check` - formatting
- `cargo clippy --all-targets -- -D warnings` - linting
- `cargo build` - compilation
- `cargo test` - tests
- `mkdocs build --strict` - documentation (when MkDocs is installed)

### Go binding changes

Build the native library, C header, Go package, and generated messages from the
same revision. The complete [Go API validation guide](docs/bindings/go-api-validation.md)
includes focused regressions and the combined check:

```bash
./scripts/check-go-api.sh
```

Install the Go version declared in `crates/hiroz-go/go.mod`, a C/C++ toolchain,
Python 3, and cbindgen 0.29.4. These tests require loopback sockets, but no ROS
installation or external router. Report skipped checks explicitly; compiling the
ROS interoperability suite does not establish wire interoperability.

## Code Style

- Use `cargo fmt` for formatting
- Fix all `cargo clippy` warnings
- Follow [conventional commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`

## Pull Request Workflow

### For External Contributors

**Please open PRs as DRAFT initially:**

1. Run `./scripts/check-local.sh` locally
2. Open PR as **draft**
3. Mark "Ready for review" when all checks pass

This helps us manage CI workflow approvals.

### For All Contributors

- Write clear PR descriptions
- Reference issues: `Fixes #123` or `Relates to #456`
- Keep PRs small and focused
- Respond to review feedback promptly

## Documentation

- Examples live in `crates/hiroz/examples/` as runnable programs
- MkDocs pages live in `docs/`; keep examples runnable and link their source.
- Test documentation: `mkdocs serve`

See the [book](https://zettascalelabs.github.io/hiroz/) for detailed guides.

## Getting Help

- **Questions**: [GitHub Discussions](https://github.com/ZettaScaleLabs/hiroz/discussions)
- **Bugs**: [GitHub Issues](https://github.com/ZettaScaleLabs/hiroz/issues)
- **Chat**: [Zenoh Discord](https://discord.gg/vSDSpqnbkm)

## License

Contributions are licensed under the same license as the project (see LICENSE).

---

Thank you for contributing to hiroz! 🦀
