#!/usr/bin/env nu

# Build hiroz-py and hiroz-msgs-py distribution wheels locally.
#
# Mirrors the steps in .github/workflows/release.yml so you can verify the
# wheel build and test pip-install before pushing a release tag.
#
# Output:
#   crates/hiroz-py/dist/      ← hiroz-py wheels (one per distro)
#   crates/hiroz-msgs/python/dist/  ← hiroz-msgs-py wheel + sdist
#
# Examples:
#   ./scripts/build-python-wheels.nu                     # jazzy + humble
#   ./scripts/build-python-wheels.nu --distros [jazzy]   # jazzy only
#   ./scripts/build-python-wheels.nu --install           # build + pip install into .venv

use lib/common.nu *

def build-msgs-wheel [] {
    log-step "Build hiroz-msgs-py wheel"
    run-cmd "cd crates/hiroz-msgs/python && python -m build" --shell bash
}

def build-hiroz-py-wheel [distro: string] {
    log-step $"Build hiroz-py wheel for ($distro)"
    run-cmd $"cd crates/hiroz-py && maturin build --release --strip --features 'pyo3/extension-module,($distro)' --no-default-features --out dist" --shell bash --distro $distro
}

def rename-wheel [distro: string] {
    log-step $"Rename wheel: insert distro as build tag (0($distro))"
    # Only rename wheels that haven't been tagged yet (no 0{distro} build tag present)
    let whl = (ls crates/hiroz-py/dist/*.whl
        | where { |f| not ($DISTROS | any { |d| ($f.name | str contains $"-0($d)-") }) }
        | get name)
    for w in $whl {
        # hiroz_py-0.1.0-cp311-... → hiroz_py-0.1.0-0jazzy-cp311-...
        let base = ($w | path basename | str replace --regex '\.whl$' '')
        let name = ($base | split row '-' | first)
        let rest = ($base | str replace $"($name)-" '')
        let version = ($rest | split row '-' | first)
        let tags = ($rest | str replace $"($version)-" '')
        let dest = ($w | path dirname) + $"/($name)-($version)-0($distro)-($tags).whl"
        mv $w $dest
        print $"  ($w | path basename) → ($dest | path basename)"
    }
}

def install-into-venv [] {
    log-step "Install wheels into crates/hiroz-py/.venv"

    if not ("crates/hiroz-py/.venv" | path exists) {
        run-cmd "python -m venv crates/hiroz-py/.venv" --shell bash
    }

    let msgs_wheel = (ls crates/hiroz-msgs/python/dist/*.whl | first | get name)
    run-cmd $"
        source crates/hiroz-py/.venv/bin/activate
        pip install --force-reinstall ($msgs_wheel)
    " --shell bash

    let py_wheel = (ls crates/hiroz-py/dist/*.whl | first | get name)
    run-cmd $"
        source crates/hiroz-py/.venv/bin/activate
        pip install --force-reinstall --no-deps ($py_wheel)
    " --shell bash

    print ""
    log-success "Installed. Activate with:"
    print $"  source crates/hiroz-py/.venv/bin/activate"
}

def print-install-hint [] {
    print "\n── Built wheels ──────────────────────────────────────────"
    ls crates/hiroz-msgs/python/dist/*.whl | get name | each { |w| print $"  ($w)" }
    ls crates/hiroz-py/dist/*.whl | get name | each { |w| print $"  ($w)" }
    print ""
    print "To pip-install from the local files, run:"
    let msgs_whl = (ls crates/hiroz-msgs/python/dist/*.whl | first | get name)
    let py_whl   = (ls crates/hiroz-py/dist/*.whl | first | get name)
    print $"  pip install ($msgs_whl)"
    print $"  pip install ($py_whl)"
    print "──────────────────────────────────────────────────────────"
}

# Build hiroz-py and hiroz-msgs-py wheels locally (mirrors the release CI workflow).
#
# Examples:
#   ./scripts/build-python-wheels.nu                  # jazzy + humble
#   ./scripts/build-python-wheels.nu jazzy            # jazzy only
#   ./scripts/build-python-wheels.nu jazzy humble     # explicit list
#   ./scripts/build-python-wheels.nu --install jazzy  # build + pip install
def main [
    --install           # Install built wheels into crates/hiroz-py/.venv
    ...distros: string  # Distros to build (default: jazzy humble)
] {
    let distros = if ($distros | is-empty) { ["jazzy", "humble"] } else { $distros }
    for d in $distros { validate-distro $d }

    log-header "Building Python wheels" ($distros | str join ", ")

    # Clean previous build artefacts
    rm -rf crates/hiroz-py/dist crates/hiroz-msgs/python/dist

    build-msgs-wheel

    # Build each distro wheel, install before renaming (rename makes filename invalid for pip)
    let primary = ($distros | first)
    for distro in $distros {
        build-hiroz-py-wheel $distro
        # Install only the first (primary) distro into venv
        if $install and ($distro == $primary) {
            install-into-venv
        }
        # Rename immediately so next distro build doesn't conflict
        rename-wheel $distro
    }

    if not $install {
        print-install-hint
    }

    log-success "Done"
}
