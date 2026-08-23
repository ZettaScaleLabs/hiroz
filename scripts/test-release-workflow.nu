#!/usr/bin/env nu

# Smoke-test the release workflow without cutting a real release.
#
# Pushes a temporary prerelease tag to trigger the full release workflow
# (build → smoke-test → release), waits for CI to finish, then cleans up
# the tag and the draft GitHub Release that was created.
#
# Usage:
#   ./scripts/test-release-workflow.nu
#   ./scripts/test-release-workflow.nu --tag v0.0.0-smoke-test
#   ./scripts/test-release-workflow.nu --no-wait   # push tag only, skip polling
#   ./scripts/test-release-workflow.nu --cleanup   # delete tag + GH release only

use lib/common.nu *

const REPO = "ZettaScaleLabs/hiroz"
# The smoke tag must carry the CORE version the `hiroz-union` crate is at.
# `build-hu-release.nu` cross-checks the tag against that crate and fails the
# build on a mismatch, so the old hard-coded `v0.0.0-smoke-test` now kills four
# jobs at their first packaging step -- it removed the only documented rehearsal
# GitHub has. A `-smoke-test` suffix keeps it a pre-release, so it is published
# as one and skips crates.io.
def hu-core []: nothing -> string {
    open --raw Cargo.toml
    | lines
    | skip until { |l| ($l | str trim) == "[workspace.package]" }
    | where { |l| ($l | str trim | str starts-with "version") }
    | first
    | split row "\""
    | get 1
}

def cleanup-tag [tag: string] {
    log-step $"Deleting remote tag ($tag)"
    ^git push origin --delete $tag
    log-step $"Deleting local tag ($tag)"
    ^git tag -d $tag

    log-step "Deleting GitHub Release (if created)"
    let releases = (^gh api $"repos/($REPO)/releases" | from json)
    let matched = ($releases | where tag_name == $tag)
    if ($matched | is-empty) {
        print "  No GitHub Release found — nothing to delete."
    } else {
        let release_id = ($matched | first | get id)
        ^gh api -X DELETE $"repos/($REPO)/releases/($release_id)"
        print $"  Deleted release ($release_id)."
    }
}

# Smoke-test the release workflow by pushing a temporary prerelease tag.
def main [
    --tag: string = ""             # Tag to push (default: v<crate-version>-smoke-test; deleted on cleanup)
    --no-wait                      # Push the tag but do not poll for CI result
    --cleanup                      # Delete the tag and GitHub Release, then exit
] {
    let tag = if ($tag | is-empty) { $"v(hu-core)-smoke-test" } else { $tag }
    print $"Smoke tag: ($tag)"

    if $cleanup {
        cleanup-tag $tag
        log-success "Cleanup done."
        return
    }

    # Verify the tag doesn't already exist
    let existing = (^git tag -l $tag | str trim)
    if not ($existing | is-empty) {
        error make { msg: $"Tag ($tag) already exists locally. Delete it first: git tag -d ($tag)" }
    }

    log-header "Release workflow smoke test" $tag

    log-step $"Creating tag ($tag)"
    # `-a -m`, not a bare `git tag`. A user with `tag.gpgsign = true` in their
    # git config gets a SIGNED tag from the bare form, and a signed tag needs a
    # message -- so the script died with `fatal: no tag message?` before pushing
    # anything, on the very machine that cuts releases. This form works either
    # way: with signing configured the tag is signed and carries a message, and
    # without it the tag is annotated.
    ^git tag -a -m $"Release workflow rehearsal for ($tag). Temporary; deleted on cleanup." $tag

    log-step $"Pushing tag ($tag) → ($REPO)"
    ^git push origin $tag

    print $"\n  Workflow triggered: https://github.com/($REPO)/actions"
    print $"  GitHub Release [draft]: https://github.com/($REPO)/releases/tag/($tag)\n"

    if $no_wait {
        print "Skipping CI poll (--no-wait). When done, run:"
        print $"  ./scripts/test-release-workflow.nu --cleanup --tag ($tag)"
        return
    }

    log-step "Waiting for CI... (polling every 60s)"

    # Poll until GitHub registers the new run (filter out any old completed runs)
    mut run_id = ""
    mut attempts = 0
    loop {
        $attempts += 1
        if $attempts > 20 {
            error make { msg: "Timed out waiting for workflow run to appear." }
        }
        let runs = (
            ^gh run list --repo $REPO --branch $tag --workflow release.yml --json databaseId,status
            | from json
            | where status != "completed"
        )
        if not ($runs | is-empty) {
            $run_id = ($runs | first | get databaseId | into string)
            break
        }
        print "  Waiting for workflow run to register..."
        sleep 10sec
    }

    print $"  Run ID: ($run_id)"
    ^gh run watch $run_id --repo $REPO --exit-status
    let exit_code = $env.LAST_EXIT_CODE

    print ""
    if $exit_code == 0 {
        log-success "All jobs passed. Release workflow is healthy."
    } else {
        log-warning "One or more jobs failed. Check the logs above."
        print $"  https://github.com/($REPO)/actions/runs/($run_id)"
    }

    print ""
    log-step "Cleaning up test tag and release"
    cleanup-tag $tag

    if $exit_code != 0 {
        exit 1
    }
    log-success "Done."
}
