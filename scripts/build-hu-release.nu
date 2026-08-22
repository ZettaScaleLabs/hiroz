#!/usr/bin/env nu
# Produce the `hu` release artifact set into a dist directory.
#
# The single source of truth for what a `hu` release contains. Every release
# platform calls it, so they cannot drift apart in what they ship.
#
#   hu-<ver>-<target>.tar.gz    hu binary + LICENSE + install README
#   hu_meter-<ver>.wasm         wasm32-wasip2 — platform-independent
#   hu_monitor-<ver>.wasm
#   hu-plugins-<ver>.tar.gz     both plugins, for offline install
#   hu-plugins-<ver>.json       index that `hu plugin install <name>` resolves
#   install-hu.sh               the installer the release notes tell users to curl
#   SHA256SUMS                  covers every file above
#
# The plugins are the point: `hu meter` and `hu monitor` do not exist without
# them, because they are not built into the binary. `install-hu.sh` ships
# because the release notes tell users to curl it from the release.

const PLUGIN_DIR = "crates/hiroz-union/plugins"
const WIT_WORLD = "hu:plugin@0.1.0"
const INSTALLER = "scripts/install-hu.sh"

# Everything that ships. `name` is the subcommand (the `hu_`/`hu-` prefix is
# stripped at discovery), `dir` the crate directory, `stem` the .wasm filename.
const PLUGINS = [
    [name, dir, stem, world, description];
    ["meter" "hu-meter" "hu_meter" "hu-cli-plugin" "Topic rate, bandwidth, echo, publish, latency, graph and parameter introspection"]
    ["monitor" "hu-monitor" "hu_monitor" "hu-cli-plugin" "Live graph events, graph snapshots, /rosout tailing and logger levels"]
]

# `hiroz-union` inherits the workspace version (`version.workspace = true`), so
# read the workspace, not the crate. Reading the crate file used to work because
# it carried a literal version -- which meant `hu` could silently drift from the
# rest of the workspace, and a `v0.2.0` tag against a `0.1.0` crate failed the
# whole release at packaging. One version now governs both.
#
# Note the crate file no longer contains a quoted version at all, so the old
# `split row '"' | get 1` on it raises rather than returning a wrong answer.
def crate-version [] {
    open --raw Cargo.toml
    | lines
    | skip until { |l| ($l | str trim) == "[workspace.package]" }
    | where { |l| ($l | str trim | str starts-with "version") }
    | first
    | split row "\""
    | get 1
}

def sha256-of [path: string] {
    open --raw $path | hash sha256
}

# Honour CARGO_TARGET_DIR — CI often redirects it, and assuming ./target makes
# the script silently look for artifacts that were written elsewhere.
def target-root [] {
    $env.CARGO_TARGET_DIR? | default "target"
}

def main [
    --target: string = ""       # rust target triple; empty = host
    --out: string = "dist"      # output directory
    --version: string = ""      # expected version; must match the crate
    --plugins-only               # skip the host binary (for non-primary legs)
    --binary-only                # skip the plugins (they ship from one leg only)
    --binary-from: string = ""  # package this prebuilt hu instead of running cargo
    --no-validate                # skip `hu plugin validate` (cross-compiled legs)
    --no-sums                    # skip SHA256SUMS; the caller assembles one
] {
    let ver = (crate-version)

    # A tag that disagrees with the crate would mislabel every asset. Fail
    # loudly rather than shipping `hu-0.2.0-...tar.gz` containing 0.1.0.
    #
    # Only the CORE version has to match. A semver pre-release suffix
    # (`0.1.0-rc1`) is legitimate: it names a rehearsal of 0.1.0, built from
    # the 0.1.0 source, and the assets it produces are 0.1.0 assets. Requiring
    # an exact match here is what previously left no way to exercise the
    # release pipeline except by publishing a real release and deleting it
    # afterwards — a workaround, not a test.
    let core = ($version | split row "-" | first)
    if $version != "" and $core != $ver {
        print $"FAIL: requested version ($version) has core ($core), but the hiroz-union crate is ($ver)"
        exit 1
    }
    if $version != "" and $core != $version {
        print $"  pre-release ($version) — assets are named for the core version ($ver)"
    }

    mkdir $out
    print $"hu release ($ver) → ($out)/"

    if not $plugins_only {
        build-binary $ver $target $out $binary_from
    }
    if not $binary_only {
        build-plugins $ver $out (not $no_validate)
        # Staged on the same leg as the plugins, and deliberately not on a
        # `--binary-only` one. Both are release-wide, platform-independent
        # assets: there is exactly one correct copy per release, and the GitHub
        # channel runs `--binary-only` once per target triple (three legs) and
        # `--plugins-only` once. Staging unconditionally would have three legs
        # racing to upload the same `install-hu.sh` under the same asset name —
        # the same reason the plugins themselves ship from one leg only.
        stage-installer $out
    }

    # A SHA256SUMS covering only part of a release is worse than none: it
    # verifies clean while saying nothing about the assets it omits, and
    # `install-hu.sh` treats an unlisted file as a refusal. So when a caller
    # assembles a release from several jobs, it must pass --no-sums here and
    # generate one file over the complete set.
    if $no_sums {
        print "  (SHA256SUMS skipped — caller assembles it over the full asset set)"
    } else {
        write-sums $out
    }
    print ""
    ls $out | select name size | print
}

def build-binary [ver: string, target: string, out: string, binary_from: string] {
    # `--binary-from` packages a binary someone else built. It exists because
    # the cross legs need `cargo zigbuild`, not `cargo build`, and pushing that
    # knowledge in here would mean this script had to model every leg's build.
    # What actually has to be shared is the *packaging* — the tarball name and
    # contents — because that is the contract `install-hu.sh` reads. Letting a
    # leg build however it likes and package through here is what keeps the
    # channels from drifting.
    let bin = if $binary_from != "" {
        if not ($binary_from | path exists) {
            print $"FAIL: --binary-from ($binary_from) does not exist"
            exit 1
        }
        print $"packaging prebuilt hu from ($binary_from)"
        $binary_from
    } else {
        # `web-plugins` is deliberate: docs/tools/hu.md documents `hu web`, and
        # a default-feature build does not have that subcommand at all.
        # Shipping the default build means shipping a binary that fails a
        # documented command.
        let target_args = (if $target == "" { [] } else { ["--target" $target] })
        print $"building hu \(--features web-plugins\) ($target)"
        (^cargo build --release -p hiroz-union --bin hu --features web-plugins ...$target_args)

        let root = (target-root)
        let bin_dir = (if $target == "" { $"($root)/release" } else { $"($root)/($target)/release" })
        $"($bin_dir)/hu"
    }

    if not ($bin | path exists) {
        print $"FAIL: expected binary at ($bin), not found"
        exit 1
    }

    let triple = (if $target == "" { host-triple } else { $target })
    let stage = $"($out)/.stage-hu"
    rm -rf $stage
    mkdir $stage
    cp $bin $"($stage)/hu"
    cp LICENSE $"($stage)/LICENSE"
    install-readme $ver | save --force $"($stage)/README-install.md"

    let tar = $"($out)/hu-($ver)-($triple).tar.gz"
    ^tar -czf $tar -C $stage hu LICENSE README-install.md
    rm -rf $stage
    print $"  → ($tar)"
}

def host-triple [] {
    ^rustc -vV | lines | where { |l| $l | str starts-with "host: " } | first | str replace "host: " ""
}

def build-plugins [ver: string, out: string, validate: bool] {
    print "building WASM plugins (wasm32-wasip2)"
    for p in $PLUGINS {
        (^cargo build --release --target wasm32-wasip2
            --manifest-path $"($PLUGIN_DIR)/($p.dir)/Cargo.toml")
    }

    # The plugins are their own workspace, so their artifacts land under the
    # plugins dir — unless CARGO_TARGET_DIR redirects everything to one root,
    # which is what CI does. Accept either.
    let wasm_dir = $"($PLUGIN_DIR)/target/wasm32-wasip2/release"
    let alt_dir = $"(target-root)/wasm32-wasip2/release"

    mut entries = []
    let stage = $"($out)/.stage-plugins"
    rm -rf $stage
    mkdir $stage

    for p in $PLUGINS {
        let src = (
            [$"($wasm_dir)/($p.stem).wasm" $"($alt_dir)/($p.stem).wasm"]
            | where { |c| $c | path exists }
            | first
        )
        if ($src | is-empty) {
            print $"FAIL: ($p.stem).wasm not found in ($wasm_dir) or ($alt_dir)"
            exit 1
        }

        # A plugin that does not compile as a component must never ship. Use a
        # host-native `hu` — on a cross leg the freshly built binary cannot run
        # here, which is what --no-validate is for.
        if $validate {
            let hu = $"(target-root)/release/hu"
            if ($hu | path exists) {
                let r = (do { ^$hu plugin validate $src } | complete)
                if $r.exit_code != 0 {
                    print $"FAIL: ($src) did not validate as a WASM component"
                    print $r.stderr
                    exit 1
                }
                print $"  validated ($p.stem)"
            } else {
                print $"FAIL: --no-validate not given but no host-native hu at ($hu)"
                exit 1
            }
        }

        let dest = $"($out)/($p.stem)-($ver).wasm"
        cp $src $dest
        cp $src $"($stage)/($p.stem).wasm"
        print $"  → ($dest)"

        $entries = ($entries | append {
            name: $p.name
            file: $"($p.stem)-($ver).wasm"
            version: $ver
            sha256: (sha256-of $dest)
            world: $p.world
            description: $p.description
        })
    }

    let tar = $"($out)/hu-plugins-($ver).tar.gz"
    ^tar -czf $tar -C $stage ...($PLUGINS | each { |p| $"($p.stem).wasm" })
    rm -rf $stage
    print $"  → ($tar)"

    let index = {
        schema: 1
        hu_version: $ver
        wit_world: $WIT_WORLD
        plugins: $entries
    }
    $index | to json --indent 2 | save --force $"($out)/hu-plugins-($ver).json"
    print $"  → ($out)/hu-plugins-($ver).json"
}

# Ship the installer itself as a release asset.
#
# It is copied, not generated, so what a user curls is byte-for-byte the script
# in the repo at the tagged commit — and `SHA256SUMS` (written afterwards over
# the whole directory) covers it like any other asset, so a caller passing
# --no-sums still gets it listed when it assembles the sums over the full set.
def stage-installer [out: string] {
    if not ($INSTALLER | path exists) {
        print $"FAIL: ($INSTALLER) not found — the release notes tell users to curl it"
        exit 1
    }

    # A release whose headline command is a syntactically broken shell script
    # is worse than one with no installer at all: the failure lands on the
    # user's machine, mid-install. `sh -n` parses without executing, so this
    # costs nothing and runs on every leg that ships the file.
    let syn = (do { ^sh -n $INSTALLER } | complete)
    if $syn.exit_code != 0 {
        print $"FAIL: ($INSTALLER) is not valid POSIX shell"
        print $syn.stderr
        exit 1
    }

    let dest = $"($out)/install-hu.sh"
    cp $INSTALLER $dest
    ^chmod 755 $dest
    if ((ls $dest | first | get size) == 0b) {
        print $"FAIL: staged ($dest) is empty"
        exit 1
    }
    print $"  → ($dest)"
}

def write-sums [out: string] {
    # `sha256sum -c` format. SHA256SUMS cannot cover itself.
    let sums = (
        ls $out
        | where type == file
        | get name
        | each { |f| $f | path basename }
        | where { |f| $f != "SHA256SUMS" }
        | sort
        | each { |f| $"(sha256-of $"($out)/($f)")  ($f)" }
        | str join "\n"
    )
    $"($sums)\n" | save --force $"($out)/SHA256SUMS"
    print $"  → ($out)/SHA256SUMS"
}

def install-readme [ver: string] {
    $"# hu ($ver)

`hu` is the command-line companion to the hiroz ROS 2 stack. It needs no ROS 2
install and no daemon — only a reachable Zenoh router.

## Install

Copy the binary somewhere on your PATH:

    install -Dm755 hu ~/.local/bin/hu

## Plugins

`hu meter` and `hu monitor` are WASM plugins, not built into this binary. They
ship as a separate `hu-plugins-($ver).tar.gz`. Install them with:

    hu plugin install ./hu_meter-($ver).wasm
    hu plugin install ./hu_monitor-($ver).wasm

or extract the plugins tarball into `~/.local/share/hu/plugins/`. Verify with:

    hu plugin list

If that list is empty, `hu meter` and `hu monitor` will not work.

## Verify this download

`SHA256SUMS` covers every asset in the release, and this archive holds only
some of them. A bare `sha256sum -c` therefore reports the absent ones as
failures. Check the files you actually downloaded:

    sha256sum --ignore-missing -c SHA256SUMS

Older coreutils has no `--ignore-missing`. Check one file instead:

    grep <the-file-you-downloaded> SHA256SUMS | sha256sum -c -

## Documentation

https://zettascalelabs.github.io/hiroz/
"
}
