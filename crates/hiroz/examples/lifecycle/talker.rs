//! Lifecycle-managed talker node.
//!
//! Demonstrates the ROS 2 lifecycle state machine:
//! - Registers `on_configure` / `on_activate` / `on_deactivate` callbacks
//! - Creates a lifecycle-gated publisher for `std_msgs/msg/String`
//! - Drives the node through its full lifecycle programmatically
//!
//! Run with:
//! ```bash
//! cargo run --example lifecycle_talker
//! ```

// ANCHOR: full_example
use hiroz::{Builder, Result, context::ZContextBuilder, lifecycle::CallbackReturn};
use hiroz_msgs::std_msgs::String as RosString;

fn main() -> Result<()> {
    // Build a Zenoh context and create a lifecycle node.
    // The node starts in the Unconfigured state.
    let ctx = ZContextBuilder::default().build()?;
    let mut node = ctx.create_lifecycle_node("lifecycle_talker").build()?;

    // Register callbacks for each lifecycle transition.
    // Each callback receives the previous state and must return
    // CallbackReturn::Success, ::Failure, or ::Error.
    node.on_configure = Box::new(|_prev| {
        // Load parameters, open files, connect to hardware here.
        println!("[configure] loading parameters");
        CallbackReturn::Success
    });

    node.on_activate = Box::new(|_prev| {
        // Start timers, enable hardware outputs here.
        println!("[activate] publisher enabled");
        CallbackReturn::Success
    });

    node.on_deactivate = Box::new(|_prev| {
        // Pause timers, disable hardware outputs here.
        println!("[deactivate] publisher paused");
        CallbackReturn::Success
    });

    node.on_cleanup = Box::new(|_prev| {
        // Release resources acquired in on_configure here.
        println!("[cleanup] releasing resources");
        CallbackReturn::Success
    });

    // Create a lifecycle-gated publisher.
    // It is registered as a managed entity: activate()/deactivate() on the
    // node will automatically gate this publisher. While deactivated,
    // publish() returns Ok(()) but silently drops the message.
    let pub_ = node.create_publisher::<RosString>("chatter")?;

    // configure(): Unconfigured → Inactive
    // on_configure callback fires; publisher remains deactivated.
    node.configure()?;

    // This publish is silently dropped — the node is Inactive.
    pub_.publish(&RosString {
        data: "dropped (inactive)".to_string(),
    })?;

    // activate(): Inactive → Active
    // on_activate callback fires; publisher is now live.
    node.activate()?;

    // Messages are delivered while the node is Active.
    for i in 0..5 {
        let msg = RosString {
            data: format!("hello {i}"),
        };
        pub_.publish(&msg)?;
        println!("published: {}", msg.data);
    }

    // deactivate(): Active → Inactive — publisher is gated again.
    node.deactivate()?;
    // cleanup(): Inactive → Unconfigured — release resources.
    node.cleanup()?;
    // shutdown(): Unconfigured → Finalized — terminal state.
    node.shutdown()?;

    println!("node finalized");
    Ok(())
}
// ANCHOR_END: full_example
