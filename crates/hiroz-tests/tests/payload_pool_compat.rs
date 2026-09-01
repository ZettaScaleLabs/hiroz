//! Does `PayloadPool` compose with the rest of hiroz, or only with the bus?
//!
//! The pool's own unit tests establish that its arithmetic is right. They say
//! nothing about whether a pooled message survives contact with a publisher, a
//! transport, a QoS profile or a second subscriber — and those are the
//! questions a user adopting it will actually hit.
//!
//! The property every test here turns on is the same one: **a slot returns to
//! the pool when the last holder drops its `Arc`**. So `pool.stats().available`
//! back at capacity after a publish is the evidence that nothing downstream
//! retained the buffer. If the wire path ever spliced the payload instead of
//! serializing a copy of it, zenoh's TX queue would hold the pooled allocation
//! and these tests would fail — which is exactly what makes them worth running.
//!
//! | test | property |
//! |---|---|
//! | `a_wire_publish_does_not_retain_the_pooled_buffer` | the transport copies; the slot returns |
//! | `a_transient_local_wire_publisher_does_not_retain_it_either` | the durability cache holds serialized samples, not the `Arc` |
//! | `two_subscribers_share_one_allocation_and_both_release_it` | fan-out costs one slot, not N |
//! | `a_remote_locality_publisher_returns_the_slot_after_both_routes` | bus + wire together still release |
//! | `a_pool_survives_a_subscriber_that_publishes_from_its_own_pool` | inline delivery does not deadlock the pool
//!
//! # These do not pass vacuously
//!
//! All five conclude "nothing downstream retained the buffer" from `available`
//! being back at capacity, so if `available` could not drop in this setting they
//! would all be green for the wrong reason. Measured: retaining the published
//! `Arc` in `a_wire_publish_does_not_retain_the_pooled_buffer` turns **that test
//! and only that test** red, with zero compile errors and the patch confirmed
//! applied. The instrument works.

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use common::*;
use hiroz::{
    Builder,
    local_bus::{Delivery, Published},
    payload_pool::PayloadPool,
    qos::{QosDurability, QosProfile},
};
use hiroz_msgs::std_msgs::String as RosString;

const DEADLINE: Duration = Duration::from_secs(5);

fn wait_until(f: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

fn msg(s: &str) -> RosString {
    RosString { data: s.to_owned() }
}

/// The wire path must not hold the pooled allocation after `publish` returns.
///
/// hiroz serializes into a fresh `ZBuf` (`ZBufWriter` + serde), so the bytes
/// zenoh queues are a copy and the pooled buffer is untouched. This asserts
/// that rather than trusting the reading: if serialization ever became a splice
/// of the existing `ZSlice`, the slot would still be out and `available` would
/// be short.
#[test]
fn a_wire_publish_does_not_retain_the_pooled_buffer() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_wire").build()?;

    // A plain publisher: no locality restriction, so this goes on the wire.
    let publisher = node.create_pub::<RosString>("pool_wire").build()?;

    let mut pool = PayloadPool::new(2, || msg("init"));
    assert_eq!(pool.stats().available, 2);

    for i in 0..8 {
        let mut slot = pool.acquire().expect("a slot, every iteration");
        slot.data = format!("m{i}");
        publisher.publish_shared(slot.into_shared())?;

        assert_eq!(
            pool.stats().available,
            2,
            "iteration {i}: the transport is still holding the pooled buffer after publish \
             returned, so the pool has lost a slot to the wire"
        );
    }
    assert_eq!(pool.stats().exhaustions, 0, "the pool ran dry on the wire");
    Ok(())
}

/// TRANSIENT_LOCAL keeps a history, and the question is *of what*.
///
/// The cache holds serialized samples, not the `Arc<T>`, so a durable publisher
/// costs the pool nothing. Were it to retain the message itself, every send
/// would permanently consume a slot and this pool of two would exhaust on the
/// third iteration.
#[test]
fn a_transient_local_wire_publisher_does_not_retain_it_either() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_tl").build()?;

    let publisher = node
        .create_pub::<RosString>("pool_tl")
        .with_qos(QosProfile {
            durability: QosDurability::TransientLocal,
            ..Default::default()
        })
        .build()?;

    let mut pool = PayloadPool::new(2, || msg("init"));
    for i in 0..8 {
        let mut slot = pool.acquire().expect("a slot, every iteration");
        slot.data = format!("m{i}");
        publisher.publish_shared(slot.into_shared())?;
        assert_eq!(
            pool.stats().available,
            2,
            "iteration {i}: the durability cache retained the pooled message itself"
        );
    }
    Ok(())
}

/// Fan-out costs one slot, not one per subscriber.
///
/// Every subscriber is handed a clone of the same `Arc`, so the slot returns
/// once the last callback returns. A pool of one therefore serves two
/// subscribers indefinitely — which is the whole point of sharing rather than
/// copying, and would be false if delivery cloned the payload per receiver.
#[test]
fn two_subscribers_share_one_allocation_and_both_release_it() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_fanout").build()?;

    let hits_a = Arc::new(AtomicUsize::new(0));
    let hits_b = Arc::new(AtomicUsize::new(0));
    let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));

    let a = hits_a.clone();
    let ptrs = seen.clone();
    let _sub_a = node
        .create_sub::<RosString>("pool_fanout")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            ptrs.lock().expect("lock").push(Arc::as_ptr(&m) as usize);
            a.fetch_add(1, Ordering::SeqCst);
        })?;

    let b = hits_b.clone();
    let _sub_b = node
        .create_sub::<RosString>("pool_fanout")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            b.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("pool_fanout")
        .with_intra_process_only()
        .build()?;

    // Capacity one: if fan-out cost a slot per subscriber this exhausts at once.
    let mut pool = PayloadPool::new(1, || msg("init"));
    for i in 0..5 {
        let mut slot = pool.acquire().unwrap_or_else(|| {
            panic!("iteration {i}: the pool exhausted, so fan-out is retaining slots")
        });
        slot.data = format!("m{i}");
        let delivered = publisher.publish_shared(slot.into_shared())?;
        assert_eq!(
            delivered,
            Published::Bus(Delivery::Sent(2)),
            "iteration {i}: both subscribers must be served"
        );
    }

    assert!(wait_until(|| hits_a.load(Ordering::SeqCst) == 5));
    assert_eq!(hits_b.load(Ordering::SeqCst), 5);
    assert_eq!(pool.stats().exhaustions, 0);
    assert_eq!(pool.stats().stuck, 0);

    // One slot, so every delivery must have carried the same allocation.
    let p = seen.lock().expect("lock");
    assert!(
        p.iter().all(|&x| x == p[0]),
        "the pool handed out more than one buffer from a capacity of one: {p:?}"
    );
    Ok(())
}

/// `Locality::Remote` runs the bus *and* the wire for one message. Both must
/// release, or the slot leaks on every send down the doubled path.
#[test]
fn a_remote_locality_publisher_returns_the_slot_after_both_routes() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_remote").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<RosString>("pool_remote")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("pool_remote")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;

    let mut pool = PayloadPool::new(2, || msg("init"));
    for i in 0..6 {
        let mut slot = pool.acquire().expect("a slot, every iteration");
        slot.data = format!("m{i}");
        publisher.publish_shared(slot.into_shared())?;
        assert_eq!(
            pool.stats().available,
            2,
            "iteration {i}: one of the two routes kept the pooled buffer"
        );
    }
    assert!(wait_until(|| hits.load(Ordering::SeqCst) == 6));
    Ok(())
}

/// Bus delivery is synchronous and inline on the publishing thread, which is
/// the property that makes a *blocking* acquire unsafe. It must not also make
/// a non-blocking one unsafe.
///
/// `Pooled::into_shared` consumes the guard, so the pool's borrow ends before
/// `publish_shared` is called and a callback reached mid-publish can take the
/// pool again. This pins that: the callback republishes from the *same* pool,
/// on the publishing thread, and neither deadlocks nor panics.
#[test]
fn a_pool_survives_a_subscriber_that_publishes_from_its_own_pool() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_reentrant").build()?;

    let echoes = Arc::new(AtomicUsize::new(0));
    let inner_pool = Arc::new(Mutex::new(PayloadPool::new(4, || msg("inner"))));
    let out = node
        .create_pub::<RosString>("pool_reentrant_out")
        .with_intra_process_only()
        .build()?;

    let e = echoes.clone();
    let p = inner_pool.clone();
    let _sub = node
        .create_sub::<RosString>("pool_reentrant_in")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            // Re-entering the pool from inside a callback, on the publishing
            // thread. If `into_shared` did not end the borrow, this is where
            // the design would deadlock.
            let mut guard = p.lock().expect("pool lock");
            if let Some(slot) = guard.acquire() {
                let _ = out.publish_shared(slot.into_shared());
                e.fetch_add(1, Ordering::SeqCst);
            }
        })?;

    let publisher = node
        .create_pub::<RosString>("pool_reentrant_in")
        .with_intra_process_only()
        .build()?;

    let mut outer = PayloadPool::new(2, || msg("outer"));
    for i in 0..5 {
        let mut slot = outer.acquire().expect("outer slot");
        slot.data = format!("m{i}");
        publisher.publish_shared(slot.into_shared())?;
    }

    assert_eq!(
        echoes.load(Ordering::SeqCst),
        5,
        "the callback did not complete five re-entrant publishes"
    );
    assert_eq!(outer.stats().exhaustions, 0);
    Ok(())
}
