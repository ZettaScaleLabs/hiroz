#!/usr/bin/env nu

# Update vendored ROS 2 message definitions

use lib/common.nu [log-step, log-success, log-warning, DISTROS]

# Clone a repository with error handling
def clone-repo [url: string, branch: string, dest: path] {
    try {
        git clone --depth 1 --branch $branch $url $dest e>| ignore
    } catch {
        log-warning $"Failed to clone: ($url)"
    }
}

# Get commit hash from a repository
def get-commit [repo_path: path] {
    if ($repo_path | path exists) {
        try {
            git -C $repo_path rev-parse HEAD | str trim
        } catch {
            "N/A"
        }
    } else {
        "N/A"
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

    log-step $"Updating vendored ROS ($distro) assets..."

    # Clone repositories
    log-step "Cloning ros2/common_interfaces..."
    clone-repo "https://github.com/ros2/common_interfaces.git" $distro $"($temp_dir)/common_interfaces"

    log-step "Cloning ros2/rcl_interfaces..."
    clone-repo "https://github.com/ros2/rcl_interfaces.git" $distro $"($temp_dir)/rcl_interfaces"

    log-step "Cloning ros2/example_interfaces..."
    clone-repo "https://github.com/ros2/example_interfaces.git" $distro $"($temp_dir)/example_interfaces"

    log-step "Cloning ros2/demos..."
    clone-repo "https://github.com/ros2/demos.git" $distro $"($temp_dir)/demos"

    log-step "Cloning ros2/unique_identifier_msgs..."
    clone-repo "https://github.com/ros2/unique_identifier_msgs.git" $distro $"($temp_dir)/unique_identifier_msgs"

    # Clear old assets
    log-step "Clearing old assets..."
    if ($assets_dir | path exists) {
        rm -rf $assets_dir
    }
    mkdir $assets_dir

    # Copy packages from rcl_interfaces
    log-step "Copying rcl_interfaces packages..."
    for pkg in [builtin_interfaces action_msgs service_msgs] {
        let src = $"($temp_dir)/rcl_interfaces/($pkg)"
        if ($src | path exists) {
            print $"  - ($pkg)"
            cp -r $src $assets_dir
        }
    }

    # Copy unique_identifier_msgs
    let uid_src = $"($temp_dir)/unique_identifier_msgs"
    if ($uid_src | path exists) {
        log-step "Copying unique_identifier_msgs..."
        cp -r $uid_src $assets_dir
    }

    # Copy packages from common_interfaces
    log-step "Copying common_interfaces packages..."
    for pkg in [std_msgs geometry_msgs sensor_msgs nav_msgs] {
        let src = $"($temp_dir)/common_interfaces/($pkg)"
        if ($src | path exists) {
            print $"  - ($pkg)"
            cp -r $src $assets_dir
        }
    }

    # Copy example_interfaces
    let example_src = $"($temp_dir)/example_interfaces"
    if ($example_src | path exists) {
        log-step "Copying example_interfaces..."
        cp -r $example_src $assets_dir
    }

    # Copy action_tutorials_interfaces from demos
    let action_tut_src = $"($temp_dir)/demos/action_tutorials/action_tutorials_interfaces"
    if ($action_tut_src | path exists) {
        log-step "Copying action_tutorials_interfaces..."
        cp -r $action_tut_src $assets_dir
    }

    # Copy test_msgs
    let test_msgs_src = $"($temp_dir)/rcl_interfaces/test_msgs"
    if ($test_msgs_src | path exists) {
        log-step "Copying test_msgs..."
        cp -r $test_msgs_src $assets_dir
    }

    # Remove unnecessary files (keep only .msg, .srv, .action)
    log-step "Removing unnecessary files..."
    glob $"($assets_dir)/**/*"
        | where { |f| ($f | path type) == "file" }
        | where { |f|
            not (($f | str ends-with '.msg') or ($f | str ends-with '.srv') or ($f | str ends-with '.action'))
        }
        | each { |f| rm $f }

    # Remove empty directories
    mut changed = true
    while $changed {
        let empty_dirs = glob $"($assets_dir)/**/*"
            | where { |d| ($d | path type) == "dir" }
            | where { |d| (ls $d | is-empty) }

        if ($empty_dirs | is-empty) {
            $changed = false
        } else {
            $empty_dirs | each { |d| rm -r $d }
        }
    }

    # Generate dependencies.json
    log-step "Generating dependencies.json..."
    nu scripts/generate-dependencies-json.nu --distro $distro

    # Calculate summary info
    let pkg_count = glob $"($assets_dir)/*" | where { |p| ($p | path type) == "dir" } | length
    let total_size = du $assets_dir | get apparent | first

    # Cleanup
    rm -rf $temp_dir

    # Report
    print ""
    log-success "Summary:"
    print $"  Distro: ($distro)"
    print $"  Location: ($assets_dir)"
    print $"  Packages: ($pkg_count)"
    print $"  Total size: ($total_size)"
    print ""
    print "Next steps:"
    print $"  1. Review changes: git diff ($assets_dir)"
    print "  2. Test build: cargo build -p hiroz-msgs"
    print $"  3. Commit: git add ($assets_dir) && git commit -m 'Update vendored ($distro) assets'"
}
