//! `PayloadPool` with publishers, QoS and subscribers: a slot must return to
//! the pool once the last holder drops its `Arc`. The table in the
//! `payload_pool` module documentation lists the cases.

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
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

/// A wire publisher, volatile or `TRANSIENT_LOCAL`, does not hold the pooled
/// `Arc<T>` after `publish_shared` returns. Serialization writes a new `ZBuf`,
/// and the durability cache holds serialized samples. This checks the outer
/// `Arc<T>` only, not the buffers inside a message.
#[test]
fn a_wire_publish_does_not_retain_the_pooled_buffer() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_wire").build()?;

    for (topic, durability) in [
        ("pool_wire_volatile", QosDurability::Volatile),
        ("pool_wire_tl", QosDurability::TransientLocal),
    ] {
        // No locality restriction, so this goes on the wire.
        let publisher = node
            .create_pub::<RosString>(topic)
            .with_qos(QosProfile {
                durability,
                ..Default::default()
            })
            .build()?;

        let mut pool = PayloadPool::new(2, || msg("init"));
        for i in 0..8 {
            let mut slot = pool.acquire().expect("a slot, every iteration");
            slot.data = format!("m{i}");
            assert_eq!(
                publisher.publish_shared(slot.into_shared())?,
                Published::Wire,
                "{topic} iteration {i}: the wire route did not run"
            );
            assert_eq!(
                pool.stats().available,
                2,
                "{topic} iteration {i}: the publisher still holds the pooled buffer"
            );
        }
        assert_eq!(pool.stats().exhaustions, 0, "{topic}: the pool ran dry");

        // Positive control: a retained `Arc` does lower `available`.
        let retained = pool.acquire().expect("a slot").into_shared();
        publisher.publish_shared(Arc::clone(&retained))?;
        assert_eq!(
            pool.stats().available,
            1,
            "{topic}: `available` did not see a retained Arc"
        );
    }
    Ok(())
}

/// Fan-out costs one slot, not one per subscriber: every subscriber gets a
/// clone of the same `Arc`, so a pool of one serves two subscribers.
#[test]
fn two_subscribers_share_one_allocation_and_both_release_it() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("pool_fanout").build()?;

    let hits_a = Arc::new(AtomicUsize::new(0));
    let hits_b = Arc::new(AtomicUsize::new(0));
    let seen_a: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_b: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let published: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));

    let a = hits_a.clone();
    let ptrs = seen_a.clone();
    let _sub_a = node
        .create_sub::<RosString>("pool_fanout")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            ptrs.lock().expect("lock").push(Arc::as_ptr(&m) as usize);
            a.fetch_add(1, Ordering::SeqCst);
        })?;

    let b = hits_b.clone();
    let ptrs = seen_b.clone();
    let _sub_b = node
        .create_sub::<RosString>("pool_fanout")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            ptrs.lock().expect("lock").push(Arc::as_ptr(&m) as usize);
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
        let shared = slot.into_shared();
        published
            .lock()
            .expect("lock")
            .push(Arc::as_ptr(&shared) as usize);
        let delivered = publisher.publish_shared(shared)?;
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
    assert_eq!(
        pool.stats().available,
        1,
        "a subscriber kept the last message"
    );

    // Both subscribers must have seen the allocation the publisher sent.
    let sent = published.lock().expect("lock");
    assert_eq!(
        *seen_a.lock().expect("lock"),
        *sent,
        "subscriber A got a copy"
    );
    assert_eq!(
        *seen_b.lock().expect("lock"),
        *sent,
        "subscriber B got a copy"
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
        assert_eq!(
            publisher.publish_shared(slot.into_shared())?,
            Published::BusAndWire(Delivery::Sent(1)),
            "iteration {i}: both routes must run"
        );
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
/// pool again. Here the callback republishes from the same pool, on the
/// publishing thread.
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

    let delivered_out = Arc::new(AtomicUsize::new(0));
    let d = delivered_out.clone();
    let _out_sub = node
        .create_sub::<RosString>("pool_reentrant_out")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            d.fetch_add(1, Ordering::SeqCst);
        })?;

    let e = echoes.clone();
    let p = inner_pool.clone();
    let _sub = node
        .create_sub::<RosString>("pool_reentrant_in")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            // Take the pool again from inside a callback, on the publishing
            // thread. Release the mutex before publishing: delivery is inline,
            // so holding it across the publish would deadlock.
            let shared = {
                let mut guard = p.lock().expect("pool lock");
                guard.acquire().map(|slot| slot.into_shared())
            };
            if let Some(shared) = shared
                && matches!(
                    out.publish_shared(shared),
                    Ok(Published::Bus(Delivery::Sent(1)))
                )
            {
                e.fetch_add(1, Ordering::SeqCst);
            }
        })?;

    let publisher = node
        .create_pub::<RosString>("pool_reentrant_in")
        .with_intra_process_only()
        .build()?;

    // The same pool the callback re-enters: that is the property under test.
    // The loop runs on its own thread so a deadlock fails the join below
    // instead of hanging the test.
    let outer_pool = inner_pool.clone();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let run = || -> hiroz::Result<()> {
            for i in 0..5 {
                let shared = {
                    let mut guard = outer_pool.lock().expect("pool lock");
                    let mut slot = guard.acquire().expect("outer slot");
                    slot.data = format!("m{i}");
                    slot.into_shared()
                };
                publisher.publish_shared(shared)?;
            }
            Ok(())
        };
        let _ = done_tx.send(run());
    });
    done_rx
        .recv_timeout(DEADLINE)
        .expect("the publishing thread did not finish: re-entering the pool deadlocked")?;

    assert_eq!(
        echoes.load(Ordering::SeqCst),
        5,
        "the callback did not complete five re-entrant publishes"
    );
    assert_eq!(
        delivered_out.load(Ordering::SeqCst),
        5,
        "the nested messages did not reach their subscriber"
    );
    // The pool both sides share: re-entering it must not have exhausted it.
    // Each outer publish takes a slot and so does the callback it invokes; four
    // slots serve both because each returns before the next send.
    assert_eq!(
        inner_pool.lock().expect("pool lock").stats().exhaustions,
        0,
        "the re-entrant acquire exhausted the pool"
    );
    Ok(())
}
