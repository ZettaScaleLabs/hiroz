#!/usr/bin/env nu
# Check that cliff.toml marks breaking commits in both template loops.
# Usage: scripts/test-cliff-breaking.nu [CLIFF_TOML]
# Needs git-cliff on PATH, or GIT_CLIFF set to its path. No token or network needed.
# Exit codes: 0 = pass, 1 = assertion failed, 2 = no verdict.

const TEST = "cliff_marks_a_breaking_commit"
const ROOT = path self | path dirname | path dirname

def git-checked [...args: string] {
    let result = (^git ...$args | complete)
    if $result.exit_code != 0 {
        error make {msg: $"git ($args | str join ' ') failed: ($result.stderr)"}
    }
}

def cliff-command [] {
    let command = ($env.GIT_CLIFF? | default --empty "git-cliff")
    if ($command | str contains '/') {
        $command | path expand
    } else {
        which $command | first | get path
    }
}

# Resolve both paths before entering the fixture repository.
def render [config: path, repo: path] {
    let config = ($config | path expand)
    let cliff = (cliff-command)
    cd $repo
    let result = (run-external $cliff '--config' $config '--unreleased' '--offline' | complete)
    print $result.stdout
    if ($result.stderr | is-not-empty) { print $result.stderr }
    print $"CLIFF=($result.exit_code)"
    if $result.exit_code != 0 {
        error make {msg: $"git-cliff exited ($result.exit_code)"}
    }
    $result.stdout
}

def check [output: string] {
    let cases = [
        [message breaking];
        ["Breaking scoped" true]
        ["Breaking unscoped" true]
        ["Footer break" true]
        ["Breaking chore" true]
        ["Breaking ci" true]
        ["Breaking build" true]
        ["Ordinary scoped" false]
        ["Ordinary unscoped" false]
    ]
    mut failed = false
    for case in $cases {
        let matches = ($output | lines | where { str contains $case.message })
        if ($matches | is-empty) {
            error make {msg: $"no line for '($case.message)'"}
        }
        let marked = ($matches | first | str contains "BREAKING")
        if $marked != $case.breaking {
            let reason = if $case.breaking { "no breaking mark on" } else { "mark on ordinary" }
            print $"R1ASSERT ($TEST) FAIL: ($reason) '($case.message)'"
            $failed = true
        } else {
            let marker = if $case.breaking { "MARK_OK" } else { "CONTROL_OK" }
            print $"($marker) ($case.message)"
        }
    }
    if ($output | str contains "Plain chore") {
        print $"R1ASSERT ($TEST) FAIL: plain chore appears"
        $failed = true
    } else {
        print "CONTROL_OK Plain chore skipped"
    }
    $failed
}

def exercise [config: path, scratch: path] {
    let repo = ($scratch | path join "repo")
    mkdir $repo
    do {
        cd $repo
        $env.GIT_AUTHOR_NAME = "t"
        $env.GIT_AUTHOR_EMAIL = "t@example.invalid"
        $env.GIT_COMMITTER_NAME = "t"
        $env.GIT_COMMITTER_EMAIL = "t@example.invalid"
        git-checked init '-q' .
        # Synthetic commits need no access to the caller's signing key.
        git-checked config commit.gpgsign 'false'
        for message in [
            "fix(a): ordinary scoped" "fix: ordinary unscoped"
            "fix(x)!: breaking scoped" "fix!: breaking unscoped"
            "chore!: breaking chore" "ci(c)!: breaking ci"
            "build!: breaking build" "chore: plain chore"
        ] {
            git-checked commit '-q' '--allow-empty' '-m' $message
        }
        git-checked commit '-q' '--allow-empty' '-m' "fix(f): footer break" '-m' "BREAKING CHANGE: x"
    }
    mut failed = (check (render $config $repo))

    # Reuse the same fixture and assertions with caller-relative paths.
    let relative = ($scratch | path join "relative")
    mkdir ($relative | path join "bin")
    cp $config ($relative | path join "cliff.toml")
    let cliff = (cliff-command)
    let link = (^ln -s $cliff ($relative | path join "bin/git-cliff") | complete)
    if $link.exit_code != 0 { error make {msg: $link.stderr} }
    let relative_failed = try {
        cd $relative
        $env.GIT_CLIFF = "./bin/git-cliff"
        check (render "cliff.toml" $repo)
    } catch { |err|
        print $"R1ASSERT ($TEST) FAIL: relative config or GIT_CLIFF path: ($err.msg)"
        true
    }
    if $relative_failed { $failed = true } else { print "CONTROL_OK relative paths" }
    let code = if $failed { 1 } else { 0 }
    print $"TEST=($code) ($TEST)"
    $code
}

def main [config: path = ($ROOT | path join "cliff.toml")] {
    let config = ($config | path expand)
    let scratch_root = ($ROOT | path join "_tmp")
    mkdir $scratch_root
    let scratch = (mktemp -d -p $scratch_root "cliff-breaking.XXXXXX")
    let code = try {
        exercise $config $scratch
    } catch { |err|
        print $"R1ASSERT ($TEST) UNKNOWN: ($err.msg)"
        2
    }
    rm -rf $scratch
    exit $code
}
