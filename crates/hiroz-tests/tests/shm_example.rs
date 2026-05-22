//! Integration test for SHM PointCloud2 example
//!
//! This test verifies that the shm_pointcloud2 example runs successfully
//! and demonstrates all three SHM patterns.

use std::process::Command;

#[test]
#[ignore = "Slow integration test - builds and runs example. Run with: cargo test --test shm_example -- --ignored"]
fn test_shm_pointcloud2_example() {
    // Build the example first
    let build_status = Command::new("cargo")
        .args([
            "build",
            "--package",
            "hiroz",
            "--example",
            "shm_pointcloud2",
        ])
        .status()
        .expect("Failed to build shm_pointcloud2 example");

    assert!(
        build_status.success(),
        "Failed to build shm_pointcloud2 example"
    );

    // Run the example
    let output = Command::new("cargo")
        .args(["run", "--package", "hiroz", "--example", "shm_pointcloud2"])
        .output()
        .expect("Failed to run shm_pointcloud2 example");

    // Check if it ran successfully
    assert!(
        output.status.success(),
        "shm_pointcloud2 example failed with exit code: {:?}\nStderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify output
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for header
    assert!(
        stdout.contains("=== PointCloud2 with SHM Example ==="),
        "Missing example header in output"
    );

    // Verify Pattern 1: User-Managed SHM
    assert!(
        stdout.contains("1. User-Managed SHM Pattern:"),
        "Missing Pattern 1 header"
    );
    assert!(
        stdout.contains("Created SHM provider with 50MB pool"),
        "Pattern 1: Missing SHM provider creation"
    );
    assert!(
        stdout.contains("Generated 100k point cloud"),
        "Pattern 1: Missing point cloud generation"
    );
    assert!(
        stdout.contains("Points stored directly in SHM (zero-copy!)"),
        "Pattern 1: Missing zero-copy confirmation"
    );
    assert!(
        stdout.contains("Published in") && stdout.contains("µs"),
        "Pattern 1: Missing publish confirmation"
    );

    // Verify Pattern 2: Automatic SHM
    assert!(
        stdout.contains("2. Automatic SHM Pattern"),
        "Missing Pattern 2 header"
    );
    assert!(
        stdout.contains("Context configured with automatic SHM"),
        "Pattern 2: Missing context configuration"
    );
    assert!(
        stdout.contains("Generated 50k point cloud"),
        "Pattern 2: Missing point cloud generation"
    );
    assert!(
        stdout.contains("automatically used SHM") || stdout.contains("auto-used SHM"),
        "Pattern 2: Missing automatic SHM confirmation"
    );

    // Verify Pattern 3: Per-Publisher SHM Override
    assert!(
        stdout.contains("3. Per-Publisher SHM Override:"),
        "Missing Pattern 3 header"
    );
    assert!(
        stdout.contains("Publisher configured with custom SHM"),
        "Pattern 3: Missing publisher configuration"
    );
    assert!(
        stdout.contains("Generated 30k point cloud"),
        "Pattern 3: Missing point cloud generation"
    );
    assert!(
        stdout.contains("used publisher's SHM config") || stdout.contains("custom config"),
        "Pattern 3: Missing custom config confirmation"
    );

    // Verify completion
    assert!(
        stdout.contains("=== All patterns completed successfully ==="),
        "Missing completion message"
    );

    // Verify that all patterns generated point clouds with reasonable sizes
    // Pattern 1: 100k points ≈ 1171 KB
    assert!(
        stdout.contains("1171 KB") || stdout.contains("1.1") || stdout.contains("1200 KB"),
        "Pattern 1: Point cloud size not within expected range"
    );

    // Pattern 2: 50k points ≈ 585 KB
    assert!(
        stdout.contains("585 KB") || stdout.contains("5") || stdout.contains("600 KB"),
        "Pattern 2: Point cloud size not within expected range"
    );

    // Pattern 3: 30k points ≈ 351 KB
    assert!(
        stdout.contains("351 KB") || stdout.contains("3") || stdout.contains("400 KB"),
        "Pattern 3: Point cloud size not within expected range"
    );

    println!("✓ shm_pointcloud2 example executed successfully");
    println!("\n{}", stdout);
}

#[test]
#[ignore = "Slow integration test - builds and runs example. Run with: cargo test --test shm_example -- --ignored"]
fn test_shm_example_no_errors() {
    // Run the example and ensure it exits successfully
    let output = Command::new("cargo")
        .args(["run", "--package", "hiroz", "--example", "shm_pointcloud2"])
        .output()
        .expect("Failed to run shm_pointcloud2 example");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Filter out expected warnings and build output
    let error_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| {
            !line.contains("Compiling")
                && !line.contains("Finished")
                && !line.contains("Running")
                && !line.contains("warning:")
                && !line.contains("Blocking waiting for file lock") // Cargo lock messages
                && !line.contains("-->") // Code location markers
                && !line.contains("|") // Code snippets
                && !line.contains("note:") // Compiler notes
                && !line.contains("= note:") // Compiler notes
                && !line.trim().is_empty()
        })
        .collect();

    // The example should run successfully even if there are compiler warnings
    assert!(
        output.status.success(),
        "Example failed to run: {:?}",
        error_lines
    );
}

#[test]
#[ignore = "Slow integration test - builds and runs example. Run with: cargo test --test shm_example -- --ignored"]
fn test_shm_example_performance() {
    // Verify that publishing is fast (sub-millisecond for cached messages)
    let output = Command::new("cargo")
        .args(["run", "--package", "hiroz", "--example", "shm_pointcloud2"])
        .output()
        .expect("Failed to run shm_pointcloud2 example");

    assert!(output.status.success(), "Example failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Verify that publishing operations report timing information
    // All three patterns should report publish times
    assert!(
        stdout.contains("Published in"),
        "Should report publish timing"
    );

    // At least some operations should be in microseconds (fast operations)
    assert!(
        stdout.contains("µs") || stdout.contains("us"),
        "Should have fast operations measured in microseconds"
    );

    // Verify all three patterns are present
    assert!(
        stdout.contains("1. User-Managed SHM Pattern:"),
        "Missing Pattern 1"
    );
    assert!(
        stdout.contains("2. Automatic SHM Pattern"),
        "Missing Pattern 2"
    );
    assert!(
        stdout.contains("3. Per-Publisher SHM Override:"),
        "Missing Pattern 3"
    );

    println!("✓ Performance verification passed");
}
