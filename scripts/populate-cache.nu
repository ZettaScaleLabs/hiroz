#!/usr/bin/env nu

# Populate hiroz.cachix.org with all CI devShell closures.
#
# Builds each devShell and pushes its full Nix store closure to the cache so
# CI runs (including PRs) can download instead of rebuild.
#
# Prerequisites:
#   nix profile install nixpkgs#cachix
#   cachix authtoken <token>   # token from https://app.cachix.org/cache/hiroz
#
# Usage:
#   nu scripts/populate-cache.nu                         # build + push all
#   nu scripts/populate-cache.nu --only pureRust-ci      # single devShell
#   nu scripts/populate-cache.nu --dry-run               # preview only

use lib/common.nu [log-step, log-success, log-warning, log-header]

const CACHE = "hiroz"

const DEVSHELLS = [
    "pureRust-ci"        # formatting, no-ros-test, coverage, no-ros-checks
    "ros-jazzy-ci"       # python-tests (jazzy), ros-tests (jazzy)
    "ros-humble-ci"      # python-tests (humble), ros-tests (humble)
    "ros-kilted-ci"      # python-tests (kilted), ros-tests (kilted)
    "ros-bridge-interop" # bridge-interop job
]

# ── helpers ──────────────────────────────────────────────────────────────────

def check-prereqs [] {
    for bin in ["nix" "cachix"] {
        if (which $bin | is-empty) {
            error make {msg: $"($bin) not found on PATH"}
        }
    }

    # Lightweight auth check — `cachix push --help` exits 0 even unauthenticated,
    # but a dry nix-store ping with a known-good path will tell us if the daemon works.
    # Just warn if the token looks absent; cachix itself will error clearly on push.
    let token_check = (^cachix whoami 2>&1 | complete)
    if $token_check.exit_code != 0 {
        print $"(ansi yellow)⚠  cachix whoami failed — make sure you have run: cachix authtoken <token>(ansi reset)"
        print $"   ($token_check.stdout | str trim)"
    }
}

def elapsed-str [start: datetime] -> string {
    let s = (((date now) - $start) / 1sec | math round)
    if $s < 60 { $"($s)s" } else { $"($s / 60 | math round)m ($s mod 60)s" }
}

# Build one devShell and push its closure. Returns a result record.
def push-devshell [shell: string, system: string, dry_run: bool] -> record {
    let attr = $".#devShells.($system).($shell)"
    log-step $"Building ($attr)"
    let t0 = (date now)

    # Build with live log output (-L streams to terminal via stderr).
    # nix exits non-zero on failure; `try` catches it.
    try {
        ^nix build $attr --no-link -L --accept-flake-config
    } catch {|err|
        print $"(ansi red)✗ build failed: ($err.msg)(ansi reset)"
        return {shell: $shell, status: "failed", elapsed: (elapsed-str $t0), pushed: 0}
    }

    # Collect output paths (instant — build is already cached).
    let out_paths = (
        ^nix build $attr --print-out-paths --no-link
        | lines
        | where ($it | str starts-with "/nix/store/")
    )

    if ($out_paths | is-empty) {
        log-warning $"($shell): no output paths"
        return {shell: $shell, status: "empty", elapsed: (elapsed-str $t0), pushed: 0}
    }

    # Expand to full transitive closure.
    let closure = (
        $out_paths
        | each {|p| ^nix path-info --recursive $p | lines}
        | flatten
        | uniq
        | sort
    )
    let count = ($closure | length)

    print $"  → ($count) paths in closure"

    if $dry_run {
        print $"  (dry-run) skipping push"
    } else {
        log-step $"  Pushing to ($CACHE).cachix.org..."
        $closure | str join "\n" | ^cachix push $CACHE
    }

    log-success $"($shell) done in (elapsed-str $t0) — ($count) paths"
    {shell: $shell, status: "ok", elapsed: (elapsed-str $t0), pushed: $count}
}

# ── main ─────────────────────────────────────────────────────────────────────

# Populate hiroz.cachix.org with CI devShell closures
def main [
    --system: string = "x86_64-linux"  # Nix system triple (must match CI runners)
    --only: string = ""                 # Comma-separated devShells to build (default: all)
    --dry-run                           # Preview closure sizes without pushing
] {
    check-prereqs

    let shells = if ($only | is-empty) {
        $DEVSHELLS
    } else {
        $only | split row "," | each { str trim } | where ($it | is-not-empty)
    }

    log-header $"Populating ($CACHE).cachix.org"
    print $"  system:    ($system)"
    print $"  devShells: ($shells | str join ', ')"
    if $dry_run { print $"  mode:      (ansi yellow)dry-run(ansi reset)" }
    print ""

    let t_total = (date now)

    let results = ($shells | each {|shell|
        try {
            push-devshell $shell $system $dry_run
        } catch {|err|
            log-warning $"($shell) error: ($err.msg)"
            {shell: $shell, status: "error", elapsed: "0s", pushed: 0}
        }
    })

    let total_paths = ($results | get pushed | math sum)
    let failed = ($results | where status in ["failed" "error"])

    print $"\n(ansi bold)═══ Summary ═══(ansi reset)"
    print $"  Total time:  (elapsed-str $t_total)"
    print $"  Paths pushed: ($total_paths)"
    print ""
    $results | select shell status elapsed pushed | print

    if ($failed | is-not-empty) {
        print $"\n(ansi red)Failed: ($failed | get shell | str join ', ')(ansi reset)"
        exit 1
    }
}
