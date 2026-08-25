#!/usr/bin/env python3
"""Fail when a changed file triggers no workflow and is not exempt.

Every workflow that builds or tests selects its work with an allow-list of
`paths`. An allow-list only skips what it does not name, so a file added
somewhere new is silently never built -- the failure mode this guard exists
to convert into a red check.

A file passes if either:
  * some workflow's `paths` filter matches it, or
  * `EXEMPT` below names it, with a reason.

Python, not Nushell like the rest of scripts/, because lint.yml runs this on
every pull request and python3 is already on the runner. Pulling in nushell
would cost more setup than the check itself takes.

Usage: check-ci-path-coverage.py <changed-file>...
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

WORKFLOWS = Path(".github/workflows")

# Files that legitimately trigger nothing. Each entry needs a reason: the
# point of the guard is that "nothing runs for this" is a decision, not an
# oversight.
EXEMPT: list[tuple[str, str]] = [
    (".github/workflows/lint.yml", "runs on every change; it has no filter to match"),
    (
        ".github/workflows/release.yml",
        "runs on v* tags, not on pushes or pull requests",
    ),
    (".github/workflows/docs-links.yml", "runs on a schedule"),
    (".github/workflows/semantic-pr.yml", "runs on every pull request"),
    (".github/workflows/pr-draft-check.yml", "runs on every pull request"),
    (".github/pull_request_template.md", "text GitHub renders; nothing consumes it"),
    (".github/**", "workflow metadata that no job reads"),
    (".gitignore", "affects no build"),
    (".gitattributes", "affects no build"),
    (".envrc", "local developer setup"),
    (".taplo.toml", "lint.yml runs taplo on every change"),
    (".yamllint*", "lint.yml runs yamllint on every change"),
    ("cliff.toml", "git-cliff changelog config, used at release time"),
    ("assets/**", "images referenced by prose"),
    ("LICENSE", "text"),
    (".clang-format", "no job runs clang-format; it configures editors only"),
    (".vale.ini", "vale ships in the devshell but no job runs it"),
    (".vale/**", "vale ships in the devshell but no job runs it"),
    ("*.md", "docs.yml covers docs; root prose triggers nothing else"),
]


def to_regex(pattern: str) -> re.Pattern[str]:
    """Translate a GitHub path filter to a regex.

    `*` stops at a slash, `**` does not, `?` is one non-slash character.
    """
    out, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            out += ".*"
            i += 2
        elif pattern[i] == "*":
            out += "[^/]*"
            i += 1
        elif pattern[i] == "?":
            out += "[^/]"
            i += 1
        else:
            out += re.escape(pattern[i])
            i += 1
    return re.compile("^" + out + "$")


def filters() -> dict[str, list[str]]:
    """Read each workflow's `paths` list.

    The parse is deliberately strict. A workflow that has a `paths:` key but
    yields no patterns means the shape changed, and a silently empty filter
    would under-report coverage -- so it raises instead.
    """
    found: dict[str, list[str]] = {}
    for path in sorted(WORKFLOWS.glob("*.yml")):
        text = path.read_text()
        if "\n    paths:\n" not in text:
            continue
        blocks = re.findall(r"\n    paths:\n((?:      .*\n)+)", text)
        lists = [re.findall(r"- '([^']*)'", block) for block in blocks]
        if not lists or not all(lists):
            raise SystemExit(f"{path}: has a paths: key but no patterns parsed")
        # Actions rejects YAML anchors, so push and pull_request each carry
        # their own copy of the list. Two copies drift, and the drift is
        # invisible -- both look maintained. Require them to stay identical.
        if any(other != lists[0] for other in lists[1:]):
            raise SystemExit(f"{path}: its paths: lists disagree between events")
        found[path.name] = lists[0]
    if not found:
        raise SystemExit(f"{WORKFLOWS}: no workflow declares paths:")
    return found


def matches(patterns: list[str], changed: str) -> bool:
    """Apply GitHub's last-match-wins rule over an ordered pattern list."""
    included = False
    for pattern in patterns:
        negated = pattern.startswith("!")
        if to_regex(pattern[1:] if negated else pattern).match(changed):
            included = not negated
    return included


def main(changed: list[str]) -> int:
    workflows = filters()
    exempt = [(to_regex(p), p, why) for p, why in EXEMPT]

    uncovered = []
    for name in changed:
        if any(patterns and matches(patterns, name) for patterns in workflows.values()):
            continue
        if any(rx.match(name) for rx, _, _ in exempt):
            continue
        uncovered.append(name)

    if not uncovered:
        print(
            f"{len(changed)} changed file(s): every one triggers a workflow or is exempt"
        )
        return 0

    print("These changed files trigger no workflow:\n")
    for name in uncovered:
        print(f"  {name}")
    print(
        "\nAdd each one to the paths: of the workflow that should build it,"
        "\nor add it to EXEMPT in scripts/check-ci-path-coverage.py with the"
        "\nreason nothing needs to run. Both are fine; silence is not."
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
