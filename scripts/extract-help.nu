#!/usr/bin/env nu
# Extract --help text from examples and optionally generate markdown reference

# Extract example names from hiroz/Cargo.toml
def extract_examples [] {
    let content = (cat hiroz/Cargo.toml)
    $content
    | split row "[[example]]"
    | skip 1
    | each { |section|
        let name_line = (
            $section
            | lines
            | where { |line| $line | str starts-with "name = \"" }
            | first
        )
        if ($name_line | is-empty) {
            null
        } else {
            $name_line | str replace 'name = "' '' | str replace '"' '' | str trim
        }
    }
    | compact
}

# Get help text for a specific example
def get_example_help [example: string] {
    try {
        let help_output = (cargo run --example $example -- --help | complete)
        if $help_output.exit_code == 0 {
            {example: $example, help: $help_output.stdout, success: true}
        } else {
            {example: $example, error: $help_output.stderr, success: false}
        }
    } catch {
        {example: $example, error: "Failed to run example", success: false}
    }
}

# Parse help output into structured format
def parse_help_output [help_text: string] {
    let lines = ($help_text | lines)

    # Extract description (first non-empty line after program name)
    let description = (
        $lines
        | where { |line| not ($line | str trim | is-empty) }
        | skip 1
        | first
        | str trim
    )

    # Extract options section
    let options_start = ($lines | enumerate | where { |it| $it.item | str contains "Options:" } | first | get index)

    if ($options_start | is-empty) {
        {description: $description, options: []}
    } else {
        let option_lines = ($lines | skip ($options_start + 1) | take until { |line| ($line | str trim | is-empty) or ($line | str starts-with "Commands:") })

        let options = (
            $option_lines
            | reduce -f {current: null, result: []} { |line, acc|
                let trimmed = ($line | str trim)
                if ($trimmed | str starts-with "-") {
                    # New option line
                    let parts = ($trimmed | parse "{flags} {description}")
                    if ($parts | length) > 0 {
                        let flags = ($parts | first | get flags)
                        let desc = ($parts | first | get description)
                        {
                            current: {flags: $flags, description: $desc, default: null},
                            result: $acc.result
                        }
                    } else {
                        $acc
                    }
                } else if ($acc.current | is-not-empty) {
                    # Continuation line (default value or more description)
                    if ($trimmed | str contains "[default:") {
                        let default_match = ($trimmed | parse "[default: {value}]")
                        if ($default_match | length) > 0 {
                            let default_val = ($default_match | first | get value)
                            let updated = ($acc.current | merge {default: $default_val})
                            {
                                current: null,
                                result: ($acc.result | append $updated)
                            }
                        } else {
                            $acc
                        }
                    } else {
                        $acc
                    }
                } else {
                    $acc
                }
            }
            | get result
        )

        {description: $description, options: $options}
    }
}

# Main execution
def main [
    example?: string  # Specific example to extract help for (optional)
    --markdown         # Generate markdown format
] {
    if ($example | is-not-empty) {
        # Extract help for specific example
        print $"Extracting help for: ($example)"
        let help_info = (get_example_help $example)

        if $help_info.success {
            if $markdown {
                print $"## ($example)\n"
                print $help_info.help
            } else {
                print $help_info.help
            }
        } else {
            print $"❌ Failed to get help: ($help_info.error)"
            exit 1
        }
    } else {
        # Extract help for all examples
        print "📚 Extracting help text for all examples...\n"

        let examples = (extract_examples)
        print $"Found ($examples | length) examples\n"

        let results = ($examples | each { |ex|
            print $"Extracting: ($ex)..."
            get_example_help $ex
        })

        let successful = ($results | where success)
        let failed = ($results | where { |r| not $r.success })

        print $"\n✅ Successfully extracted: ($successful | length)/($examples | length)"

        if ($failed | length) > 0 {
            print $"\n❌ Failed examples:"
            $failed | each { |f| print $"  - ($f.example): ($f.error)" }
        }

        if $markdown {
            print "\n# Example Command Reference\n"
            $successful | each { |s|
                print $"## ($s.example)\n"
                print "```"
                print $s.help
                print "```\n"
            }
        }
    }
}
