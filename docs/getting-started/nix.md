<!-- markdownlint-disable MD046 -->
# Reproducible Development with Nix

!!! warning
    This is an advanced topic for users familiar with Nix. If you're new to Nix, you can safely skip this chapter and use the standard [Building](./building.md) instructions instead.

hiroz provides Nix flakes for reproducible development environments with all dependencies pre-configured.

## Why Use Nix?

**Reproducible Builds:** Nix ensures every developer and CI system uses the exact same dependencies, eliminating "works on my machine" issues. Nix pins all dependencies — from compilers to ROS 2 packages — to specific versions and caches them immutably.

**Uniform CI/CD:** Continuous integration uses the same Nix configuration that runs locally, ensuring build consistency across development, testing, and deployment environments.

**Zero Setup:** New team members can start developing with a single `nix develop` command — no manual ROS installation, no dependency hunting, no environment configuration.

## Prerequisites

Install Nix with flakes enabled:

```bash
# Install Nix (Determinate Systems installer — recommended)
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install

# Verify
nix --version
```

!!! tip
    The Determinate Systems installer enables flakes by default. If using the official Nix installer, add `experimental-features = nix-command flakes` to `/etc/nix/nix.conf`.

## Available Environments

```bash
# Default: ROS 2 Jazzy with full tooling
nix develop

# Specific ROS distros
nix develop .#ros-jazzy      # ROS 2 Jazzy
nix develop .#ros-kilted     # ROS 2 Kilted
nix develop .#ros-lyrical    # ROS 2 Lyrical (LTS)
nix develop .#ros-rolling    # ROS 2 Rolling

# Pure Rust (no ROS 2 — fastest to enter)
nix develop .#pureRust
```

## Step-by-Step Usage

### 1. Enter a Development Shell

```bash
cd /path/to/hiroz
nix develop        # enters .#ros-jazzy by default
```

The first run downloads and builds all dependencies. This takes 5–15 minutes. Subsequent runs are instant (cached).

Once inside, your shell has:

- Rust toolchain (stable + nightly)
- ROS 2 Jazzy (or the distro you selected)
- All system libraries (`libclang`, `mold`, etc.)
- `cargo`, `colcon`, `ros2` CLI

### 2. Build and Test

All standard commands work inside the shell:

```bash
# Build everything
cargo build

# Build with ROS 2 messages
cargo build -p hiroz-msgs

# Run tests
cargo test -p hiroz

# Run an example
cargo run --example z_pubsub
```

### 3. Exit the Shell

```bash
exit   # or Ctrl-D
```

Your system environment remains intact — Nix installed nothing globally.

## Use Cases

| Use Case | Environment | Benefit |
|----------|-------------|---------|
| **Core Rust development** | `.#pureRust` | Fast entry, no ROS 2 needed |
| **Full ROS 2 development** | `.#ros-jazzy` | All ROS 2 tools available |
| **Multi-distro testing** | `.#ros-jazzy` + `.#ros-lyrical` + `.#ros-rolling` | Test against different distros |
| **CI/CD Pipelines** | `.#pureRust-ci` | Same environment as GitHub Actions |

## Troubleshooting

??? question "nix develop hangs or is slow"
    The first run must download and compile dependencies. Check progress with:

    ```bash
    nix develop --print-build-logs
    ```

    If it hangs completely, check available disk space (Nix needs ~10 GB in `/nix/store`):

    ```bash
    df -h /nix
    ```

??? question "Error: experimental feature 'flakes' is not enabled"
    Add to `/etc/nix/nix.conf`:

    ```
    experimental-features = nix-command flakes
    ```

    Then restart the Nix daemon:

    ```bash
    sudo systemctl restart nix-daemon
    ```

??? question "cargo: command not found inside nix develop"
    The shell hook sets up the environment. If `cargo` is missing, ensure you are
    inside the Nix shell (your prompt should change) and try:

    ```bash
    nix develop --command bash
    which cargo
    ```

## Learning More

- **[Nix Official Guide](https://nixos.org/manual/nix/stable/)** — Introduction to Nix
- **[Nix Flakes](https://wiki.nixos.org/wiki/Flakes)** — Understanding flakes
- **[hiroz flake.nix](https://github.com/ZettaScaleLabs/hiroz/blob/main/flake.nix)** — Our Nix configuration
