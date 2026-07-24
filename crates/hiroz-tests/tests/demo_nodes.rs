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

// These 6 tests each spawn a real `ros2 run demo_nodes_cpp ...` subprocess
// (a full RCL/rclcpp C++ node). The `interop` nextest profile runs with
// test-threads=8 so the other 8 hiroz-only tests in this file stay parallel,
// but observed CI failures show all of the RCL-spawning tests hitting
// nextest's hard 60s kill *simultaneously* when several run concurrently --
// the runner can't keep up with that many RCL subprocesses starting at once.
// nextest runs each test in its own process, so serial_test's in-process
// static Mutex is a no-op there; it serializes them only under `cargo test`
// (single test binary). Under nextest, keep the interop suite from
// oversubscribing by limiting the runner's parallelism in CI.
#[test]
fn test_rcl_talker_to_hiroz_listener() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_demo_nodes_cpp_available() {
        panic!("demo_nodes_cpp package not found - ensure it is installed");
    }

    let router = TestRouter::new();

    println!("\n=== Test: RCL demo_nodes_cpp talker -> hiroz listener ===");

    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();

    // Start the RCL talker FIRST. demo_nodes_cpp talker publishes continuously
    // (1 Hz, forever until killed), so bringing it up before the listener
    // decouples the listener's receive window from runner stalls: under CI load
    // the gap between spawning the talker and the listener actually starting can
    // exceed several seconds, and if the listener's fixed-wall-clock timeout were
    // already ticking it could expire before the talker was discovered. Since the
    // talker keeps publishing, the listener only needs its window to overlap the
    // talker's steady stream — not to race a one-shot burst.
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

    wait_for_ready(Duration::from_secs(5));

    // Start hiroz listener in a thread using the example code. The 60s window is
    // generous on purpose: it must survive discovery latency spikes on loaded /
    // self-hosted runners (this step runs right after the job's clippy step).
    let router_endpoint = router.endpoint().to_string();
    let listener_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
                .expect("Failed to create hiroz context");

            // Use the actual listener example code with timeout
            let messages =
                demo_nodes::run_listener(ctx, "chatter", Some(3), Some(Duration::from_secs(60)))
                    .await
                    .expect("Listener failed");

            let mut received = received_clone.lock().unwrap();
            *received = messages;
        });
    });

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

    let router = TestRouter::new();

    println!("\n=== Test: hiroz talker -> RCL demo_nodes_cpp listener ===");

    // Start RCL listener
    let listener = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "listener"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
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
                create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");

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

    let router = TestRouter::new();

    println!("\n=== Test: RCL demo_nodes_cpp add_two_ints server -> hiroz client ===");

    // Start RCL server. stdout/stderr are piped (not discarded) so that if
    // discovery never completes we can tell whether the process is still
    // alive-but-slow or has actually exited/errored -- silently discarding
    // its output here was hiding that distinction entirely (this is how the
    // missing-ros2run-verb regression was originally found).
    let mut server = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "add_two_ints_server"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL server");

    let mut server_stdout = server.stdout.take().expect("stdout was piped");
    let mut server_stderr = server.stderr.take().expect("stderr was piped");
    let stdout_handle = thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut server_stdout, &mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut server_stderr, &mut buf);
        buf
    });

    let mut server_guard = ProcessGuard::new(server, "RCL add_two_ints server");

    wait_for_ready(Duration::from_secs(3));

    // Retry until the server's queryable is actually discovered -- same
    // rationale as the fibonacci RCL-server test below: this is discovery
    // latency, not response latency, so a longer fixed wait/timeout doesn't
    // reliably help under CI load.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let result = loop {
        let ctx =
            create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");
        match demo_nodes::run_add_two_ints_client(ctx, 4, 7, false) {
            Ok(v) => break v,
            Err(_) if std::time::Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                let exit_status = server_guard
                    .child
                    .as_mut()
                    .and_then(|c| c.try_wait().ok().flatten());
                // Terminate the server before joining the reader threads: they
                // block on read_to_string until stdout/stderr hit EOF, which only
                // happens once the child exits. In the alive-but-slow case (the
                // one this diagnostic exists for) joining first would hang.
                if let Some(c) = server_guard.child.as_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                let stdout = stdout_handle.join().unwrap_or_default();
                let stderr = stderr_handle.join().unwrap_or_default();
                panic!(
                    "Client failed: {e} (process exit status: {exit_status:?})\n\
                     stdout:\n{stdout}\nstderr:\n{stderr}"
                );
            }
        }
    };
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

    let router = TestRouter::new();

    println!("\n=== Test: hiroz add_two_ints server -> RCL demo_nodes_cpp client ===");

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

    // Generous discovery buffer: the RCL client is a one-shot external
    // process with no retry of its own, so the server's queryable must
    // already be discoverable by the time it starts.
    wait_for_ready(Duration::from_secs(5));

    // Start RCL client
    let mut client = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "add_two_ints_client"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL client");

    // Bound the wait on the RCL client's own exit instead of a fixed sleep,
    // so a slow-to-discover run fails fast with a clear message rather than
    // hanging the hiroz server thread (blocked on its one expected request)
    // until nextest's hard kill.
    let client_deadline = std::time::Instant::now() + Duration::from_secs(30);
    let client_status = loop {
        if let Some(status) = client.try_wait().expect("Failed to poll RCL client") {
            break Some(status);
        }
        if std::time::Instant::now() >= client_deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(200));
    };
    let _client_guard = ProcessGuard::new(client, "RCL add_two_ints client");
    // Check the exit status is actually success, not just that the process
    // exited -- an instantly-failing `ros2 run` (e.g. missing verb plugin,
    // bad args) also "exits within 30s" and would otherwise false-pass here.
    match client_status {
        None => panic!(
            "RCL add_two_ints client did not exit within 30s (likely failed to discover the hiroz server)"
        ),
        Some(status) if !status.success() => {
            panic!("RCL add_two_ints client exited with failure status {status:?}")
        }
        Some(_) => {}
    }

    // Wait for server to signal completion (with timeout)
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(_) => {
            server_handle.join().expect("Server thread panicked");
            println!("Test passed: RCL client called hiroz server");
        }
        Err(_) => {
            println!("Test passed: RCL client called hiroz server (server still cleaning up)");
        }
    }
}

#[test]
fn test_rcl_fibonacci_action_server_to_hiroz_client() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available - ensure ROS 2 is installed");
    }

    if !check_action_tutorials_cpp_available() {
        panic!("action_tutorials_cpp package not found - ensure it is installed");
    }

    let router = TestRouter::new();

    println!("\n=== Test: RCL demo_nodes_cpp fibonacci action server -> hiroz client ===");

    // Start RCL server
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

    wait_for_ready(Duration::from_secs(3));

    // Retry until the action server's queryables are actually discovered --
    // same rationale as the add_two_ints RCL-server test above: this is
    // discovery latency, not response latency, so a longer fixed wait/
    // timeout doesn't reliably help under CI load.
    let client_handle = thread::spawn(move || -> Vec<i32> {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");
            let result = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { demo_nodes::run_fibonacci_action_client(ctx, 2).await });
            match result {
                Ok(v) => break v,
                Err(_) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => panic!("Client failed: {e}"),
            }
        }
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

    let router = TestRouter::new();

    println!("\n=== Test: hiroz fibonacci action server -> RCL demo_nodes_cpp client ===");

    // Start hiroz server in a thread
    let router_endpoint = router.endpoint().to_string();
    let server_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&router_endpoint)
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
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
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
