#![cfg(feature = "ros-interop")]

mod common;

use std::thread;

use crate::common::*;

#[test]
fn test_protobuf_pubsub_demo() {
    let router = TestRouter::new();

    println!("\n=== Test: protobuf pub/sub demo ===");

    // Run the pub/sub demo using the example code
    let handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_router(&router).expect("Failed to create hiroz context");

        // Use the actual pubsub demo code with max 3 messages
        protobuf_demo::run_pubsub_demo(ctx, Some(3)).expect("Pubsub demo failed");
    });

    handle.join().expect("Pubsub demo thread panicked");

    println!("Test passed: protobuf pub/sub demo completed successfully");
}
