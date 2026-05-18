#!/usr/bin/env nu
# Check that all examples referenced in book documentation exist in the codebase

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

    # Collect all example names referenced in book markdown files
    let book_refs = (
        glob book/src/**/*.md
        | each { |file|
            open $file
            | lines
            | each { |line|
                if ($line | str contains "cargo run --example") {
                    let trimmed = ($line | str trim)
                    if not ($trimmed | str starts-with "#") {
                        # Extract example name: text after "--example " up to " " or end
                        let after = ($trimmed | str replace --regex ".*--example\\s+" "")
                        let name = ($after | str replace --regex "[`\\s].*" "" | str trim)
                        if ($name | is-not-empty) {
                            {file: ($file | path basename), example: $name}
                        }
                    }
                }
            }
            | compact
        }
        | flatten
        | uniq-by example
    )

    # Check each book reference against actual cargo examples
    let missing = (
        $book_refs
        | where { |ref_| $ref_.example not-in $example_names }
    )

    print $"Book references ($book_refs | length) unique examples"
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
