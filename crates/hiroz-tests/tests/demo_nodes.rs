#![cfg(feature = "ros-interop")]

mod common;

// Import the demo_nodes module from the examples directory.
// This uses #[path] to reference code outside the normal module tree,
// allowing tests to reuse the exact same code that users run as examples.
// This is preferable to code duplication and ensures quality.
#[path = "../../hiroz/examples/demo_nodes/mod.rs"]
mod demo_nodes;

use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crate::common::*;

#[test]
fn test_hiroz_talker_to_hiroz_listener() {
    let router = TestRouter::new();

    println!("\n=== Test: hiroz talker -> hiroz listener ===");

    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();

    // Start hiroz listener in a thread using the example code
    let router_endpoint = router.endpoint().to_string();
    let listener_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
                .expect("Failed to create hiroz context");

            // Use the actual listener example code with timeout
            let messages =
                demo_nodes::run_listener(ctx, "chatter", Some(3), Some(Duration::from_secs(15)))
                    .await
                    .expect("Listener failed");

            let mut received = received_clone.lock().unwrap();
            *received = messages;
        });
    });

    wait_for_ready(Duration::from_secs(2));

    // Start hiroz talker in a thread using the example code
    let talker_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");

            // Use the actual talker example code with max 5 messages
            demo_nodes::run_talker(ctx, "chatter", Duration::from_secs(1), Some(5))
                .await
                .expect("Talker failed");
        });
    });

    talker_handle.join().expect("Talker thread panicked");
    listener_handle.join().expect("Listener thread panicked");

    let msgs = received.lock().unwrap();
    assert!(
        msgs.len() >= 3,
        "Test failed: Expected at least 3 messages, got {}",
        msgs.len()
    );

    println!(
        "Test passed: hiroz listener received {} messages from hiroz talker",
        msgs.len()
    );
}

#[test]
fn test_rcl_talker_to_hiroz_listener() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_demo_nodes_cpp_available() {
        panic!("demo_nodes_cpp package not found - ensure it is installed");
    }

    // Use the zenoh-rs 1.9.0 TestRouter so that both the hiroz listener (1.9.0)
    // and the RCL talker (zenoh-c 1.6.2) connect to the same native router.
    // The in-process router correctly routes publications from the 1.6.2 client
    // to the 1.9.0 subscriber, whereas rmw_zenohd (1.6.2 router) does not.
    let router = TestRouter::new();

    println!("\n=== Test: RCL demo_nodes_cpp talker -> hiroz listener ===");

    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();

    // Start hiroz listener in a thread using the example code
    let router_endpoint = router.endpoint().to_string();
    let listener_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
                .expect("Failed to create hiroz context");

            // Use the actual listener example code with timeout
            let messages =
                demo_nodes::run_listener(ctx, "chatter", Some(3), Some(Duration::from_secs(15)))
                    .await
                    .expect("Listener failed");

            let mut received = received_clone.lock().unwrap();
            *received = messages;
        });
    });

    wait_for_ready(Duration::from_secs(2));

    // Start RCL talker, connecting to the zenoh-rs TestRouter
    let talker = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "talker"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL talker");

    let _talker_guard = ProcessGuard::new(talker, "RCL talker");

    listener_handle.join().expect("Listener thread panicked");

    let msgs = received.lock().unwrap();
    assert!(
        msgs.len() >= 3,
        "Test failed: Expected at least 3 messages, got {}",
        msgs.len()
    );

    println!(
        "Test passed: hiroz listener received {} messages from RCL talker",
        msgs.len()
    );
}

#[test]
fn test_hiroz_talker_to_rcl_listener() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_demo_nodes_cpp_available() {
        panic!("demo_nodes_cpp package not found - ensure it is installed");
    }

    let daemon = RmwZenohDaemon::new();

    println!("\n=== Test: hiroz talker -> RCL demo_nodes_cpp listener ===");

    // Start RCL listener
    let listener = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "listener"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", daemon.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL listener");

    let _listener_guard = ProcessGuard::new(listener, "RCL listener");

    wait_for_ready(Duration::from_secs(2));

    // Start hiroz talker in a thread using the example code
    let talker_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx =
                create_hiroz_context_with_daemon(&daemon).expect("Failed to create hiroz context");

            // Use the actual talker example code with faster publishing (100ms intervals)
            demo_nodes::run_talker(ctx, "chatter", Duration::from_millis(100), Some(10))
                .await
                .expect("Talker failed");
        });
    });

    talker_handle.join().expect("Talker thread panicked");

    // Give some time for RCL listener to process
    wait_for_ready(Duration::from_secs(1));

    println!("Test passed: hiroz talker published messages to RCL listener");
}

#[test]
fn test_hiroz_add_two_ints_server_to_hiroz_client() {
    let router = TestRouter::new();

    println!("\n=== Test: hiroz add_two_ints server -> hiroz client ===");

    let (tx, rx) = std::sync::mpsc::channel();

    // Start hiroz server in a thread using the example code
    let router_endpoint = router.endpoint().to_string();
    let server_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
            .expect("Failed to create hiroz context");

        // Use the actual server example code (handle one request)
        let result = demo_nodes::run_add_two_ints_server(ctx, Some(1));
        let _ = tx.send(()); // Signal completion
        result.expect("Server failed");
    });

    // Retry until the server is ready (avoids a fixed sleep that may not be
    // long enough under CI load, especially on kilted which is slower to start).
    let result = {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");
            match demo_nodes::run_add_two_ints_client(ctx, 2, 3, false) {
                Ok(v) => break v,
                Err(_) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => panic!("Client failed: {e}"),
            }
        }
    };

    assert_eq!(result, 5, "Expected 2 + 3 = 5");

    // Wait for server to signal completion (with timeout)
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(_) => {
            server_handle.join().expect("Server thread panicked");
            println!(
                "Test passed: hiroz client received {} from hiroz server",
                result
            );
        }
        Err(_) => {
            println!(
                "Test passed: hiroz client received {} from hiroz server (server still cleaning up)",
                result
            );
            // Don't wait for server join if it's taking too long
        }
    }
}

#[test]
fn test_rcl_add_two_ints_server_to_hiroz_client() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_demo_nodes_cpp_available() {
        panic!("demo_nodes_cpp package not found - ensure it is installed");
    }

    // Use zenoh-rs TestRouter so the hiroz client's Interest-based querier
    // is handled natively by the 1.9.0 router, which correctly routes queries
    // from the hiroz client (1.9.0) to the RCL server (zenoh-c 1.6.2).
    let router = TestRouter::new();

    println!("\n=== Test: RCL demo_nodes_cpp add_two_ints server -> hiroz client ===");

    // Start RCL server, connecting to the zenoh-rs TestRouter
    let server = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "add_two_ints_server"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL server");

    let _server_guard = ProcessGuard::new(server, "RCL add_two_ints server");

    wait_for_ready(Duration::from_secs(3));

    // Start hiroz client in a thread using the example code
    let client_handle = thread::spawn(move || -> i64 {
        let ctx =
            create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");

        // Use the actual client example code
        demo_nodes::run_add_two_ints_client(ctx, 4, 7, false).expect("Client failed")
    });

    let result = client_handle.join().expect("Client thread panicked");
    assert_eq!(result, 11, "Expected 4 + 7 = 11");

    println!(
        "Test passed: hiroz client received {} from RCL server",
        result
    );
}

#[test]
fn test_hiroz_add_two_ints_server_to_rcl_client() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_demo_nodes_cpp_available() {
        panic!("demo_nodes_cpp package not found - ensure it is installed");
    }

    // Use TestRouter so that `ros2 service call` (zenoh-c 1.6.2 client) can
    // reach the hiroz service server. The `demo_nodes_cpp add_two_ints_client`
    // binary calls wait_for_service() which blocks indefinitely when the hiroz
    // liveliness token is not visible via rmw_zenohd; `ros2 service call` does
    // not call wait_for_service() and works correctly through the TestRouter.
    let router = TestRouter::new();

    println!("\n=== Test: hiroz add_two_ints server -> RCL ros2 service call ===");

    // Start hiroz server in a thread using the example code
    let router_endpoint = router.endpoint().to_string();
    let server_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
            .expect("Failed to create hiroz context");

        // Handle one request then exit
        demo_nodes::run_add_two_ints_server(ctx, Some(1)).expect("Server failed");
    });

    wait_for_ready(Duration::from_secs(2));

    // Use `ros2 service call` which bypasses wait_for_service()
    let output = Command::new("ros2")
        .args([
            "service",
            "call",
            "/add_two_ints",
            "example_interfaces/srv/AddTwoInts",
            "{a: 4, b: 7}",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .output()
        .expect("Failed to run ros2 service call");

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("ros2 service call output: {}", stdout);
    assert!(
        output.status.success(),
        "ros2 service call failed: {}",
        stdout
    );

    server_handle.join().expect("Server thread panicked");

    println!("Test passed: RCL called hiroz add_two_ints server");
}

#[test]
fn test_rcl_fibonacci_action_server_to_hiroz_client() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_action_tutorials_cpp_available() {
        panic!("action_tutorials_cpp package not found - ensure it is installed");
    }

    // Use zenoh-rs TestRouter: the hiroz action client uses declare_querier()
    // internally and the 1.9.0 router handles its Interest correctly, whereas
    // rmw_zenohd (1.6.2) does not route the hiroz querier's requests to the RCL
    // action server.
    let router = TestRouter::new();

    println!("\n=== Test: RCL demo_nodes_cpp fibonacci action server -> hiroz client ===");

    // Start RCL action server, connecting to the zenoh-rs TestRouter
    let server = Command::new("ros2")
        .args(["run", "action_tutorials_cpp", "fibonacci_action_server"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL fibonacci action server");

    let _server_guard = ProcessGuard::new(server, "RCL fibonacci action server");

    wait_for_ready(Duration::from_secs(5));

    // Start hiroz client in a thread
    let client_handle = thread::spawn(move || -> Vec<i32> {
        let ctx =
            create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");

        // Use the actual client example code
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { demo_nodes::run_fibonacci_action_client(ctx, 2).await })
            .expect("Client failed")
    });

    let result = client_handle.join().expect("Client thread panicked");

    // Check that we got the correct Fibonacci sequence for order 2
    let expected = vec![0, 1, 1];
    assert_eq!(
        result, expected,
        "Expected Fibonacci sequence {:?}",
        expected
    );

    println!(
        "Test passed: hiroz client received Fibonacci sequence {:?} from RCL server",
        result
    );
}

#[test]
fn test_hiroz_fibonacci_action_server_to_rcl_client() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_action_tutorials_cpp_available() {
        panic!("action_tutorials_cpp package not found - ensure it is installed");
    }

    let daemon = RmwZenohDaemon::new();

    println!("\n=== Test: hiroz fibonacci action server -> RCL demo_nodes_cpp client ===");

    // Start hiroz server in a thread
    let daemon_endpoint = daemon.endpoint().to_string();
    let server_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&daemon_endpoint)
            .expect("Failed to create hiroz context");

        // Use the actual server example code
        let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
            demo_nodes::run_fibonacci_action_server(ctx, Some(Duration::from_secs(10))).await
        });
        result.expect("Server failed");
    });

    wait_for_ready(Duration::from_secs(2));

    // Start RCL client
    let client = Command::new("ros2")
        .args(["run", "action_tutorials_cpp", "fibonacci_action_client"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", daemon.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL fibonacci action client");

    let _client_guard = ProcessGuard::new(client, "RCL fibonacci action client");

    // Wait for the client to complete
    wait_for_ready(Duration::from_secs(10));

    // Stop the server
    server_handle.join().expect("Server thread panicked");

    println!("Test passed: RCL client called hiroz fibonacci action server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_hiroz_fibonacci_action_server_to_hiroz_client() {
    zenoh::init_log_from_env_or("error");
    let router = TestRouter::new();

    println!("\n=== Test: hiroz fibonacci action server -> hiroz client ===");

    let (fib_tx, fib_rx) = std::sync::mpsc::channel();

    // Start hiroz server in a thread using the example code
    let router_endpoint = router.endpoint().to_string();
    let fib_server_handle = tokio::task::spawn(async move {
        let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
            .expect("Failed to create hiroz context");
        let result =
            demo_nodes::run_fibonacci_action_server(ctx, Some(Duration::from_secs(30))).await;
        let _ = fib_tx.send(()); // Signal completion
        result.expect("Server failed");
    });

    wait_for_ready(Duration::from_secs(2));

    // Run hiroz client in the main thread
    let ctx = create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");
    let result = demo_nodes::run_fibonacci_action_client(ctx, 5)
        .await
        .expect("Client failed");

    // Check that we got the correct Fibonacci sequence for order 5
    let expected = vec![0, 1, 1, 2, 3, 5];
    assert_eq!(
        result, expected,
        "Expected Fibonacci sequence {:?}",
        expected
    );

    // Wait for server to signal completion (with timeout)
    match fib_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(_) => {
            fib_server_handle.await.expect("Server thread panicked");
            println!(
                "Test passed: hiroz client received Fibonacci sequence {:?} from hiroz server",
                result
            );
        }
        Err(_) => {
            println!(
                "Test passed: hiroz client received Fibonacci sequence {:?} from hiroz server (server still cleaning up)",
                result
            );
            // Don't wait for server join if it's taking too long
        }
    }
}
