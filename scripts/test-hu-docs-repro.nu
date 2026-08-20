#!/usr/bin/env nu
# Reproduce every documented `hu` command against a *downloaded* install.
#
# The bar this enforces: a user who downloads `hu` and its plugins — no repo,
# no cargo, no build tree — can run every `hu` command that appears in the
# docs. A command in the docs is a promise; this executes the promises.
#
# Commands are classified by a `repro:` directive. Two forms, because docs are
# for readers first and the directives must not clutter the rendered page:
#
#   1. Fence-level, as an HTML comment immediately before the fence. Invisible
#      in the rendered docs, and covers every command in that fence — which is
#      what most fences need, since they tend to be homogeneous:
#
#          <!-- repro: skip needs a real rclcpp node exposing get_logger_levels -->
#          ```bash
#          hu monitor log-level /talker
#          ```
#
#   2. Per-command, as a `# repro:` line inside the fence, overriding the
#      fence default for the single command that follows. This one IS visible,
#      so use it only where the note earns its place.
#
# An unannotated command line defaults to `run` and MUST succeed. Nothing is
# silently ignored — a docs command missing from the report fails the suite.
#
# TWO command classes are extracted, and the second one is easy to forget:
#
#   1. `hu ...` — the promises about the tool.
#   2. INSTALL commands (`curl`, `install-hu.sh`, `tar`, `sha256sum`, `cp`, …)
#      — the promises about *getting* the tool.
#
# Class 2 was invisible until 2026-08-15, because extraction took only lines
# starting with `hu`. docs/tools/hu-install.md was in DOC_FILES and carried
# `repro:` directives, so it looked covered while every load-bearing line in it
# — the `curl … | sh` one-liner, `install-hu.sh --offline`, `tar -xzf`,
# `sha256sum -c` — was silently dropped. A one-liner pointing at a URL nothing
# served shipped in the docs under that blind spot. A suite that cannot see a
# command cannot report it missing, and absence of output is UNKNOWN, never
# SUCCESS.

const DOC_FILES = [
    "docs/tools/hu.md"
    "docs/tools/hu-install.md"
    "docs/tools/hu-plugins.md"
    "docs/tools/hu-vs-ros2cli.md"
    "docs/tools/why-hu.md"
]

# Fences whose contents are shell commands. Everything else (rust, toml, wit,
# mermaid, text) is prose or output and is never executed.
const SHELL_FENCES = ["bash" "sh" "shell" "console"]

# Command heads that make a documented *installation* step. The list is a
# whitelist, not a heuristic: it names the verbs the install docs actually
# instruct with, so an unrelated shell line in some other fence does not become
# a silent new test. Adding a doc step with a head that is not here reproduces
# the D10 blind spot for that step, so extend this list when the docs grow a
# new install verb.
#
#   curl/wget            the download one-liner
#   install-hu.sh, sh    the installer, run directly or piped into a shell
#   tar/unzip            unpacking a release tarball
#   install/cp/mkdir     placing the binary and the plugins by hand
#   sha256sum/shasum     the integrity check the docs tell the reader to run
#   rm                   the uninstall step
const INSTALL_HEADS = [
    "curl" "wget"
    "install-hu.sh" "./install-hu.sh"
    "sh" "bash"
    "tar" "unzip"
    "install" "cp" "mkdir"
    "sha256sum" "shasum"
    "rm"
]

# Heads that would mutate the very install this suite is measuring. They are
# extracted and reported like anything else, but never executed: `rm -rf
# ~/.local/share/hu` inside the scratch HOME deletes the artifact under test,
# and every command after it then fails for a reason that has nothing to do
# with the artifact — a cascade whose root cause is invisible in the report.
# Refusing is louder than running and louder than ignoring: the entry fails
# with a note naming the directive that resolves it.
const DESTRUCTIVE_HEADS = ["rm"]

# ---------------------------------------------------------------- extraction

# First real token of a command, ignoring any leading `KEY=value` environment
# prefixes (`HU_VERSION=0.1.0 install-hu.sh …`). Returns "" for a line that is
# nothing but assignments.
def cmd-head [cmd: string] {
    mut toks = ($cmd | split row " " | where { |t| $t != "" })
    while (($toks | length) > 0) and (($toks | first) =~ '^[A-Za-z_][A-Za-z0-9_]*=') {
        $toks = ($toks | skip 1)
    }
    if ($toks | is-empty) { "" } else { $toks | first }
}

# Pull every `hu ...` invocation out of the shell fences of one markdown file,
# carrying its classification and any `export` lines that precede it in the
# same fence.
def extract-file [file: string] {
    mut out = []
    mut in_fence = false
    mut fence_is_shell = false
    mut pending = null          # `# repro:` directive awaiting its command
    mut pending_expect = null   # `# repro-expect:` regex awaiting its command
    mut fence_default = null    # `<!-- repro: -->` covering the whole fence
    mut next_fence_default = null
    mut fence_env = []          # `export K=V` seen earlier in this fence
    mut cont = ""               # accumulator for `\`-continued lines
    mut cont_line = 0

    # `--raw`: nu parses .md into a table otherwise, and we need the text.
    for entry in (open --raw $file | lines | enumerate) {
        let lineno = $entry.index + 1
        let raw = $entry.item
        let line = ($raw | str trim)

        if ($line | str starts-with "```") {
            if $in_fence {
                $in_fence = false
                $fence_is_shell = false
                $fence_env = []
                $pending = null
                $pending_expect = null
                $fence_default = null
                $cont = ""
            } else {
                let info = ($line | str replace --regex '^`+' "" | str trim | split row " " | first | default "")
                $in_fence = true
                $fence_is_shell = ($info in $SHELL_FENCES)
                $fence_env = []
                $pending = null
                $pending_expect = null
                $fence_default = $next_fence_default
                $cont = ""
            }
            $next_fence_default = null
            continue
        }

        if not $in_fence {
            # Fence-level directive: an HTML comment, so it never renders.
            # Applies to the next fence only; a blank line does not cancel it,
            # but any other prose does — keeping its scope obvious to a reader.
            if ($line | str starts-with "<!-- repro:") {
                $next_fence_default = ($line
                    | str replace "<!-- repro:" ""
                    | str replace "-->" ""
                    | str trim)
            } else if $line != "" {
                $next_fence_default = null
            }
            continue
        }

        if not $fence_is_shell { continue }

        # Mid-continuation: keep accumulating until a line without a trailing `\`.
        if $cont != "" {
            if ($line | str ends-with "\\") {
                $cont = $"($cont) ($line | str substring 0..<(($line | str length) - 1) | str trim)"
            } else {
                let full = $"($cont) ($line)"
                $out = ($out | append (make-entry $file $cont_line $full ($pending | default $fence_default) $fence_env $pending_expect))
                $cont = ""
                $pending = null
                $pending_expect = null
            }
            continue
        }

        if ($line | str starts-with "# repro:") {
            $pending = ($line | str replace "# repro:" "" | str trim)
            continue
        }

        # `# repro-expect: <regex>` asserts the command's OUTPUT, not just its
        # exit status. It is the difference between "the plugin dispatched" and
        # "the plugin computed the right answer" — an audit found every release
        # assertion was the former, so a truncated .wasm could pass. Kept
        # separate from `# repro:` so a command can carry both a class and an
        # expectation without inventing a combined grammar.
        if ($line | str starts-with "# repro-expect:") {
            $pending_expect = ($line | str replace "# repro-expect:" "" | str trim)
            continue
        }

        # Other comments and output lines carry no command.
        if ($line | str starts-with "#") or ($line == "") { continue }

        if ($line | str starts-with "export ") {
            $fence_env = ($fence_env | append ($line | str replace "export " "" | str trim))
            continue
        }

        # `$ hu ...` prompt form is accepted too.
        let cmd = if ($line | str starts-with "$ ") { $line | str substring 2.. | str trim } else { $line }

        # The `hu` test is unchanged, and is asked FIRST: an install-class head
        # can never also be an `hu` line, so nothing is counted twice and the
        # set of extracted `hu` commands is byte-for-byte what it was before.
        let is_hu = (($cmd | str starts-with "hu ") or ($cmd == "hu"))
        # An install line is classified by exactly the same `repro:` directives
        # as an `hu` line, and — deliberately — defaults to `run` in exactly
        # the same way. A command the fixture cannot satisfy must be declared
        # `skip <reason>` in the doc, next to the promise, where a reader sees
        # the reason. It must never be dropped by the extractor, which is the
        # failure mode this whole change exists to remove.
        let is_install = ((not $is_hu) and ((cmd-head $cmd) in $INSTALL_HEADS))
        if not ($is_hu or $is_install) { continue }

        if ($cmd | str ends-with "\\") {
            $cont = ($cmd | str substring 0..<(($cmd | str length) - 1) | str trim)
            $cont_line = $lineno
            continue
        }

        $out = ($out | append (make-entry $file $lineno $cmd ($pending | default $fence_default) $fence_env $pending_expect))
        $pending = null
        $pending_expect = null
    }

    $out
}

def make-entry [file: string, line: int, cmd: string, directive: any, env_lines: list, expect: any = null] {
    let d = ($directive | default "run")
    let kind = ($d | split row " " | first)
    let rest = ($d | str replace --regex '^\S+\s*' "")

    let class = match $kind {
        "skip" => "skip"
        "timeout" => "run-timeout"
        # For commands that stream *changes* (`hu monitor watch`, `hu stream`),
        # silence on an idle graph is the correct behaviour, so requiring
        # output would fail a working command. Kept as a separate class rather
        # than relaxing `timeout` for everything: a command that silently does
        # nothing should still fail unless someone said it may be quiet.
        "timeout-quiet" => "run-timeout-quiet"
        "run" => "run"
        _ => "run"
    }

    {
        file: $file
        line: $line
        command: $cmd
        class: $class
        reason: (if $class == "skip" { $rest } else { "" })
        timeout: (if $class in ["run-timeout" "run-timeout-quiet"] { ($rest | into int) } else { 0 })
        env: $env_lines
        expect: ($expect | default "")
    }
}

def extract-all [] {
    $DOC_FILES | each { |f| extract-file $f } | flatten
}

# ------------------------------------------------------------------ execution

# Build the environment a *downloaded* install runs in: a scratch HOME, a PATH
# holding only the installed bin dir, and no HU_PLUGIN_PATH so discovery must
# fall through to ~/.local/share/hu/plugins — exactly what a user gets.
def clean-env [home: string, extra: list] {
    # Prepend the install dir rather than replacing PATH. Replacing it removed
    # `bash` itself on NixOS, where the shell lives in the nix store and not in
    # /bin — and it would also hide `jq`, which documented commands pipe into.
    # `hu` resolving to the installed copy is asserted separately, in main.
    let inherited = ($env.PATH? | default [] | str join (char esep))
    mut e = {
        HOME: $home
        PATH: $"($home)/.local/bin:($inherited)"
        HU_PLUGIN_PATH: null
        RUSTFLAGS: ""
        # hiroz logs a dozen INFO lines per node at startup. Unfiltered they
        # bury the actual error in the failure excerpt, which is the one thing
        # the report exists to show.
        RUST_LOG: "error"
    }
    for kv in $extra {
        let parts = ($kv | split row "=")
        if ($parts | length) >= 2 {
            $e = ($e | insert ($parts | first) ($parts | skip 1 | str join "="))
        }
    }
    $e
}

def run-one [entry: record, home: string] {
    if $entry.class == "skip" {
        return { result: "skip", rc: 0, note: $entry.reason, out: "" }
    }

    # Asked after `skip`, so a doc that declares the reason still wins. Asked
    # before execution, so an undeclared uninstall step cannot delete the
    # install the remaining commands are measured against — see
    # DESTRUCTIVE_HEADS. This is a fail, not a skip: the promise is unproven,
    # and the note says what to write in the doc to resolve it.
    if (cmd-head $entry.command) in $DESTRUCTIVE_HEADS {
        return {
            result: "fail"
            rc: 0
            note: "destructive command not run — it would delete the install under test; declare it in the doc with `<!-- repro: skip <reason> -->`"
            out: ""
        }
    }

    let env_map = (clean-env $home $entry.env)
    let secs = (if $entry.timeout > 0 { $entry.timeout } else { 30 })
    # `timeout` so a streaming command cannot hang the suite. `-k 5` is not
    # optional: SIGINT alone is a request, and a TUI or a wedged plugin can
    # ignore it — without the follow-up SIGKILL one bad command hangs the whole
    # run with no output to say which.
    #
    # `set -o pipefail` is load-bearing, not hygiene. Several documented
    # commands pipe into `jq` (`hu meter hz … --json | jq '.rate_hz'`), and
    # bash reports the LAST command's status by default — so a failing `hu`
    # whose error went to stderr scored a pass, because jq exited 0 on empty
    # input. An audit found two such commands among the "passing" set. With
    # pipefail the pipeline carries hu's status, and a timeout still surfaces
    # as 124 for the streaming classes.
    let wrapped = $"set -o pipefail; timeout -k 5 --preserve-status -s INT ($secs) ($entry.command)"

    # `bash -c`, not `-lc`: a login shell sources the user's profile, which can
    # put a system `hu` on PATH and quietly test the wrong binary.
    # `^cmd | complete` captures a non-zero exit as data. Wrapping it in a bare
    # `do { }` instead makes nu raise on the first failing command, which ends
    # the whole suite at the first red — the opposite of what a test runner
    # should do.
    let res = (with-env $env_map { ^bash -c $wrapped | complete })
    let rc = $res.exit_code
    let out = $"($res.stdout)($res.stderr)"

    # An `expect` regex outranks every status rule below. A command can exit 0,
    # stream plenty, and still be wrong — which is exactly the hole this closes:
    # before, `hu meter hz` reported a pass whatever number it printed, so a
    # plugin that dispatched but computed nonsense was indistinguishable from a
    # correct one. Checked first so a wrong answer cannot be rescued by a
    # generous status rule.
    if $entry.expect != "" {
        # Whitespace is collapsed first: plugin output is column-aligned, so a
        # pattern written against the doc's example would otherwise have to
        # encode the exact padding.
        let flat = ($out | str replace --all --regex '\s+' " ")
        if not ($flat =~ $entry.expect) {
            return { result: "fail", rc: $rc, note: $"output did not match expect: ($entry.expect)", out: $out }
        }
    }

    if $entry.class == "run-timeout-quiet" {
        # Survived its window without erroring. No output requirement — see
        # the class comment in make-entry.
        let alive = ($rc in [124 130 2])
        if $alive or $rc == 0 {
            { result: "pass", rc: $rc, note: "ran (quiet allowed)", out: $out }
        } else {
            { result: "fail", rc: $rc, note: "exited non-zero before timeout", out: $out }
        }
    } else if $entry.class == "run-timeout" {
        # A streaming command is healthy if it was still running when the
        # timeout fired (124, or the INT-preserved 130/2) AND it printed
        # something. Exiting 0 early is also fine. Anything else is a failure.
        let alive = ($rc in [124 130 2])
        if ($alive or $rc == 0) and (($out | str trim) != "") {
            { result: "pass", rc: $rc, note: "streamed output", out: $out }
        } else if ($alive or $rc == 0) {
            { result: "fail", rc: $rc, note: "ran but produced no output", out: $out }
        } else {
            { result: "fail", rc: $rc, note: "exited non-zero before timeout", out: $out }
        }
    } else {
        if $rc == 0 {
            { result: "pass", rc: 0, note: "", out: $out }
        } else {
            { result: "fail", rc: $rc, note: $"exit ($rc)", out: $out }
        }
    }
}

# -------------------------------------------------------------------- report

# Stand up the graph the docs describe, using a publisher that is NOT the
# artifact under test.
#
# This separation is the point. `hu` cannot generate its own traffic: `hu meter
# pub` needs a message schema from a `.msg` on HIROZ_MSG_PATH or from a live
# node, and a release ships neither. So a suite whose only fixture is `hu
# router` measures an empty graph, and every `hu meter` command it runs is
# reduced to checking that the process started. That is how a truncated plugin
# could pass a release check.
#
# The publisher therefore comes from the build tree — it stands in for the
# "existing ROS 2 or hiroz deployment" that `hu` is documented to observe.
# Using the artifact for both sides would repeat the mistake that hid this:
# a fixture built from the same source as the code under test cannot represent
# a user who only downloaded the code.
def start-traffic [publisher: string, endpoint: string, home: string] {
    if $publisher == "" { return null }
    if not ($publisher | path exists) {
        print $"FAIL: --publisher ($publisher) does not exist"
        exit 1
    }
    print $"starting traffic fixture: ($publisher)"
    # Log beside the scratch HOME, not in a temp dir: it belongs with the rest
    # of the run's state, and `$nu.temp-path` does not exist in every nushell.
    let log = $"($home)/talker.log"
    let pid = (
        job spawn {
            with-env { RUST_LOG: "error" } {
                ^$publisher --role talker --endpoint $endpoint out+err> $log
            }
        }
    )
    # The graph is liveliness-driven, so a `hu meter list` issued too early
    # legitimately sees nothing. Wait for the token to propagate rather than
    # sleeping a guessed interval.
    sleep 4sec
    { job: $pid, log: $log }
}

def main [
    --home: string          # scratch HOME holding the downloaded install
    --list                  # only print what would run
    --filter: string = ""   # substring filter on the command
    --publisher: string = "" # binary that puts real traffic on /chatter (see start-traffic)
    --endpoint: string = "tcp/127.0.0.1:7447"
    --require-traffic       # fail rather than run against an empty graph
] {
    let entries = (extract-all | where { |e| ($filter == "") or ($e.command | str contains $filter) })

    if ($entries | is-empty) {
        # Zero extracted commands is a harness failure, not a pass. A suite
        # that runs nothing must never report success.
        print "FAIL: extracted 0 documented commands from the docs — the extractor is broken"
        exit 1
    }

    if $list {
        $entries | select file line class command | print
        print $"($entries | length) commands"
        return
    }

    let home = ($home | default "")
    if $home == "" or not ($home | path exists) {
        print "FAIL: --home must point at a prepared install dir (see scripts/install-hu.sh --offline)"
        exit 1
    }

    # The whole point is testing the *downloaded* hu. If PATH resolution picks
    # up a system or build-tree copy instead, every result below is about the
    # wrong binary and the suite is worse than useless — so prove it first.
    let probe = (with-env (clean-env $home []) { ^bash -c "command -v hu" | complete })
    let resolved = ($probe.stdout | str trim)
    if $probe.exit_code != 0 or not ($resolved | str starts-with $"($home)/.local/bin/") {
        print $"FAIL: `hu` resolves to '($resolved)', not the install under ($home)"
        exit 1
    }
    print $"testing ($resolved)"

    if $require_traffic and $publisher == "" {
        # Guard against the quiet degradation this flag exists to prevent: a
        # release gate that silently becomes an exit-status check because
        # nobody passed a publisher.
        print "FAIL: --require-traffic was set but no --publisher was given"
        exit 1
    }
    let traffic = (start-traffic $publisher $endpoint $home)

    if $traffic != null {
        # Prove the fixture works before trusting any result that depends on
        # it. If the talker died on startup, every `hu meter` command below
        # would fail for a reason that has nothing to do with the artifact.
        let seen = (with-env (clean-env $home []) {
            ^bash -c "timeout 15 hu meter list topics 2>&1" | complete
        })
        if not ($seen.stdout | str contains "/chatter") {
            print "FAIL: traffic fixture produced no /chatter topic — the graph is empty"
            print ($seen.stdout | lines | first 10 | str join "\n")
            print $"talker log: ($traffic.log)"
            if ($traffic.log | path exists) { print (open --raw $traffic.log | lines | last 10 | str join "\n") }
            job kill $traffic.job
            exit 1
        }
        print "traffic fixture live: /chatter visible to the installed hu"
    }

    mut rows = []
    let total = ($entries | length)
    for it in ($entries | enumerate) {
        let e = $it.item
        # Print before running, not after: if a command wedges, the last line
        # printed names the culprit instead of leaving a silent hang.
        print $"[($it.index + 1)/($total)] ($e.class) ($e.command)"
        let r = (run-one $e $home)
        $rows = ($rows | append {
            result: $r.result
            where: $"($e.file):($e.line)"
            class: $e.class
            command: ($e.command | str substring 0..90)
            note: $r.note
        })
        if $r.result == "fail" {
            print $"FAIL ($e.file):($e.line)  ($e.command)"
            print $"     ($r.note)"
            print ($r.out | lines | first 6 | each { |l| $"     | ($l)" } | str join "\n")
        }
    }

    if $traffic != null { job kill $traffic.job }

    print ""
    $rows | print
    let passed = ($rows | where result == "pass" | length)
    let failed = ($rows | where result == "fail" | length)
    let skipped = ($rows | where result == "skip" | length)
    print $"\n($passed) passed, ($failed) failed, ($skipped) skipped, ($rows | length) total"

    if $failed > 0 { exit 1 }
}
