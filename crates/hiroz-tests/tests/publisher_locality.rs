//! `ZPubBuilder::with_locality`: does the restriction reach the wire?
//!
//! `Locality::SessionLocal` skips the transport, so the samples never leave the
//! session. Subscribers in the same session still receive them.
//!
//! "The other context received nothing" proves nothing alone: a broken router,
//! a wrong topic or a short deadline give the same zero. Each scenario therefore
//! publishes on two topics from the same context.
//!
//! | topic | publisher locality | same context | other context |
//! |---|---|---|---|
//! | `local_only` | `SessionLocal` | must receive | must receive nothing |
//! | `control`    | default (`Any`) | must receive | must receive |
//!
//! The control row proves the router routes, the topics match and the deadline
//! suffices. Without it a zero is unknown, not a pass.

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use common::*;
use hiroz::{Builder, Locality};
use hiroz_msgs::std_msgs::String as RosString;

const MESSAGES: usize = 5;
/// Generous relative to loopback delivery. It bounds the "received nothing"
/// assertions, so it has to be long enough that a slow-but-working path is not
/// mistaken for a blocked one.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);

/// A string subscriber, spelled once so the signature below stays readable.
///
/// `ZSub` carries a serializer parameter, so writing the full type at the
/// return position is what trips `clippy::type_complexity`.
type StringSub = hiroz::pubsub::ZSub<RosString, (), hiroz::msg::NativeCdrSerdes<RosString>>;

fn counting_sub(
    node: &hiroz::node::ZNode,
    topic: &str,
) -> hiroz::Result<(StringSub, Arc<AtomicUsize>)> {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let sub = node
        .create_sub::<RosString>(topic)
        .build_with_callback(move |_msg: RosString| {
            c.fetch_add(1, Ordering::SeqCst);
        })?;
    Ok((sub, count))
}

/// Poll until `f` holds or the deadline passes. Returns whether it held.
fn wait_until(f: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < DELIVERY_DEADLINE {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    f()
}

#[test]
fn session_local_publisher_is_invisible_to_another_context() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx_publisher = create_hiroz_context_with_router(&router)?;
    let ctx_other = create_hiroz_context_with_router(&router)?;

    let node_pub = ctx_publisher.create_node("locality_pub").build()?;
    let node_same = ctx_publisher.create_node("locality_same").build()?;
    let node_other = ctx_other.create_node("locality_other").build()?;

    let (_s1, same_local) = counting_sub(&node_same, "local_only")?;
    let (_s2, other_local) = counting_sub(&node_other, "local_only")?;
    let (_s3, same_control) = counting_sub(&node_same, "control")?;
    let (_s4, other_control) = counting_sub(&node_other, "control")?;

    let pub_local = node_pub
        .create_pub::<RosString>("local_only")
        .with_locality(Locality::SessionLocal)
        .build()?;
    let pub_control = node_pub.create_pub::<RosString>("control").build()?;

    // Let the router propagate the four subscriber declarations before the first
    // put, or the control row can lose messages for a reason unrelated to
    // locality and take the test red for the wrong cause.
    wait_for_ready(Duration::from_millis(500));

    for i in 0..MESSAGES {
        let msg = RosString {
            data: format!("msg-{i}"),
        };
        pub_local.publish(&msg)?;
        pub_control.publish(&msg)?;
    }

    // The control must arrive in BOTH contexts. Assert it first: if it fails,
    // the environment is broken and the SessionLocal assertions below would be
    // meaningless rather than informative.
    assert!(
        wait_until(|| same_control.load(Ordering::SeqCst) >= MESSAGES),
        "control publisher did not reach the same context: {} of {MESSAGES}",
        same_control.load(Ordering::SeqCst)
    );
    assert!(
        wait_until(|| other_control.load(Ordering::SeqCst) >= MESSAGES),
        "control publisher did not reach the other context: {} of {MESSAGES}. \
         The router is not routing, so this run proves nothing about locality.",
        other_control.load(Ordering::SeqCst)
    );

    // Same session: SessionLocal still delivers.
    assert!(
        wait_until(|| same_local.load(Ordering::SeqCst) >= MESSAGES),
        "SessionLocal publisher did not reach a subscriber in its own session: {} of {MESSAGES}",
        same_local.load(Ordering::SeqCst)
    );

    // Other session: it must have delivered nothing. The control above already
    // arrived over the same router within the same deadline, so a zero here is
    // the restriction working, not a slow path.
    assert_eq!(
        other_local.load(Ordering::SeqCst),
        0,
        "SessionLocal publisher leaked to another context — the restriction is not applied"
    );

    Ok(())
}
