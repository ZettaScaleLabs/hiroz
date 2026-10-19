#!/usr/bin/env nu
# Check the raw suite guard of scripts/test-pure-rust.nu against a stub `cargo`. No build, no network.
# Usage: scripts/test-raw-suite-guard.nu
# The stub lists the raw suite only when the feature list it receives names `raw-sub-tests`, as cargo does.
# Exit codes: 0 = pass, 1 = assertion failed.

const SCRIPTS = path self | path dirname
const SOURCE = path self | path dirname | path join test-pure-rust.nu

const STUB = '#!/usr/bin/env bash
echo "$*" >> "$STUB_LOG"
[ "${STUB_MODE:-}" = fail ] && exit 1
case "$*" in
  *"nextest list"*raw-sub-tests*) for i in $(seq 1 "$STUB_COUNT"); do echo "hiroz-tests::buffer_cpu_group_raw case_$i"; done ;;
  *"nextest list"*) echo "hiroz-tests::other_suite case_1" ;;
esac
exit 0
'

# Run the script at `script` with the stub first on PATH. Returns the `complete` record and the stub log.
def run-guard [script: path, dir: path, mode: string, args: list<string>, count_delta: int = 0] {
    let log = ($dir | path join cargo.log)
    rm -f $log
    # `count_delta` is added to the stub count: 0 gives the real count of the suite, another value a wrong one.
    let count = ((open --raw $SOURCE | parse --regex 'const RAW_SUITE_TESTS = (\d+)' | get capture0.0 | into int) + $count_delta)
    let result = (with-env {PATH: ($env.PATH | prepend ($dir | path join bin)), STUB_LOG: $log, STUB_MODE: $mode, STUB_COUNT: $count} {
        ^$nu.current-exe $script ...$args | complete
    })
    {result: $result, log: (if ($log | path exists) { open --raw $log } else { "" })}
}

def verdict [name: string, ok: bool, detail: string] {
    print $"TEST ($name)=(if $ok { 'PASS' } else { 'FAIL' }) ($detail)"
    $ok
}

def main [] {
    let dir = (mktemp --directory)
    mkdir ($dir | path join bin)
    $STUB | save --force ($dir | path join bin cargo)
    ^chmod +x ($dir | path join bin cargo)
    # The script finds lib/common.nu next to itself.
    ^ln -s ($SCRIPTS | path join lib) ($dir | path join lib)

    # A copy of the script with `raw-sub-tests` removed from every line except the guard's own
    # `nextest list` line: the shared constant drifts, and every command that reads it drifts with it.
    let mutant = ($dir | path join mutant.nu)
    let lines = (open --raw $SOURCE | lines)
    let mutated = ($lines | each {|l| if ($l | str contains 'nextest list') { $l } else { $l | str replace --all ',raw-sub-tests' '' } })
    $mutated | str join (char newline) | save --force $mutant
    let changed = ($lines | zip $mutated | where {|p| $p.0 != $p.1} | length)

    mut ok = true

    let control = (run-guard $SOURCE $dir normal [check-raw-suite-listed])
    $ok = ((verdict guard_passes_on_the_unmutated_script ($control.result.exit_code == 0) $"rc=($control.result.exit_code)") and $ok)

    let drift = (run-guard $mutant $dir normal [check-raw-suite-listed])
    $ok = ((verdict guard_fails_when_the_shared_constant_drops_the_feature ($changed > 0 and $drift.result.exit_code != 0 and ($drift.result.stdout | str contains "feature is off")) $"changed=($changed) rc=($drift.result.exit_code)") and $ok)

    # `run-tests` spells its own feature list on one line and the constant is untouched: drift the constant cannot show.
    let spell = {|features| $lines | each {|l| if ($l | str contains 'nextest run --no-fail-fast -p hiroz-tests') { $l | str replace '($RAW_FEATURES)' $features } else { $l } } }
    let literal = ($dir | path join literal.nu)
    let spelled = (do $spell "ros-msgs,jazzy")
    $spelled | str join (char newline) | save --force $literal
    let spelled_changed = ($lines | zip $spelled | where {|p| $p.0 != $p.1} | length)
    let hardcoded = (run-guard $literal $dir normal [check-raw-suite-listed])
    let hardcoded_text = $"($hardcoded.result.stdout)($hardcoded.result.stderr)"
    $ok = ((verdict guard_fails_when_run_tests_drops_the_feature_in_its_own_literal ($spelled_changed == 1 and $hardcoded.result.exit_code != 0 and ($hardcoded_text | str contains "RAW_FEATURES")) $"changed=($spelled_changed) rc=($hardcoded.result.exit_code)") and $ok)

    # Control: the same literal with the feature still fails. The guard demands the constant, not only the feature.
    let right = ($dir | path join right.nu)
    (do $spell "ros-msgs,jazzy,raw-sub-tests") | str join (char newline) | save --force $right
    let right_run = (run-guard $right $dir normal [check-raw-suite-listed])
    $ok = ((verdict guard_fails_when_run_tests_spells_the_right_features_itself ($right_run.result.exit_code != 0) $"rc=($right_run.result.exit_code)") and $ok)

    # A stub that lists one test too few, and one too many: the count check must fail for both.
    for delta in [-1 1] {
        let wrong = (run-guard $SOURCE $dir normal [check-raw-suite-listed] $delta)
        let wrong_ok = ($wrong.result.exit_code != 0 and ($"($wrong.result.stdout)($wrong.result.stderr)" | str contains "count is stale"))
        $ok = ((verdict $"guard_fails_when_the_stub_count_is_off_by_($delta)" $wrong_ok $"rc=($wrong.result.exit_code)") and $ok)
    }

    let skipped = (run-guard $SOURCE $dir fail [--os-name macos check-raw-suite-listed])
    let skip_ok = ($skipped.result.exit_code == 0 and ($skipped.result.stdout | str contains "skipped") and ($skipped.log | is-empty))
    $ok = ((verdict guard_skips_when_the_os_flag_says_macos $skip_ok $"rc=($skipped.result.exit_code) stub_calls=($skipped.log | lines | length)") and $ok)

    let linux = (run-guard $SOURCE $dir normal [--os-name linux check-raw-suite-listed])
    let linux_ok = ($linux.result.exit_code == 0 and ($linux.log | str contains "nextest list"))
    $ok = ((verdict guard_runs_when_the_os_flag_says_linux $linux_ok $"rc=($linux.result.exit_code)") and $ok)

    rm -rf $dir
    print $"RESULT=(if $ok { 0 } else { 1 })"
    if not $ok { exit 1 }
}
