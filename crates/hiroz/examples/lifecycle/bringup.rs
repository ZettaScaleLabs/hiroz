//! Lifecycle bringup orchestrator example.
//!
//! Demonstrates using `ZLifecycleClient` to drive a remote lifecycle node
//! through its full state machine — the pattern used by system bringup
//! managers to coordinate startup and shutdown of multiple nodes.
//!
//! This example spawns a lifecycle node in a background thread, then uses
//! a `ZLifecycleClient` on the main thread to configure, activate,
//! deactivate, and shut it down.
//!
//! Run with:
//! ```bash
//! cargo run --example lifecycle_bringup
//! ```

// ANCHOR: full_example
use std::time::Duration;

use hiroz::{
    Builder, Result,
    context::ZContextBuilder,
    lifecycle::{CallbackReturn, ZLifecycleClient},
};

fn main() -> Result<()> {
    // Shared Zenoh context — both the lifecycle node and the client connect
    // through the same context (in production they'd typically be separate
    // processes connected via a Zenoh router).
    let ctx = ZContextBuilder::default().build()?;

    // --- Lifecycle node (simulates a remote node) ---
    let mut node = ctx.create_lifecycle_node("camera_driver").build()?;

    node.on_configure = Box::new(|_| {
        println!("[camera_driver] on_configure: opening device");
        CallbackReturn::Success
    });
    node.on_activate = Box::new(|_| {
        println!("[camera_driver] on_activate: streaming");
        CallbackReturn::Success
    });
    node.on_deactivate = Box::new(|_| {
        println!("[camera_driver] on_deactivate: paused");
        CallbackReturn::Success
    });
    node.on_shutdown = Box::new(|_| {
        println!("[camera_driver] on_shutdown: releasing device");
        CallbackReturn::Success
    });

    // Give services a moment to register with Zenoh
    std::thread::sleep(Duration::from_millis(200));

    // --- Bringup manager ---
    let mgr = ctx.create_node("bringup_manager").build()?;
    let client = ZLifecycleClient::new(&mgr, "camera_driver")?;

    // Allow time for service discovery
    std::thread::sleep(Duration::from_millis(300));

    let timeout = Duration::from_secs(5);
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        // Query the initial state
        let state = client.get_state(timeout).await?;
        println!("[bringup] camera_driver is {:?}", state);

        // Drive the node through its lifecycle
        println!("[bringup] configuring...");
        assert!(client.configure(timeout).await?);

        println!("[bringup] activating...");
        assert!(client.activate(timeout).await?);

        let state = client.get_state(timeout).await?;
        println!("[bringup] camera_driver is now {:?}", state);

        // Simulate some work
        println!("[bringup] node is running... (would do work here)");

        // Graceful shutdown
        println!("[bringup] deactivating...");
        assert!(client.deactivate(timeout).await?);

        println!("[bringup] shutting down...");
        assert!(client.shutdown(timeout).await?);

        let state = client.get_state(timeout).await?;
        println!("[bringup] camera_driver is now {:?}", state);

        Ok(())
    })
}
// ANCHOR_END: full_example
