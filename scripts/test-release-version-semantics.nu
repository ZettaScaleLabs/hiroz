#!/usr/bin/env nu
# Release version semantics: a tag is three different strings, and every
# release channel must use the right one at each site.
#
#   release identity   tag/release/download path   0.2.0-rc1
#   asset core         artifact file names         0.2.0
#   binary version     `hu --version`               hu 0.2.0
#
# This test needs no runner, network, or build.

const ROOT = path self ..
const GH = path self ../.github/workflows/release.yml
const INSTALLER = path self install-hu.sh
const REHEARSAL = path self test-release-workflow.nu

def --env ok [message: string] {
    $env.RELEASE_TEST_PASS = (($env.RELEASE_TEST_PASS | into int) + 1 | into string)
    print $"  ok   ($message)"
}

def --env bad [message: string] {
    $env.RELEASE_TEST_FAIL = (($env.RELEASE_TEST_FAIL | into int) + 1 | into string)
    print $"  FAIL ($message)"
}

def --env check [description: string, expected: any, actual: any] {
    if $expected == $actual {
        ok $description
    } else {
        bad $description
        print $"         expected: ($expected)\n         actual:   ($actual)"
    }
}

def --env must-contain [text: string, description: string, needle: string] {
    if ($text | str contains $needle) {
        ok $description
    } else {
        bad $"($description) — text absent: ($needle)"
    }
}

def --env must-not-contain [text: string, description: string, needle: string] {
    if ($text | str contains $needle) {
        bad $"($description) — text present: ($needle)"
    } else {
        ok $description
    }
}

def --env must-match [text: string, description: string, pattern: string] {
    if ($text =~ $pattern) {
        ok $description
    } else {
        bad $"($description) — pattern absent: ($pattern)"
    }
}

def --env must-not-match [text: string, description: string, pattern: string] {
    if ($text =~ $pattern) {
        bad $"($description) — pattern present: ($pattern)"
    } else {
        ok $description
    }
}

def release-id [tag: string, prefix: string]: nothing -> string {
    $tag | str substring ($prefix | str length)..
}

def asset-core [release: string]: nothing -> string {
    $release | split row "-" | first
}

def is-prerelease [release: string]: nothing -> bool {
    $release | str contains "-"
}

def path-version-dependencies [manifest: record]: nothing -> table {
    $manifest
    | transpose name value
    | where { |dependency|
        (($dependency.value | describe | str starts-with "record")
            and ("path" in $dependency.value)
            and ("version" in $dependency.value))
    }
}

def line-number [text: string, needle: string]: nothing -> int {
    let found = ($text | lines | enumerate | where { |line| $line.item | str contains $needle })
    if ($found | is-empty) { 0 } else { ($found | first).index + 1 }
}

# Read the real derivation blocks from release.yml and evaluate the two value
# expressions they are allowed to contain. Unknown expressions evaluate to an
# empty string and fail, rather than silently becoming an untested new form.
def workflow-derivations [workflow: string, reference: string]: nothing -> table {
    let lines = ($workflow | lines)
    let starts = ($lines | enumerate | where { |line| ($line.item | str trim) == 'V="${GITHUB_REF_NAME#v}"' })
    $starts | each { |start|
        let block = ($lines | skip $start.index | first 8 | each { str trim })
        let version_line = ($block | where { str contains "HU_VERSION=" } | first | default "")
        let core_line = ($block | where { str contains "HU_CORE=" } | first | default "")
        let parsed_version = if ($reference | str starts-with "v") {
            $reference | str substring 1..
        } else {
            ""
        }
        let version = if ($version_line | str contains 'HU_VERSION=$V') { $parsed_version } else { "" }
        let core = if ($core_line | str contains 'HU_CORE=${V%%-*}') {
            asset-core $version
        } else if ($core_line | str contains 'HU_CORE=$V') {
            $version
        } else {
            ""
        }
        { version: $version, core: $core }
    }
}

def --env check-workflow-derivation [workflow: string, description: string, reference: string, wanted_version: string, wanted_core: string] {
    let derived = (workflow-derivations $workflow $reference)
    if ($derived | is-empty) {
        bad $"($description) — no derivation block found"
        return
    }
    let total = ($derived | length)
    for item in ($derived | enumerate) {
        let label = $"($description) [block ($item.index + 1)/($total)]"
        if $item.item.version == $wanted_version and $item.item.core == $wanted_core {
            ok $"($label) — HU_VERSION=($item.item.version) HU_CORE=($item.item.core)"
        } else {
            bad $"($label) — got HU_VERSION='($item.item.version)' HU_CORE='($item.item.core)', wanted '($wanted_version)' / '($wanted_core)'"
        }
    }
}

def --env version-location-bad [location: string, found: string, wanted: string] {
    bad $"R1ASSERT version_locations_agree: ($location) is ($found), wanted ($wanted)"
}

def main [] {
    $env.RELEASE_TEST_PASS = "0"
    $env.RELEASE_TEST_FAIL = "0"

    let workflow = (open --raw $GH)
    let installer = (open --raw $INSTALLER)
    let rehearsal = (open --raw $REHEARSAL)

    print "== version semantics per tag =="
    let cases = [
        [tag prefix wanted_id wanted_core wanted_version wanted_prerelease];
        [v0.2.0 v 0.2.0 0.2.0 "hu 0.2.0" false]
        [v0.2.0-rc1 v 0.2.0-rc1 0.2.0 "hu 0.2.0" true]
        [v0.2.0-beta2 v 0.2.0-beta2 0.2.0 "hu 0.2.0" true]
        [hu-v0.1.0 hu-v 0.1.0 0.1.0 "hu 0.1.0" false]
        [hu-v0.1.0-rc1 hu-v 0.1.0-rc1 0.1.0 "hu 0.1.0" true]
    ]
    mut rows_ok = true
    for case in $cases {
        let identity = (release-id $case.tag $case.prefix)
        let core = (asset-core $identity)
        let prerelease = (is-prerelease $identity)
        print $"($case.tag) -> id=($identity) core=($core) pre=($prerelease)"
        if ($identity != $case.wanted_id
            or $core != $case.wanted_core
            or $"hu ($core)" != $case.wanted_version
            or $prerelease != $case.wanted_prerelease) {
            $rows_ok = false
        }
    }
    if $rows_ok { ok "table rows derive id/core/version/prerelease as documented" } else { bad "a table row derived the wrong id/core/version/prerelease" }

    print "\n== the two tag shapes differ in exactly the documented way =="
    let final_id = (release-id v0.2.0 v)
    let final_core = (asset-core $final_id)
    let rc_id = (release-id v0.2.0-rc1 v)
    let rc_core = (asset-core $rc_id)
    check "a normal tag: identity equals core" $final_id $final_core
    if $rc_id == $rc_core { bad "a pre-release tag: identity must not equal core" } else { ok $"a pre-release tag: identity (($rc_id)) differs from core (($rc_core))" }
    check "both tags name the same assets" $final_core $rc_core
    check "both tags produce the same binary version" $"hu ($final_core)" $"hu ($rc_core)"

    print "\n== the asset set an rc tag actually produces =="
    for asset in [
        $"hu-($rc_core)-x86_64-unknown-linux-gnu.tar.gz"
        $"hu_meter-($rc_core).wasm"
        $"hu_monitor-($rc_core).wasm"
        $"hu-plugins-($rc_core).tar.gz"
        $"hu-plugins-($rc_core).json"
    ] {
        if ($asset | str contains $rc_id) { bad $"asset ($asset) must not carry the pre-release suffix" } else { ok $"asset named for the core version: ($asset)" }
    }

    print "\n== .github/workflows/release.yml =="
    must-contain $workflow "defines HU_CORE alongside HU_VERSION" "HU_CORE="
    must-contain $workflow 'plugin verify loop checks hu_meter-$HU_CORE' 'hu_meter-$HU_CORE.wasm'
    must-not-match $workflow "plugin verify loop never checks a HU_VERSION-named asset" 'hu_[a-z]*-\$HU_VERSION'
    must-not-contain $workflow "plugins tarball/index never checked under HU_VERSION" 'hu-plugins-$HU_VERSION'
    must-contain $workflow "release-install asserts the CORE version" 'test "$got" = "hu $CORE"'
    must-not-contain $workflow "release-install does not assert the tag version" 'test "$got" = "hu $VER"'
    must-contain $workflow "binary packaging arms the tag-vs-crate guard" '--version "$HU_VERSION"'
    must-contain $workflow "plugin build arms the tag-vs-crate guard" 'plugins-only --version "$HU_VERSION"'
    must-not-contain $workflow "plugin build no longer skips component validation" "--no-validate"
    must-contain $workflow "a host-native hu is built so validation can run" "cargo build --release --bin hu --package hiroz-union"
    must-contain $workflow "the release step sets prerelease from the tag shape" "prerelease: "
    must-contain $workflow "publish-crates is guarded against pre-release tags" "if: ${{ !contains(github.ref_name, '-') }}"
    must-contain $workflow "prerelease is derived from the semver hyphen, not an rc/alpha/beta list" "prerelease: ${{ contains(github.ref_name, '-') }}"

    print "\n== scripts/install-hu.sh =="
    must-contain $installer "splits the requested version into a core version" 'CORE="${VERSION%%-*}"'
    must-contain $installer "downloads core-named assets" 'hu-$CORE-$TARGET.tar.gz'
    must-contain $installer "the offline path selects the tarball by platform" 'hu-*-"$TARGET".tar.gz'
    must-not-contain $installer "no lexical tarball pick remains" 'ls "$SRC"/hu-*-*.tar.gz 2>/dev/null | grep -v -- .-plugins-. | head -n1'

    print "\n== D1: the documented installer URL is actually published =="
    must-contain $workflow "the release job stages install-hu.sh into dist/" "cp scripts/install-hu.sh dist/install-hu.sh"
    must-contain $workflow "staging parse-checks the installer before publishing it" "sh -n scripts/install-hu.sh"
    must-contain $workflow "SHA256SUMS coverage of install-hu.sh is asserted" "install-hu.sh is not covered by SHA256SUMS"
    must-contain $workflow "the published installer URL is fetched back and compared" 'curl -fsSL "$HU_RELEASE_BASE/install-hu.sh"'
    must-contain $workflow "the fetched installer is diffed against the tagged source" "diff -u scripts/install-hu.sh fetched-install-hu.sh"
    let stage_line = (line-number $workflow "cp scripts/install-hu.sh dist/install-hu.sh")
    let sums_line = (line-number $workflow "Assemble SHA256SUMS over the complete asset set")
    if $stage_line > 0 and $sums_line > 0 and $stage_line < $sums_line { ok $"install-hu.sh is staged before SHA256SUMS is assembled: line ($stage_line) < ($sums_line)" } else { bad $"install-hu.sh must be staged before SHA256SUMS: stage=($stage_line) sums=($sums_line)" }
    let syntax = (do { ^sh -n $INSTALLER } | complete)
    if $syntax.exit_code == 0 { ok "scripts/install-hu.sh parses (sh -n)" } else { bad "scripts/install-hu.sh does not parse (sh -n)" }

    print "\n== D7: the release is checked against its own docs, not just filenames =="
    must-contain $workflow "runs the docs-reproduction suite after installing from the release" "nu scripts/test-hu-docs-repro.nu"
    must-contain $workflow "the suite is pointed at the installed-from-release HOME" '--home "$HUHOME"'
    must-contain $workflow "--require-traffic, so an empty graph fails instead of passing" "--require-traffic"
    must-contain $workflow "a traffic fixture is built from source, not taken from the artifact" "cargo build --release --example z_pubsub -p hiroz"
    must-contain $workflow "a router is started from the installed binary" '"$HUHOME/.local/bin/hu" router'
    must-contain $workflow "the suite exit status is captured, not piped away" 'test "$rc" -eq 0'

    print "\n== the workflows' own derivation, evaluated =="
    check-workflow-derivation $workflow "GitHub: a pre-release tag splits release id from asset core" v0.2.0-rc1 0.2.0-rc1 0.2.0
    check-workflow-derivation $workflow "GitHub: a normal tag makes the two identical" v0.2.0 0.2.0 0.2.0

    print "\n== every workspace crate inherits one version =="
    let stray = (glob $"($ROOT)/crates/*/Cargo.toml" | each { |file|
        let package = (open $file | get package? | default {})
        let version = ($package | get version? | default null)
        if $version == null or (($version | describe | str starts-with "record") and ($version.workspace? | default false)) { null } else { $file | path dirname | path basename }
    } | compact)
    if ($stray | is-empty) { ok "no crate carries a literal version" } else { bad $"these crates carry a literal version instead of version.workspace: ($stray | str join ' ')" }

    print "\n== the release is not public until it verifies =="
    check "both jobs checksum through the shared script" 2 ($workflow | split row "write-sha256sums.sh" | length | $in - 1)
    must-not-contain $workflow "no inline checksum assembly remains" "xargs sha256sum"
    must-contain $workflow "the release is created as a draft" "draft: true"
    must-contain $workflow "a job promotes the draft" "--draft=false"
    must-contain $workflow "a job withdraws it again" "--draft=true"
    must-not-contain $workflow "the withdraw job does not use a bare failure()" 'if: ${{ failure() }}'
    must-contain $workflow "the withdraw job requires a successful publish" "needs.publish-release.result == 'success'"
    must-contain $workflow "the withdraw job fires on any non-success download test" "needs.smoke-test-release-install.result != 'success'"
    must-not-contain $workflow "the withdraw job does not test equality with failure" "needs.smoke-test-release-install.result == 'failure'"
    must-contain $workflow "the download test waits for the promotion" "needs: [publish-release]"
    must-contain $workflow "crates.io is gated behind the download test" "needs: [smoke-test-release-install]"
    must-match $rehearsal "the rehearsal tag carries a message" '(?m)^\s*\^git tag -a -m'
    must-not-match $rehearsal "the rehearsal does not create a bare tag" '(?m)^\s*\^git tag \$tag\s*$'

    print "\n== version locations agree =="
    let root_manifest = (open $"($ROOT)/Cargo.toml")
    let workspace_version = ($root_manifest.workspace.package.version | into string)
    ok $"workspace version read: ($workspace_version)"
    let expected = ($env.EXPECT_VERSION? | default "")
    if $expected != "" and $workspace_version != $expected {
        version-location-bad "Cargo.toml [workspace.package] version" $workspace_version $"($expected), from EXPECT_VERSION"
    }

    let root_dependencies = (path-version-dependencies $root_manifest.workspace.dependencies | each { |dependency| $dependency | insert location "Cargo.toml" })
    let hiroz_manifest = (open $"($ROOT)/crates/hiroz/Cargo.toml")
    let hiroz_dependencies = (path-version-dependencies $hiroz_manifest.dependencies | each { |dependency| $dependency | insert location "crates/hiroz/Cargo.toml" })
    let dependencies = ($root_dependencies | append $hiroz_dependencies)
    for dependency in $dependencies {
        let found = ($dependency.value.version | into string)
        if $found != $workspace_version { version-location-bad $"($dependency.location) dependency ($dependency.name)" $found $workspace_version }
    }
    if ($dependencies | length) == 8 { ok "read 8 path dependency version strings" } else { version-location-bad "path dependency version strings found" ($dependencies | length | into string) "exactly 8" }

    let msgs_project = (open $"($ROOT)/crates/hiroz-msgs/python/pyproject.toml").project
    let msgs_init = (open --raw $"($ROOT)/crates/hiroz-msgs/python/hiroz_msgs_py/__init__.py")
    let py_project = (open $"($ROOT)/crates/hiroz-py/pyproject.toml").project
    let py_lock = (open --raw $"($ROOT)/crates/hiroz-py/uv.lock" | from toml)
    for wheel in [
        { location: "crates/hiroz-msgs/python/pyproject.toml version", version: ($msgs_project.version | into string) }
        { location: "crates/hiroz-msgs/python/hiroz_msgs_py/__init__.py __version__", version: ($msgs_init | lines | where { str starts-with "__version__ = " } | first | default "" | split row '"' | get 1? | default "") }
        { location: "crates/hiroz-py/pyproject.toml version", version: ($py_project.version | into string) }
        { location: "crates/hiroz-py/uv.lock hiroz-py version", version: ($py_lock.package | where name == "hiroz-py" | first | default { version: "" }).version }
    ] {
        if $wheel.version != $workspace_version { version-location-bad $wheel.location $wheel.version $workspace_version }
    }
    let bound = ($py_project.dependencies | where { str starts-with "hiroz-msgs-py>=" } | first | default "" | str replace "hiroz-msgs-py>=" "")
    if $bound != $workspace_version { version-location-bad "crates/hiroz-py/pyproject.toml hiroz-msgs-py bound" (if $bound == "" { "absent" } else { $bound }) $workspace_version }

    if ($env.RELEASE_TEST_FAIL | into int) == 0 { ok $"version_locations_agree: every location reads ($workspace_version)" }
    print "  note: the hu package version in flake.nix is not covered"
    print $"-- ($env.RELEASE_TEST_PASS) passed, ($env.RELEASE_TEST_FAIL) failed"
    if ($env.RELEASE_TEST_FAIL | into int) != 0 { exit 1 }
}
