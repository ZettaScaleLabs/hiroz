#![cfg(feature = "ros-interop")]

mod common;

use std::{sync::Arc, time::Duration};

use common::*;
use hiroz::{Builder, action::server::ExecutingGoal};
// Distro-specific action interfaces:
// - Humble/Jazzy: action_tutorials_cpp uses action_tutorials_interfaces
// - Kilted/Lyrical: action_tutorials_cpp uses example_interfaces
#[cfg(not(any(feature = "kilted", feature = "lyrical")))]
use hiroz_msgs::action_tutorials_interfaces::{
    FibonacciFeedback, FibonacciGoal, FibonacciResult, action::Fibonacci,
};
#[cfg(any(feature = "kilted", feature = "lyrical"))]
use hiroz_msgs::example_interfaces::{
    FibonacciFeedback, FibonacciGoal, FibonacciResult, action::Fibonacci,
};

/// Build the Fibonacci sequence up to `order`.
fn fibonacci_sequence(order: i32) -> Vec<i32> {
    let mut seq = vec![0, 1];
    for i in 2..=order as usize {
        let next = seq[i - 1] + seq[i - 2];
        seq.push(next);
    }
    seq
}

// ---------------------------------------------------------------------------
// Test 1: server accepts, sends feedback, returns result; client asserts all
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_action_goal_accept_and_succeed() {
    zenoh::init_log_from_env_or("error");
    let router = TestRouter::new();

    let order = 5_i32;
    let expected_sequence = fibonacci_sequence(order);

    // --- Server ---
    let server_endpoint = router.endpoint().to_string();
    let server_handle = tokio::spawn(async move {
        let ctx = create_hiroz_context_with_endpoint(&server_endpoint).expect("server context");
        let node = ctx.create_node("fib_server_accept").build().expect("node");
        let _server = node
            .create_action_server::<Fibonacci>("fibonacci_accept")
            .build()
            .expect("server")
            .with_handler(|executing: ExecutingGoal<Fibonacci>| async move {
                let ord = executing.goal.order;
                let mut seq = vec![0, 1];
                for i in 2..=ord as usize {
                    let next = seq[i - 1] + seq[i - 2];
                    seq.push(next);
                    #[cfg(any(feature = "kilted", feature = "lyrical"))]
                    executing
                        .publish_feedback(FibonacciFeedback {
                            sequence: seq.clone(),
                        })
                        .unwrap();
                    #[cfg(not(any(feature = "kilted", feature = "lyrical")))]
                    executing
                        .publish_feedback(FibonacciFeedback {
                            partial_sequence: seq.clone(),
                        })
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                executing
                    .succeed(FibonacciResult { sequence: seq })
                    .unwrap();
            });

        // Keep server alive long enough for the client to complete
        tokio::time::sleep(Duration::from_secs(15)).await;
    });

    // Give the server time to start
    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Client ---
    let ctx = create_hiroz_context_with_router(&router).expect("client context");
    let node = ctx.create_node("fib_client_accept").build().expect("node");
    let client = node
        .create_action_client::<Fibonacci>("fibonacci_accept")
        .build()
        .expect("client");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut goal_handle = client
        .send_goal(FibonacciGoal { order })
        .await
        .expect("goal should be accepted");

    // Collect feedback
    let feedback_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fc = feedback_count.clone();
    if let Some(mut fb_rx) = goal_handle.feedback() {
        tokio::spawn(async move {
            while fb_rx.recv().await.is_some() {
                fc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }

    let result = tokio::time::timeout(Duration::from_secs(10), goal_handle.result())
        .await
        .expect("no timeout")
        .expect("result ok");

    assert_eq!(
        result.sequence, expected_sequence,
        "Fibonacci sequence mismatch"
    );
    assert!(
        feedback_count.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "expected at least one feedback message"
    );

    server_handle.abort();
}

// ---------------------------------------------------------------------------
// Test 2: server rejects; client receives Err (goal rejected)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_action_goal_reject() {
    zenoh::init_log_from_env_or("error");
    let router = TestRouter::new();

    // --- Server (manual mode, always rejects) ---
    let server_endpoint = router.endpoint().to_string();
    let server_handle = tokio::spawn(async move {
        let ctx = create_hiroz_context_with_endpoint(&server_endpoint).expect("server context");
        let node = ctx.create_node("fib_server_reject").build().expect("node");
        let server = node
            .create_action_server::<Fibonacci>("fibonacci_reject")
            .build()
            .expect("server");

        // Reject the first incoming goal
        match tokio::time::timeout(Duration::from_secs(10), server.recv_goal()).await {
            Ok(Ok(requested)) => {
                requested.reject().expect("reject should succeed");
            }
            Ok(Err(e)) => panic!("recv_goal error: {}", e),
            Err(_) => panic!("timeout waiting for goal"),
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Client ---
    let ctx = create_hiroz_context_with_router(&router).expect("client context");
    let node = ctx.create_node("fib_client_reject").build().expect("node");
    let client = node
        .create_action_client::<Fibonacci>("fibonacci_reject")
        .build()
        .expect("client");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let result = client.send_goal(FibonacciGoal { order: 3 }).await;
    assert!(result.is_err(), "expected goal to be rejected");

    server_handle.abort();
}

// ---------------------------------------------------------------------------
// Test 3: server accepts, client requests cancel, server handles it
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_action_goal_cancel() {
    zenoh::init_log_from_env_or("error");
    let router = TestRouter::new();

    // --- Server (long-running, cancel-aware) ---
    let server_endpoint = router.endpoint().to_string();
    let server_handle = tokio::spawn(async move {
        let ctx = create_hiroz_context_with_endpoint(&server_endpoint).expect("server context");
        let node = ctx.create_node("fib_server_cancel").build().expect("node");
        let _server = node
            .create_action_server::<Fibonacci>("fibonacci_cancel")
            .build()
            .expect("server")
            .with_handler(|executing: ExecutingGoal<Fibonacci>| async move {
                // Run a long computation that checks for cancellation
                for _ in 0..100 {
                    if executing.is_cancel_requested() {
                        executing
                            .canceled(FibonacciResult { sequence: vec![0] })
                            .unwrap();
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                executing
                    .succeed(FibonacciResult {
                        sequence: vec![0, 1],
                    })
                    .unwrap();
            });

        tokio::time::sleep(Duration::from_secs(15)).await;
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Client ---
    let ctx = create_hiroz_context_with_router(&router).expect("client context");
    let node = ctx.create_node("fib_client_cancel").build().expect("node");
    let client = node
        .create_action_client::<Fibonacci>("fibonacci_cancel")
        .build()
        .expect("client");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let goal_handle = client
        .send_goal(FibonacciGoal { order: 100 })
        .await
        .expect("goal accepted");

    // Request cancellation after a short delay
    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancel_response = goal_handle.cancel().await.expect("cancel request ok");
    // ROS 2 returns error_code 0 for accepted cancellation
    assert_eq!(
        cancel_response.return_code, 0,
        "expected cancellation to be accepted (error_code 0), got {}",
        cancel_response.return_code
    );

    server_handle.abort();
}

// ---------------------------------------------------------------------------
// Test 4: feedback arrives in send order
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_action_feedback_ordering() {
    zenoh::init_log_from_env_or("error");
    let router = TestRouter::new();

    let order = 8_i32;

    // --- Server ---
    let server_endpoint = router.endpoint().to_string();
    let server_handle = tokio::spawn(async move {
        let ctx = create_hiroz_context_with_endpoint(&server_endpoint).expect("server context");
        let node = ctx.create_node("fib_server_order").build().expect("node");
        let _server = node
            .create_action_server::<Fibonacci>("fibonacci_order")
            .build()
            .expect("server")
            .with_handler(|executing: ExecutingGoal<Fibonacci>| async move {
                let ord = executing.goal.order;
                executing
                    .wait_for_feedback_subscriber(1, Duration::from_secs(5))
                    .await;
                let mut seq = vec![0, 1];
                for i in 2..=ord as usize {
                    let next = seq[i - 1] + seq[i - 2];
                    seq.push(next);
                    // Publish feedback with the growing partial sequence
                    #[cfg(any(feature = "kilted", feature = "lyrical"))]
                    executing
                        .publish_feedback(FibonacciFeedback {
                            sequence: seq.clone(),
                        })
                        .unwrap();
                    #[cfg(not(any(feature = "kilted", feature = "lyrical")))]
                    executing
                        .publish_feedback(FibonacciFeedback {
                            partial_sequence: seq.clone(),
                        })
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
                executing
                    .succeed(FibonacciResult { sequence: seq })
                    .unwrap();
            });

        tokio::time::sleep(Duration::from_secs(15)).await;
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Client ---
    let ctx = create_hiroz_context_with_router(&router).expect("client context");
    let node = ctx.create_node("fib_client_order").build().expect("node");
    let client = node
        .create_action_client::<Fibonacci>("fibonacci_order")
        .build()
        .expect("client");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut goal_handle = client
        .send_goal(FibonacciGoal { order })
        .await
        .expect("goal accepted");

    // Collect all feedback
    let received_feedback: Arc<std::sync::Mutex<Vec<Vec<i32>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let rf = received_feedback.clone();

    if let Some(mut fb_rx) = goal_handle.feedback() {
        tokio::spawn(async move {
            while let Some(fb) = fb_rx.recv().await {
                #[cfg(any(feature = "kilted", feature = "lyrical"))]
                let seq = fb.sequence.clone();
                #[cfg(not(any(feature = "kilted", feature = "lyrical")))]
                let seq = fb.partial_sequence.clone();
                rf.lock().unwrap().push(seq);
            }
        });
    }

    // Wait for the result
    tokio::time::timeout(Duration::from_secs(10), goal_handle.result())
        .await
        .expect("no timeout")
        .expect("result ok");

    // Give feedback task time to drain remaining messages
    tokio::time::sleep(Duration::from_millis(200)).await;

    let feedbacks = received_feedback.lock().unwrap();
    assert!(
        !feedbacks.is_empty(),
        "expected at least one feedback message"
    );

    // Assert that feedback sequences are monotonically growing in length
    for window in feedbacks.windows(2) {
        assert!(
            window[1].len() >= window[0].len(),
            "feedback not in order: {:?} followed by {:?}",
            window[0],
            window[1]
        );
    }

    // Assert that last feedback ends with the correct value for the sequence
    let last = feedbacks.last().unwrap();
    let expected_last = fibonacci_sequence(order);
    assert_eq!(
        last,
        &expected_last[..last.len()],
        "last feedback slice mismatch"
    );

    server_handle.abort();
}
