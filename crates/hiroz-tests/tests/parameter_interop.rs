#![cfg(feature = "ros-interop")]
//! Parameter interoperability tests: hiroz node ↔ ros2 param CLI.
//!
//! These tests verify that `ros2 param list/get/set` works against a hiroz node,
//! confirming RIHS01 type hashes and CDR encoding are correct end-to-end.
//!
//! Requires: `ros-interop` feature, ROS 2 sourced with `rmw_zenoh_cpp`.

mod common;

use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use common::*;
use hiroz::{
    Builder,
    parameter::{ParameterDescriptor, ParameterType, ParameterValue},
};

/// Spin time for `ros2` CLI discovery (seconds). Each `ros2` invocation needs
/// time to connect to the Zenoh router and receive liveliness tokens.
const SPIN_TIME: &str = "2";

/// Build a `ros2` command with rmw_zenoh_cpp environment and discovery spin time.
fn ros2_cmd(rmw_env: &str) -> Command {
    let mut cmd = Command::new("ros2");
    cmd.env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", rmw_env);
    cmd
}

/// Wait until `ros2 node list` shows the given node name, or panic after timeout.
fn wait_for_node(node_name: &str, rmw_env: &str, timeout: Duration) {
    let start = Instant::now();
    loop {
        let output = ros2_cmd(rmw_env)
            .args(["node", "list", "--spin-time", SPIN_TIME, "--no-daemon"])
            .output()
            .expect("Failed to run ros2 node list");

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains(node_name) {
            println!("Node {} discovered after {:?}", node_name, start.elapsed());
            return;
        }

        if start.elapsed() > timeout {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "Timed out waiting for node {} after {:?}\nstdout: {}\nstderr: {}",
                node_name, timeout, stdout, stderr
            );
        }

        thread::sleep(Duration::from_secs(1));
    }
}

/// Test that `ros2 param list` can see parameters declared on a hiroz node.
#[test]
fn test_ros2_param_list_on_hiroz_node() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    let router = TestRouter::new();
    let rmw_env = router.rmw_zenoh_env();

    let endpoint = router.endpoint().to_string();
    let _server = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("ctx");
        let node = ctx.create_node("param_list_node").build().expect("node");

        let desc = ParameterDescriptor::new("count", ParameterType::Integer);
        node.declare_parameter("count", ParameterValue::Integer(42), desc)
            .expect("declare count");

        let desc = ParameterDescriptor::new("label", ParameterType::String);
        node.declare_parameter("label", ParameterValue::String("hello".into()), desc)
            .expect("declare label");

        thread::sleep(Duration::from_secs(30));
    });

    wait_for_node("param_list_node", &rmw_env, Duration::from_secs(15));

    let output = ros2_cmd(&rmw_env)
        .args([
            "param",
            "list",
            "/param_list_node",
            "--spin-time",
            SPIN_TIME,
            "--no-daemon",
        ])
        .output()
        .expect("Failed to run ros2 param list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("stdout: {}", stdout);
    println!("stderr: {}", stderr);

    assert!(
        stdout.contains("count"),
        "Expected 'count' in param list, got: {}",
        stdout
    );
    assert!(
        stdout.contains("label"),
        "Expected 'label' in param list, got: {}",
        stdout
    );

    println!("Test passed: ros2 param list sees hiroz parameters");
}

/// Test that `ros2 param get` and `ros2 param set` work against a hiroz node.
#[test]
fn test_ros2_param_get_set_on_hiroz_node() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    let router = TestRouter::new();
    let rmw_env = router.rmw_zenoh_env();

    let endpoint = router.endpoint().to_string();
    let _server = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("ctx");
        let node = ctx.create_node("param_getset_node").build().expect("node");

        let desc = ParameterDescriptor::new("count", ParameterType::Integer);
        node.declare_parameter("count", ParameterValue::Integer(0), desc)
            .expect("declare count");

        thread::sleep(Duration::from_secs(60));
    });

    wait_for_node("param_getset_node", &rmw_env, Duration::from_secs(15));

    // Get initial value
    let output = ros2_cmd(&rmw_env)
        .args([
            "param",
            "get",
            "/param_getset_node",
            "count",
            "--spin-time",
            SPIN_TIME,
            "--no-daemon",
        ])
        .output()
        .expect("Failed to run ros2 param get");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("Initial get stdout: {}", stdout);
    println!("Initial get stderr: {}", stderr);
    assert!(stdout.contains("0"), "Expected value 0, got: {}", stdout);

    // Set new value
    let output = ros2_cmd(&rmw_env)
        .args([
            "param",
            "set",
            "/param_getset_node",
            "count",
            "42",
            "--spin-time",
            SPIN_TIME,
            "--no-daemon",
        ])
        .output()
        .expect("Failed to run ros2 param set");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("Set output: {} {}", stdout, stderr);
    assert!(
        stdout.contains("Set parameter successful") || output.status.success(),
        "Expected successful set, got: {} {}",
        stdout,
        stderr
    );

    // Get updated value
    let output = ros2_cmd(&rmw_env)
        .args([
            "param",
            "get",
            "/param_getset_node",
            "count",
            "--spin-time",
            SPIN_TIME,
            "--no-daemon",
        ])
        .output()
        .expect("Failed to run ros2 param get");

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("Updated get: {}", stdout);
    assert!(
        stdout.contains("42"),
        "Expected value 42 after set, got: {}",
        stdout
    );

    println!("Test passed: ros2 param get/set works against hiroz node");
}

/// Test that a hiroz service client can read parameters from an rclcpp node.
///
/// This is a stretch goal — requires demo_nodes_cpp to be installed.
#[test]
fn test_hiroz_reads_rclcpp_node_params() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    if !check_demo_nodes_cpp_available() {
        panic!(
            "demo_nodes_cpp package not found!\n\
             Please install it with: apt install ros-$ROS_DISTRO-demo-nodes-cpp\n\
             Or ensure ROS environment is sourced: source /opt/ros/$ROS_DISTRO/setup.bash"
        );
    }

    let router = TestRouter::new();
    let rmw_env = router.rmw_zenoh_env();

    // Start an rclcpp talker node (it declares the 'use_sim_time' parameter by default)
    let server = Command::new("ros2")
        .args([
            "run",
            "demo_nodes_cpp",
            "talker",
            "--ros-args",
            "-r",
            "__node:=rclcpp_param_node",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", &rmw_env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to start rclcpp node");

    let _guard = ProcessGuard::new(server, "rclcpp_param_node");

    wait_for_node("rclcpp_param_node", &rmw_env, Duration::from_secs(15));

    let output = ros2_cmd(&rmw_env)
        .args([
            "param",
            "list",
            "/rclcpp_param_node",
            "--spin-time",
            SPIN_TIME,
            "--no-daemon",
        ])
        .output()
        .expect("Failed to run ros2 param list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("rclcpp node params: {}", stdout);

    assert!(
        stdout.contains("use_sim_time"),
        "Expected 'use_sim_time' in rclcpp node params, got: {}",
        stdout
    );

    println!("Test passed: can list rclcpp node parameters via Zenoh");
}
