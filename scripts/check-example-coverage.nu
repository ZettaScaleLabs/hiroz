#!/usr/bin/env nu
# Check that all examples referenced in MkDocs documentation exist in the codebase

def main [] {
    # Get actual example names from cargo metadata (authoritative)
    let metadata = (cargo metadata --no-deps --format-version 1 | from json)
    let example_names = (
        $metadata.packages
        | where name == "hiroz"
        | each { |pkg| $pkg.targets }
        | flatten
        | where kind == ["example"]
        | get name
    )

    # Collect all example names referenced in MkDocs markdown files
    let doc_refs = (
        glob docs/**/*.md
        | each { |file|
            open --raw $file
            | lines
            | each { |line|
                if ($line | str contains "cargo run --example") {
                    let trimmed = ($line | str trim)
                    if not ($trimmed | str starts-with "#") {
                        # Ignore prose that mentions the option without naming an
                        # example, and collect every complete command on the line.
                        $trimmed
                        | parse --regex 'cargo\s+run\s+--example\s+(?P<example>[A-Za-z0-9_-]+)'
                        | each { |entry| {file: ($file | path basename), example: $entry.example} }
                    }
                }
            }
            | flatten
            | compact
        }
        | flatten
        | uniq-by example
    )

    # Check each documentation reference against actual cargo examples
    let missing = (
        $doc_refs
        | where { |ref_| $ref_.example not-in $example_names }
    )

    print $"Documentation references ($doc_refs | length) unique examples"
    print $"Codebase contains ($example_names | length) examples"

    if ($missing | is-empty) {
        print "All documented examples exist in the codebase"
    } else {
        let count = ($missing | length)
        print $"\n($count) documented examples not found in codebase:\n"
        $missing | each { |m|
            print $"  ($m.file): cargo run --example ($m.example)"
        }
        print ""
        exit 1
    }
}
