#!/usr/bin/env nu

# Generate a minimal dependencies.json from ROS 2 packages

use lib/common.nu [log-step, log-success, DISTROS]

# Extract message dependencies from package.xml
def extract-deps [xml_path: path] {
    if not ($xml_path | path exists) {
        return []
    }

    let content = open $xml_path --raw

    # Extract <depend> and <build_depend> tags
    let deps = $content
        | parse --regex '<(?:depend|build_depend)>([^<]+)</(?:depend|build_depend)>'
        | get capture0
        | where { |dep| ($dep | str ends-with 'msgs') or ($dep | str ends-with 'interfaces') }
        | uniq
        | sort

    $deps
}

# Clone ROS 2 repositories
def clone-repos [temp_dir: path, distro: string] {
    let repos = [
        "common_interfaces"
        "rcl_interfaces"
        "example_interfaces"
        "demos"
        "unique_identifier_msgs"
    ]

    for repo in $repos {
        try {
            git clone --depth 1 --branch $distro $"https://github.com/ros2/($repo).git" $"($temp_dir)/($repo)" e>| ignore
        } catch {
            # Silently ignore clone failures (branch may not exist)
        }
    }
}

# Main entry point
def main [
    --distro: string = "jazzy"   # ROS distro (humble, jazzy)
] {
    if $distro not-in $DISTROS {
        error make { msg: $"Unknown distro: ($distro). Must be: ($DISTROS | str join ', ')" }
    }

    let temp_dir = mktemp -d
    let assets_dir = $"hiroz-codegen/assets/($distro)"

    log-step $"Generating dependencies.json for ROS ($distro)..."

    # Clone repositories
    log-step "Cloning repositories..."
    clone-repos $temp_dir $distro

    # Package mappings
    let packages = {
        builtin_interfaces: $"($temp_dir)/rcl_interfaces/builtin_interfaces"
        service_msgs: $"($temp_dir)/rcl_interfaces/service_msgs"
        action_msgs: $"($temp_dir)/rcl_interfaces/action_msgs"
        std_msgs: $"($temp_dir)/common_interfaces/std_msgs"
        geometry_msgs: $"($temp_dir)/common_interfaces/geometry_msgs"
        sensor_msgs: $"($temp_dir)/common_interfaces/sensor_msgs"
        nav_msgs: $"($temp_dir)/common_interfaces/nav_msgs"
        example_interfaces: $"($temp_dir)/example_interfaces"
        action_tutorials_interfaces: $"($temp_dir)/demos/action_tutorials/action_tutorials_interfaces"
        test_msgs: $"($temp_dir)/rcl_interfaces/test_msgs"
        unique_identifier_msgs: $"($temp_dir)/unique_identifier_msgs"
    }

    # Build package dependencies map
    let pkg_deps = $packages
        | transpose key value
        | where { |row| ($row.value | path exists) }
        | each { |row|
            let deps = extract-deps $"($row.value)/package.xml"
            { name: $row.key, dependencies: $deps }
        }
        | reduce --fold {} { |pkg, acc|
            $acc | insert $pkg.name { dependencies: $pkg.dependencies }
        }

    # Create output structure
    let output = {
        format_version: "1.0"
        ros_distro: $distro
        generated: (date now | format date "%Y-%m-%dT%H:%M:%SZ")
        packages: $pkg_deps
    }

    # Ensure assets directory exists
    mkdir $assets_dir

    # Write JSON file
    $output | to json --indent 2 | save --force $"($assets_dir)/dependencies.json"

    # Cleanup
    rm -rf $temp_dir

    log-success $"Generated: ($assets_dir)/dependencies.json"
    print $"Size: (ls $'($assets_dir)/dependencies.json' | get size | first)"
    print ""
    open $"($assets_dir)/dependencies.json"
}
