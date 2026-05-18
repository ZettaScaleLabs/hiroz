#![cfg(feature = "ros-interop")]

mod common;

use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts};

#[test]
fn test_hiroz_server_hiroz_client() {
    let router = TestRouter::new();

    println!("\n=== Test: hiroz server <-> hiroz client ===");

    // Start server in background thread
    let router_endpoint = router.endpoint().to_string();
    let _server_handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&router_endpoint).expect("Failed to create context");

        let node = ctx
            .create_node("test_server")
            .build()
            .expect("Failed to create node");

        let mut zsrv = node
            .create_service::<AddTwoInts>("add_two_ints_test1")
            .build()
            .expect("Failed to create service");

        println!("Server ready, waiting for requests...");

        // Handle one request
        if let Ok(req) = zsrv.take_request() {
            println!(
                "Received request: {} + {}",
                req.message().a,
                req.message().b
            );
            let resp = AddTwoIntsResponse {
                sum: req.message().a + req.message().b,
            };
            println!("Sending response: {}", resp.sum);
            req.reply_blocking(&resp).expect("Failed to send response");
        }
    });

    wait_for_ready(Duration::from_secs(3));

    // Run client
    let client_handle = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("test_client")
                .build()
                .expect("Failed to create node");

            let zcli = node
                .create_client::<AddTwoInts>("add_two_ints_test1")
                .build()
                .expect("Failed to create client");

            println!("Client ready, waiting for service discovery...");
            // Give some time for service discovery to complete
            tokio::time::sleep(Duration::from_millis(500)).await;

            println!("Sending request...");

            let resp = zcli
                .call_with_timeout(&AddTwoIntsRequest { a: 5, b: 3 }, Duration::from_secs(5))
                .await
                .expect("Failed to receive response");
            println!("Received response: {}", resp.sum);

            assert_eq!(resp.sum, 8, "Expected 5 + 3 = 8");
            resp
        })
    });

    let result = client_handle.join().expect("Client thread panicked");
    assert_eq!(result.sum, 8);
    println!("Test passed: hiroz service call successful");
}

#[test]
fn test_hiroz_server_hiroz_client_multipart_name() {
    let router = TestRouter::new();

    println!("\n=== Test: hiroz server <-> hiroz client with multi-part service name ===");

    let router_endpoint = router.endpoint().to_string();
    let _server_handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&router_endpoint).expect("Failed to create context");

        let node = ctx
            .create_node("test_server")
            .build()
            .expect("Failed to create node");

        let mut zsrv = node
            .create_service::<AddTwoInts>("/ns/add_two_ints_multi")
            .build()
            .expect("Failed to create service");

        println!("Server ready (multi-part name), waiting for requests...");

        if let Ok(req) = zsrv.take_request() {
            println!(
                "Received request: {} + {}",
                req.message().a,
                req.message().b
            );
            let resp = AddTwoIntsResponse {
                sum: req.message().a + req.message().b,
            };
            req.reply_blocking(&resp).expect("Failed to send response");
        }
    });

    wait_for_ready(Duration::from_secs(3));

    let client_handle = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("test_client")
                .build()
                .expect("Failed to create node");

            let zcli = node
                .create_client::<AddTwoInts>("/ns/add_two_ints_multi")
                .build()
                .expect("Failed to create client");

            println!("Client ready, waiting...");
            tokio::time::sleep(Duration::from_millis(500)).await;

            let resp = zcli
                .call_with_timeout(&AddTwoIntsRequest { a: 7, b: 5 }, Duration::from_secs(5))
                .await
                .expect("Failed to receive response");
            println!("Received response: {}", resp.sum);

            assert_eq!(resp.sum, 12, "Expected 7 + 5 = 12");
            resp
        })
    });

    let result = client_handle.join().expect("Client thread panicked");
    assert_eq!(result.sum, 12);
    println!("Test passed: hiroz multi-part service call successful");
}

#[test]
fn test_hiroz_server_ros2_client() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    let router = TestRouter::new();

    println!("\n=== Test: hiroz server <-> ROS2 client ===");

    // Start hiroz server
    let router_endpoint = router.endpoint().to_string();
    let _server = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&router_endpoint).expect("Failed to create context");

        let node = ctx
            .create_node("hiroz_server")
            .build()
            .expect("Failed to create node");

        let mut zsrv = node
            .create_service::<AddTwoInts>("add_two_ints_test2")
            .build()
            .expect("Failed to create service");

        println!("Server ready for ROS2 client...");

        // Handle one request
        if let Ok(req) = zsrv.take_request() {
            println!(
                "Received request from ROS2: {} + {}",
                req.message().a,
                req.message().b
            );
            let resp = AddTwoIntsResponse {
                sum: req.message().a + req.message().b,
            };
            println!("Sending response: {}", resp.sum);
            req.reply_blocking(&resp).expect("Failed to send response");
        }
    });

    wait_for_ready(Duration::from_secs(10));

    // Call from ros2 CLI
    let output = Command::new("timeout")
        .args([
            "5",
            "ros2",
            "service",
            "call",
            "/add_two_ints_test2",
            "example_interfaces/srv/AddTwoInts",
            "{a: 10, b: 7}",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .output()
        .expect("Failed to call service");

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("ROS2 output: {}", stdout);
    assert!(
        stdout.contains("sum: 17") || stdout.contains("sum=17"),
        "Expected sum: 17, got: {}",
        stdout
    );

    println!("Test passed: ROS2 client called hiroz service");
}

#[test]
fn test_hiroz_server_ros2_client_multipart() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    let router = TestRouter::new();

    println!("\n=== Test: hiroz server <-> ROS2 client with multi-part service name ===");

    // Use a multi-part service name: "ns/add_two_ints_test3" (relative, qualifies to
    // "/ns/add_two_ints_test3"). This verifies that multi-part service names work across
    // the hiroz ↔ rmw_zenoh_cpp boundary.
    let router_endpoint = router.endpoint().to_string();
    let _server = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&router_endpoint).expect("Failed to create context");

        let node = ctx
            .create_node("hiroz_server_mp")
            .build()
            .expect("Failed to create node");

        let mut zsrv = node
            .create_service::<AddTwoInts>("ns/add_two_ints_test3")
            .build()
            .expect("Failed to create service");

        println!("Server ready for ROS2 client (multi-part name)...");

        if let Ok(req) = zsrv.take_request() {
            println!(
                "Received request from ROS2: {} + {}",
                req.message().a,
                req.message().b
            );
            let resp = AddTwoIntsResponse {
                sum: req.message().a + req.message().b,
            };
            println!("Sending response: {}", resp.sum);
            req.reply_blocking(&resp).expect("Failed to send response");
        }
    });

    wait_for_ready(Duration::from_secs(10));

    let output = Command::new("timeout")
        .args([
            "5",
            "ros2",
            "service",
            "call",
            "/ns/add_two_ints_test3",
            "example_interfaces/srv/AddTwoInts",
            "{a: 6, b: 8}",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .output()
        .expect("Failed to call service");

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("ROS2 output: {}", stdout);
    assert!(
        stdout.contains("sum: 14") || stdout.contains("sum=14"),
        "Expected sum: 14, got: {}",
        stdout
    );

    println!("Test passed: ROS2 client called hiroz service (multi-part name)");
}

#[test]
fn test_ros2_server_hiroz_client_multipart() {
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

    println!("\n=== Test: ROS2 server <-> hiroz client with multi-part service name ===");

    // Remap the default "add_two_ints" service to a multi-part absolute name.
    let server = Command::new("ros2")
        .args([
            "run",
            "demo_nodes_cpp",
            "add_two_ints_server",
            "--ros-args",
            "-r",
            "add_two_ints:=/ns/add_two_ints_ros2_mp",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start ROS2 server");

    let _guard = ProcessGuard::new(server, "ros2 server (multi-part)");

    wait_for_ready(Duration::from_secs(3));

    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("hiroz_client_mp")
                .build()
                .expect("Failed to create node");

            let zcli = node
                .create_client::<AddTwoInts>("/ns/add_two_ints_ros2_mp")
                .build()
                .expect("Failed to create client");

            println!("Client ready, waiting for service discovery...");
            tokio::time::sleep(Duration::from_millis(500)).await;

            println!("Calling ROS2 multi-part server...");

            let resp = zcli
                .call_with_timeout(
                    &AddTwoIntsRequest { a: 11, b: 13 },
                    std::time::Duration::from_secs(5),
                )
                .await
                .expect("Failed to receive response");
            println!("Received response from ROS2: {}", resp.sum);

            resp
        })
    })
    .join()
    .expect("Client thread panicked");

    assert_eq!(result.sum, 24, "Expected 11 + 13 = 24");

    println!("Test passed: hiroz client called ROS2 service (multi-part name)");
}

#[test]
fn test_ros2_server_hiroz_client() {
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

    println!("\n=== Test: ROS2 server <-> hiroz client ===");

    // Start ROS2 server with service name remapping to avoid conflicts
    let server = Command::new("ros2")
        .args([
            "run",
            "demo_nodes_cpp",
            "add_two_ints_server",
            "--ros-args",
            "-r",
            "add_two_ints:=add_two_ints_test_ros2",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start ROS2 server");

    let _guard = ProcessGuard::new(server, "ros2 server");

    wait_for_ready(Duration::from_secs(3));

    // Call from hiroz client
    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("hiroz_client")
                .build()
                .expect("Failed to create node");

            let zcli = node
                .create_client::<AddTwoInts>("add_two_ints_test_ros2")
                .build()
                .expect("Failed to create client");

            println!("Client ready, waiting for service discovery...");
            // Give some time for service discovery to complete
            tokio::time::sleep(Duration::from_millis(500)).await;

            println!("Calling ROS2 server...");

            let resp = zcli
                .call_with_timeout(
                    &AddTwoIntsRequest { a: 15, b: 9 },
                    std::time::Duration::from_secs(5),
                )
                .await
                .expect("Failed to receive response");
            println!("Received response from ROS2: {}", resp.sum);

            resp
        })
    })
    .join()
    .expect("Client thread panicked");

    assert_eq!(result.sum, 24, "Expected 15 + 9 = 24");

    println!("Test passed: hiroz client called ROS2 service");
}
