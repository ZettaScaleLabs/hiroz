//! Python interoperability tests for hiroz
//!
//! Tests pub/sub and service communication between Rust (hiroz) and Python (hiroz-py).
//!
//! Run with: cargo test --features python-interop -p hiroz-tests --test python_interop

#![cfg(feature = "python-interop")]

mod common;

use std::{
    io::{BufRead, BufReader},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use common::{
    ProcessGuard, TestRouter, check_python_venv_available, create_hiroz_context_with_endpoint,
    spawn_python_listener, spawn_python_service_client, spawn_python_service_server,
    spawn_python_talker,
};
use hiroz::Builder;
use hiroz_msgs::{
    example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts},
    std_msgs,
};
use serial_test::serial;

/// Helper to read stdout from a ProcessGuard
fn read_process_output(guard: &mut ProcessGuard) -> Vec<String> {
    if let Some(child) = guard.child.as_mut() {
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            return reader.lines().map_while(Result::ok).collect();
        }
    }
    Vec::new()
}

/// Helper to wait for a specific line in process output
#[allow(dead_code)]
fn wait_for_output_line(
    guard: &mut ProcessGuard,
    prefix: &str,
    timeout: Duration,
) -> Option<String> {
    if let Some(child) = guard.child.as_mut() {
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let start = std::time::Instant::now();
            for line in reader.lines().map_while(Result::ok) {
                println!("  [py] {}", line);
                if line.starts_with(prefix) {
                    return Some(line);
                }
                if start.elapsed() > timeout {
                    break;
                }
            }
        }
    }
    None
}

// ============================================================================
// Pub/Sub Tests
// ============================================================================

/// Test: Python publisher -> Rust subscriber
#[test]
#[serial]
fn test_python_pub_rust_sub() {
    if !check_python_venv_available() {
        println!("Skipping test: Python venv not available");
        return;
    }

    let router = TestRouter::new();
    let topic = "/test_py_pub_rust_sub";

    // Start Rust subscriber in a thread
    let received = Arc::new(Mutex::new(Vec::new()));
    let recv_clone = received.clone();
    let endpoint = router.endpoint().to_string();

    let sub_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("Failed to create context");
        let node = ctx
            .create_node("rust_sub")
            .build()
            .expect("Failed to create node");
        let sub = node
            .create_sub::<std_msgs::String>(topic)
            .build()
            .expect("Failed to create subscriber");

        println!("Rust subscriber ready, waiting for messages...");

        for _ in 0..10 {
            if let Ok(msg) = sub.recv_timeout(Duration::from_secs(2)) {
                println!("  [rust] Received: {}", msg.data);
                recv_clone.lock().unwrap().push(msg.data.clone());
            }
        }
    });

    // Give Rust subscriber time to set up
    thread::sleep(Duration::from_secs(1));

    // Start Python talker (publisher)
    let mut py_pub = spawn_python_talker(router.endpoint(), topic, 5);

    // Wait for Python publisher to finish
    let output = read_process_output(&mut py_pub);
    for line in &output {
        println!("  [py] {}", line);
    }

    // Wait for Rust subscriber
    sub_handle.join().expect("Subscriber thread panicked");

    // Validate
    let msgs = received.lock().unwrap();
    println!("Received {} messages", msgs.len());
    assert!(
        msgs.len() >= 3,
        "Expected at least 3 messages, got {}",
        msgs.len()
    );
}

/// Test: Rust publisher -> Python subscriber
#[test]
#[serial]
fn test_rust_pub_python_sub() {
    if !check_python_venv_available() {
        println!("Skipping test: Python venv not available");
        return;
    }

    let router = TestRouter::new();
    let topic = "/test_rust_pub_py_sub";

    // Start Python listener (subscriber)
    let mut py_sub = spawn_python_listener(router.endpoint(), topic, 10.0);

    // Give Python subscriber time to set up
    thread::sleep(Duration::from_secs(2));

    // Publish from Rust
    let endpoint = router.endpoint().to_string();
    let pub_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("Failed to create context");
        let node = ctx
            .create_node("rust_pub")
            .build()
            .expect("Failed to create node");
        let zpub = node
            .create_pub::<std_msgs::String>(topic)
            .build()
            .expect("Failed to create publisher");

        // Wait for subscriber discovery
        thread::sleep(Duration::from_secs(1));

        for i in 0..5 {
            let msg = std_msgs::String {
                data: format!("Hello from Rust {}", i),
            };
            zpub.publish(&msg).expect("Failed to publish");
            println!("  [rust] Published: {}", msg.data);
            thread::sleep(Duration::from_millis(300));
        }
    });

    pub_handle.join().expect("Publisher thread panicked");

    // Wait a bit for messages to arrive
    thread::sleep(Duration::from_secs(1));

    // Read Python output
    let output = read_process_output(&mut py_sub);
    for line in &output {
        println!("  [py] {}", line);
    }

    // Validate - look for SUB: lines
    let received_count = output
        .iter()
        .filter(|line| line.starts_with("SUB:Hello from Rust"))
        .count();

    println!("Python received {} messages", received_count);
    assert!(
        received_count >= 3,
        "Expected Python to receive at least 3 messages, got {}",
        received_count
    );
}

// ============================================================================
// Service Tests
// ============================================================================

/// Test: Python service server -> Rust client
#[test]
#[serial]
fn test_python_server_rust_client() {
    if !check_python_venv_available() {
        println!("Skipping test: Python venv not available");
        return;
    }

    let router = TestRouter::new();
    let service = "/test_py_server_rust_client";

    // Start Python server
    let mut py_server = spawn_python_service_server(router.endpoint(), service, 1);

    // Wait for server to be ready
    thread::sleep(Duration::from_secs(2));

    // Call from Rust
    let endpoint = router.endpoint().to_string();
    let result = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("Failed to create context");
        let node = ctx
            .create_node("rust_client")
            .build()
            .expect("Failed to create node");
        let client = node
            .create_client::<AddTwoInts>(service)
            .build()
            .expect("Failed to create client");

        // Wait for service discovery
        thread::sleep(Duration::from_secs(1));

        let request = AddTwoIntsRequest { a: 5, b: 7 };
        println!("  [rust] Sending request: {} + {}", request.a, request.b);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            client
                .call_with_timeout(&request, Duration::from_secs(5))
                .await
                .map(|resp| resp.sum)
        })
    })
    .join()
    .expect("Client thread panicked");

    // Read Python output
    let output = read_process_output(&mut py_server);
    for line in &output {
        println!("  [py] {}", line);
    }

    // Validate
    assert!(result.is_ok(), "Failed to get response from Python server");
    assert_eq!(result.unwrap(), 12, "Expected 5 + 7 = 12");
}

/// Test: Rust service server -> Python client
#[test]
#[serial]
fn test_rust_server_python_client() {
    if !check_python_venv_available() {
        println!("Skipping test: Python venv not available");
        return;
    }

    let router = TestRouter::new();
    let service = "/test_rust_server_py_client";
    let endpoint = router.endpoint().to_string();

    // Start Rust server in a thread
    let server_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("Failed to create context");
        let node = ctx
            .create_node("rust_server")
            .build()
            .expect("Failed to create node");
        let mut server = node
            .create_service::<AddTwoInts>(service)
            .build()
            .expect("Failed to create server");

        println!("  [rust] Server ready, waiting for request...");

        // Handle one request with polling
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if let Ok(req) = server.take_request() {
                let sum = req.message().a + req.message().b;
                println!(
                    "  [rust] Received: {} + {} = {}",
                    req.message().a,
                    req.message().b,
                    sum
                );

                let response = AddTwoIntsResponse { sum };
                req.reply_blocking(&response)
                    .expect("Failed to send response");
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    // Give Rust server time to set up
    thread::sleep(Duration::from_secs(2));

    // Start Python client
    let mut py_client = spawn_python_service_client(router.endpoint(), service, 10, 32);

    // Wait for client to finish
    let output = read_process_output(&mut py_client);
    for line in &output {
        println!("  [py] {}", line);
    }

    server_handle.join().expect("Server thread panicked");

    // Validate Python got correct response
    let response_line = output
        .iter()
        .find(|line| line.starts_with("CLIENT:RESPONSE:"));
    assert!(
        response_line.is_some(),
        "Python client should have received response"
    );
    assert!(
        response_line.unwrap().contains("42"),
        "Expected 10 + 32 = 42, got: {}",
        response_line.unwrap()
    );
}
