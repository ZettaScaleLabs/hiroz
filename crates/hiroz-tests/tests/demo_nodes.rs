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

// Each RCL interop test launches a real `ros2 run demo_nodes_cpp` C++ node;
// running many at once oversubscribes the runner and starves discovery.
// serial_test's Mutex only serializes under `cargo test` (one binary) — nextest
// runs each test in its own process, so parallelism is capped by the `interop`
// profile (test-threads = 8).
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

    // Start the talker first: it publishes continuously (1 Hz until killed), so
    // the listener's window only needs to overlap that steady stream rather than
    // race a startup burst while its fixed timeout ticks through CI-load stalls.
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

    // Proceed as soon as the RCL talker is discoverable, not after a blind sleep.
    wait_for_ros_node("talker", &router, Duration::from_secs(15));

    // Start hiroz listener via the example code. 60s window absorbs discovery
    // latency spikes on loaded self-hosted runners.
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
    let mut listener = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "listener"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        // ROS 2 logging goes to stderr by default, so the listener's `I heard`
        // lines are not on stdout. Capture both.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL listener");

    let listener_output = common::OutputCapture::start(&mut listener);
    let _listener_guard = ProcessGuard::new(listener, "RCL listener");

    // Proceed as soon as the RCL listener is discoverable, not after a blind sleep.
    wait_for_ros_node("listener", &router, Duration::from_secs(15));

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

    // Poll for the listener's output on a deadline rather than sampling once
    // after a fixed sleep. The talker publishes a finite burst of 10 messages
    // over ~900ms, so a single sample races CI-load stalls. The reverse
    // direction (`test_rcl_talker_to_hiroz_listener`) documents the same hazard
    // and solves it the same way.
    //
    // Assert the C++ listener actually received. Without this the test passes
    // whether or not the listener works at all -- it could crash on the first
    // sample and nothing here would notice. The listener never exits on its
    // own, so read what it has printed so far rather than waiting for EOF.
    let heard_deadline = std::time::Instant::now() + Duration::from_secs(15);
    let heard = loop {
        let snap = listener_output.snapshot();
        if snap.contains("I heard") || std::time::Instant::now() >= heard_deadline {
            break snap;
        }
        thread::sleep(Duration::from_millis(200));
    };
    assert!(
        heard.contains("I heard"),
        "the C++ listener printed no `I heard` line, so it received nothing from the hiroz \
         talker. Captured output:\n{heard}"
    );

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

    // Pipe stdout/stderr (not discard) so a discovery failure can be told apart
    // as alive-but-slow vs exited/errored — discarding output hid the
    // missing-ros2run-verb regression.
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

    // Proceed as soon as the RCL server node is discoverable, not after a blind sleep.
    wait_for_ros_node("add_two_ints_server", &router, Duration::from_secs(15));

    // Retry until the server's queryable is discovered — discovery latency, not
    // response latency, so a longer fixed timeout doesn't help under load. Cap
    // retries at 40s to fail fast; each attempt can block ~15s internally, well
    // clear of the interop profile's 120s kill (60s period × terminate-after 2).
    let attempt_worst_case = Duration::from_secs(15);
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let result = loop {
        let ctx =
            create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");
        // Don't start an attempt that can't finish before the 40s deadline.
        let out_of_budget = std::time::Instant::now() + attempt_worst_case >= deadline;
        match demo_nodes::run_add_two_ints_client(ctx, 4, 7, false) {
            Ok(v) => break v,
            Err(_) if !out_of_budget => {
                thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                let exit_status = server_guard
                    .child
                    .as_mut()
                    .and_then(|c| c.try_wait().ok().flatten());
                // Kill the server before joining readers: read_to_string blocks
                // until EOF (child exit), so joining first would hang in the
                // alive-but-slow case. Take the child out of the guard so its
                // Drop can't later re-signal this (possibly recycled) PID group.
                if let Some(mut c) = server_guard.child.take() {
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

    // One-shot RCL client with no retry: the server's queryable must be
    // discoverable before it starts.
    wait_for_ready(Duration::from_secs(5));

    // Start RCL client with both streams piped, and drain them on reader threads
    // (see `OutputCapture`). `ros2 run` reports why it failed on stderr; without
    // that text a failure here is only diagnosable as an exit code. Draining
    // concurrently is what keeps piping safe for the try_wait loop below — an
    // unread pipe would fill and block the child.
    let mut client = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "add_two_ints_client"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL client");

    let client_output = common::OutputCapture::start(&mut client);

    // Guard owns the child throughout; poll try_wait through the guard's own
    // handle. Reaping via a separate handle first could let Drop later signal a
    // recycled PID's process group.
    let mut client_guard = ProcessGuard::new(client, "RCL add_two_ints client");

    // Bound on the client's actual exit (30s), not a fixed sleep: a slow-to-
    // discover run fails with a diagnostic instead of stalling until nextest's kill.
    let client_deadline = std::time::Instant::now() + Duration::from_secs(30);
    let client_status = loop {
        let child = client_guard
            .child
            .as_mut()
            .expect("client child owned by guard");
        if let Some(status) = child.try_wait().expect("Failed to poll RCL client") {
            // Reaped: take it out of the guard so `Drop` cannot signal a PID the
            // OS may have recycled. See the action test for the same reasoning.
            client_guard.child.take();
            break Some(status);
        }
        if std::time::Instant::now() >= client_deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(200));
    };
    // Require success, not just exit: an instantly-failing `ros2 run` (missing
    // verb, bad args) also exits within 30s and would false-pass.
    match client_status {
        None => {
            // Kill the child before rendering. `finish()` joins the reader
            // threads, and they block on the open pipes until the child exits.
            // In this branch the child provably has not exited, so calling
            // `finish()` first would hang until nextest's kill and print
            // nothing -- the exact failure this capture exists to prevent.
            drop(client_guard);
            panic!(
                "RCL add_two_ints client did not exit within 30s (likely failed to discover the hiroz server)\n{}",
                client_output.finish()
            )
        }
        Some(status) if !status.success() => {
            panic!(
                "RCL add_two_ints client exited with failure status {status:?}\n{}",
                client_output.finish()
            )
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

    // Proceed as soon as the RCL action server is discoverable, not after a blind sleep.
    wait_for_ros_node("fibonacci_action_server", &router, Duration::from_secs(15));

    // Retry until the action server's queryables are discovered — discovery
    // latency, not response latency, so a longer fixed timeout doesn't help.
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
    let mut client = Command::new("ros2")
        .args(["run", "action_tutorials_cpp", "fibonacci_action_client"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start RCL fibonacci action client");

    let client_output = common::OutputCapture::start(&mut client);
    let mut client_guard = ProcessGuard::new(client, "RCL fibonacci action client");

    // Wait on the client's actual exit rather than a fixed sleep, and require it
    // to succeed. Previously this test slept and then declared success without
    // looking at the client at all: it passed whether the client completed the
    // action, crashed, or never started.
    // Bound the wait by the server's own lifetime, not by a round number. The
    // server above runs for 10s and the client starts ~2s into that, so a client
    // that has not succeeded by then never will -- the server is gone. A longer
    // deadline would only delay the report.
    const CLIENT_BUDGET: Duration = Duration::from_secs(15);
    let deadline = std::time::Instant::now() + CLIENT_BUDGET;
    let client_status = loop {
        let child = client_guard
            .child
            .as_mut()
            .expect("client child owned by guard");
        if let Some(status) = child.try_wait().expect("Failed to poll RCL action client") {
            // The child is reaped, so its PID is free for the OS to reuse. Take
            // it out of the guard: `ProcessGuard::drop` signals the process
            // group by negative PID, and that runs only after the 10s server
            // thread joins below. By then the PGID may belong to something else.
            client_guard.child.take();
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(200));
    };

    match client_status {
        None => {
            // Kill the child before rendering -- see the same branch in
            // `test_hiroz_add_two_ints_server_to_rcl_client` for why.
            drop(client_guard);
            panic!(
                "RCL fibonacci action client did not exit within {}s\n{}",
                CLIENT_BUDGET.as_secs(),
                client_output.finish()
            )
        }
        Some(status) if !status.success() => panic!(
            "RCL fibonacci action client exited with failure status {status:?}\n{}",
            client_output.finish()
        ),
        Some(_) => {}
    }

    // A zero exit status does not prove the client completed the action.
    // `action_tutorials_cpp` calls `rclcpp::shutdown()` when its 10s
    // `wait_for_action_server` expires, so the client exits 0 after printing
    // "Action server not available after waiting" -- which is exactly the case
    // this test exists to catch. Require the marker that only the result
    // callback emits.
    let client_out = client_output.finish();
    assert!(
        client_out.contains("Result received"),
        "the C++ action client exited 0 but never reported a result, so it did not complete \
         the action against the hiroz server. Captured output:\n{client_out}"
    );

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
