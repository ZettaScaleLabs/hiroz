#!/usr/bin/env nu

# Shared utilities for hiroz test scripts

export const DISTROS = ["humble", "jazzy", "kilted", "lyrical"]

# Check if running in CI environment
export def is-ci [] {
    ($env.CI? | default "false") == "true"
}

# Check if already in a nix shell
export def in-nix-shell [] {
    ($env.IN_NIX_SHELL? | default "") != ""
}

# Get current ROS distro from environment
export def get-distro [] {
    $env.DISTRO? | default "jazzy"
}

# Log a step message (blue arrow)
export def log-step [message: string] {
    let prefix = if (is-ci) { "\n→" } else { $"\n(ansi blue)→(ansi reset)" }
    print $"($prefix) ($message)"
}

# Log a success message (green checkmark)
export def log-success [message: string] {
    let icon = if (is-ci) { "✅" } else { $"(ansi green)✅(ansi reset)" }
    print $"($icon) ($message)"
}

# Log a warning message (yellow warning)
export def log-warning [message: string] {
    let icon = if (is-ci) { "⚠️" } else { $"(ansi yellow)⚠️(ansi reset)" }
    print $"($icon) ($message)"
}

# Print a header with title and optional distro
export def log-header [title: string, distro?: string] {
    let header = if ($distro | is-empty) { $title } else { $"($title) - ROS 2 ($distro | str upcase)" }
    let sep = (1..50 | each { "=" } | str join "")
    print $"\n($sep)\n($header)\n($sep)\n"
}

# Execute a command, wrapping with nix develop if needed
#
# --shell: "nu" (default) or "bash"
# --distro: ROS distro for nix flake (empty = use .#pureRust)
export def run-cmd [
    cmd: string
    --shell: string = "nu"
    --distro: string = ""
] {
    let dist = if ($distro | is-empty) { get-distro } else { $distro }
    print $"($cmd)\n"

    if (is-ci) or (in-nix-shell) {
        # In CI or nix shell, environment is already set up
        if $shell == "bash" {
            bash -c $cmd
        } else {
            nu -c $cmd
        }
    } else {
        # Locally, wrap with nix develop
        let flake = if ($distro | is-empty) and ($env.DISTRO? | is-empty) {
            ".#pureRust"
        } else {
            $".#ros-($dist)"
        }

        if $shell == "bash" {
            nix develop $flake --command bash -c $cmd
        } else {
            nix develop $flake --command nu -c $cmd
        }
    }
}

# Run a test pipeline with error handling
export def run-test-pipeline [tests: list<string>, test_fn: closure] {
    for test_name in $tests {
        try {
            log-step $"Running: ($test_name)"
            do $test_fn $test_name
        } catch { |err|
            print $"❌ Test failed: ($test_name)\n($err.msg)"
            exit 1
        }
    }
}

# Validate that a distro is in the allowed list
export def validate-distro [distro: string] {
    if $distro not-in $DISTROS {
        error make { msg: $"Unknown distro: ($distro). Must be: ($DISTROS | str join ', ')" }
    }
}
