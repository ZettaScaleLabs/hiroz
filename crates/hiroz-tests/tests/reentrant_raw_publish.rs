//! Re-entrancy coverage for the raw (FFI) publish path.
//!
//! `CallbackDispatcher::local_only_shim` defers a sample to the drain thread
//! only while `LOCAL_PUBLISH_DEPTH` is set, and that flag is set by
//! `LocalPublishGuard`. The four `ZPub` publish methods each take the guard.
//! `RawPublisher::publish_bytes` — the path `rmw-zenoh-rs` publishes through —
//! did not, so a same-process raw publish was never marked local: the shim saw
//! depth 0, delivered inline, and the subscriber's callback ran on the
//! publishing thread. A raw callback that publishes back into its own topic
//! then recurses until the stack is gone, which is exactly the defect the
//! dispatcher exists to prevent — still reachable, just through the FFI door.
//!
//! The detector asserts the *thread*, not the absence of a crash. Asserting
//! "no stack overflow" would need an unbounded feedback loop, which aborts the
//! process on failure and tells you nothing about why; thread identity is
//! exact, deterministic, and cheap. Without the guard the callback thread and
//! the publishing thread are the same and the assertion fails.

mod common;

use std::{sync::mpsc, thread, time::Duration};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::Builder;
use serial_test::serial;

/// A 4-byte CDR encapsulation header followed by an arbitrary body. The raw
/// path never decodes this — it hands the bytes straight to the callback — so
/// the contents only need to be well-formed enough to travel.
const RAW_SAMPLE: &[u8] = &[0x00, 0x01, 0x00, 0x00, 0xde, 0xad, 0xbe, 0xef];

#[test]
#[serial]
fn raw_publish_does_not_deliver_on_the_publishing_thread() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("context");
    let node = ctx.create_node("raw_reentrancy").build().expect("node");

    let (tx, rx) = mpsc::channel();
    let _sub = node
        .create_raw_subscriber("/raw_reentrant", "std_msgs/msg/String", "", move |_bytes| {
            // Report which thread the callback body actually runs on.
            let _ = tx.send(format!("{:?}", thread::current().id()));
        })
        .expect("raw subscriber");

    let publisher = node
        .create_raw_publisher("/raw_reentrant", "std_msgs/msg/String", "")
        .expect("raw publisher");

    // Let the local subscriber be discovered before publishing.
    thread::sleep(Duration::from_millis(800));

    let publishing_thread = format!("{:?}", thread::current().id());
    publisher.publish_bytes(RAW_SAMPLE).expect("raw publish");

    let callback_thread = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the raw callback never ran — the sample was not delivered at all");

    assert_ne!(
        callback_thread, publishing_thread,
        "raw subscriber callback ran inline on the publishing thread \
         ({publishing_thread}); RawPublisher::publish_bytes is not entering \
         LocalPublishGuard, so a callback that publishes back into its own \
         topic will recurse until the stack is exhausted"
    );
}
