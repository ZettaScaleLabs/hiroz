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
def run-guard [script: path, dir: path, mode: string, args: list<string>] {
    let log = ($dir | path join cargo.log)
    rm -f $log
    let count = (open --raw $SOURCE | parse --regex 'const RAW_SUITE_TESTS = (\d+)' | get capture0.0)
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
    # `nextest list` line: the drift the guard exists to catch.
    let mutant = ($dir | path join mutant.nu)
    let lines = (open --raw $SOURCE | lines)
    let mutated = ($lines | each {|l| if ($l | str contains 'nextest list') { $l } else { $l | str replace --all ',raw-sub-tests' '' } })
    $mutated | str join (char newline) | save --force $mutant
    let changed = ($lines | zip $mutated | where {|p| $p.0 != $p.1} | length)

    mut ok = true

    let control = (run-guard $SOURCE $dir normal [check-raw-suite-listed])
    $ok = ((verdict guard_passes_on_the_unmutated_script ($control.result.exit_code == 0) $"rc=($control.result.exit_code)") and $ok)

    let drift = (run-guard $mutant $dir normal [check-raw-suite-listed])
    $ok = ((verdict guard_fails_when_run_tests_drops_the_feature ($changed > 0 and $drift.result.exit_code != 0 and ($drift.result.stdout | str contains "feature is off")) $"changed=($changed) rc=($drift.result.exit_code)") and $ok)

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
