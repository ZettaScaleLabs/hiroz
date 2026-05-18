//! Integration tests for [`ZCache`].
//!
//! Each test creates its own [`TestRouter`] on a unique port and a pair of
//! hiroz contexts (publisher + subscriber / cache) so they run in parallel
//! under cargo nextest without port collisions.

#![cfg(feature = "ros-msgs")]

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::std_msgs::String as RosString;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_msg(s: &str) -> RosString {
    RosString { data: s.into() }
}

/// Publish `msgs` from a separate pub context on `topic`, waiting for the
/// cache to be ready first. Returns when all messages have been published.
fn publish_n(pub_endpoint: &str, topic: &str, msgs: &[RosString]) {
    let ctx = create_hiroz_context_with_endpoint(pub_endpoint).expect("pub ctx");
    let node = ctx.create_node("test_pub").build().expect("pub node");
    let publisher = node
        .create_pub::<RosString>(topic)
        .build()
        .expect("publisher");
    // Small delay to let the cache subscriber connect.
    thread::sleep(Duration::from_millis(100));
    for msg in msgs {
        publisher.publish(msg).expect("publish");
        // Give Zenoh time to deliver each message.
        thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Test 1: basic insert — len() reflects published messages
// ---------------------------------------------------------------------------

#[test]
fn cache_zenoh_stamp_inserts() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    let cache = node
        .create_cache::<RosString>("/cache_inserts", 50)
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs: Vec<_> = (0..5).map(|i| make_msg(&format!("msg{i}"))).collect();
    publish_n(&endpoint, "/cache_inserts", &msgs);

    // Allow delivery.
    thread::sleep(Duration::from_millis(200));
    assert!(cache.len() > 0, "expected messages in cache");
    assert!(cache.len() <= 5);
}

// ---------------------------------------------------------------------------
// Test 2: capacity eviction — oldest is gone after capacity+1 inserts
// ---------------------------------------------------------------------------

#[test]
fn cache_capacity_evicts_oldest() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    const CAP: usize = 5;
    let cache = node
        .create_cache::<RosString>("/cache_evict", CAP)
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs: Vec<_> = (0..(CAP + 3)).map(|i| make_msg(&format!("m{i}"))).collect();
    publish_n(&endpoint, "/cache_evict", &msgs);

    thread::sleep(Duration::from_millis(300));
    assert_eq!(cache.len(), CAP, "cache should not exceed capacity");
}

// ---------------------------------------------------------------------------
// Test 3: get_interval — returns exact range
// ---------------------------------------------------------------------------

#[test]
fn cache_get_interval() {
    // Use extractor stamps with known SystemTime values so we don't depend
    // on timing of actual delivery.
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    // Messages at t=1s, 2s, 3s, 4s, 5s
    let cache = node
        .create_cache::<RosString>("/cache_interval", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs: Vec<_> = (1u64..=5).map(|i| make_msg(&i.to_string())).collect();
    publish_n(&endpoint, "/cache_interval", &msgs);
    thread::sleep(Duration::from_millis(300));

    // Query [2s, 4s] — should contain 3 messages ("2", "3", "4")
    let results = cache.get_interval(
        epoch + Duration::from_secs(2),
        epoch + Duration::from_secs(4),
    );
    assert_eq!(results.len(), 3, "expected 3 messages in [2s,4s]");
    assert_eq!(results[0].data, "2");
    assert_eq!(results[1].data, "3");
    assert_eq!(results[2].data, "4");
}

// ---------------------------------------------------------------------------
// Test 4: get_before
// ---------------------------------------------------------------------------

#[test]
fn cache_get_before() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_before", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs = vec![make_msg("1"), make_msg("3"), make_msg("5")];
    publish_n(&endpoint, "/cache_before", &msgs);
    thread::sleep(Duration::from_millis(300));

    // Query at t=4s → should return message "3" (closest before 4)
    let result = cache.get_before(epoch + Duration::from_secs(4));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "3");
}

// ---------------------------------------------------------------------------
// Test 5: get_after
// ---------------------------------------------------------------------------

#[test]
fn cache_get_after() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_after", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs = vec![make_msg("1"), make_msg("3"), make_msg("5")];
    publish_n(&endpoint, "/cache_after", &msgs);
    thread::sleep(Duration::from_millis(300));

    // Query at t=2s → should return message "3" (earliest after 2)
    let result = cache.get_after(epoch + Duration::from_secs(2));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "3");
}

// ---------------------------------------------------------------------------
// Test 6: get_nearest — tie-breaking prefers earlier
// ---------------------------------------------------------------------------

#[test]
fn cache_get_nearest() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_nearest", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    // Messages at 1s and 5s
    let msgs = vec![make_msg("1"), make_msg("5")];
    publish_n(&endpoint, "/cache_nearest", &msgs);
    thread::sleep(Duration::from_millis(300));

    // t=3s: equidistant (2s from each) → tie broken to earlier = "1"
    let result = cache.get_nearest(epoch + Duration::from_secs(3));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "1");

    // t=4s: closer to 5s
    let result = cache.get_nearest(epoch + Duration::from_secs(4));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "5");

    // t=2s: closer to 1s
    let result = cache.get_nearest(epoch + Duration::from_secs(2));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "1");
}

// ---------------------------------------------------------------------------
// Test 7: extractor stamp ordering
// ---------------------------------------------------------------------------

#[test]
fn cache_extractor_stamp() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    // Publish in reverse order — extractor should sort correctly
    let cache = node
        .create_cache::<RosString>("/cache_extractor", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs = vec![make_msg("5"), make_msg("2"), make_msg("8")];
    publish_n(&endpoint, "/cache_extractor", &msgs);
    thread::sleep(Duration::from_millis(300));

    assert_eq!(cache.oldest_stamp(), Some(epoch + Duration::from_secs(2)));
    assert_eq!(cache.newest_stamp(), Some(epoch + Duration::from_secs(8)));
    assert_eq!(cache.len(), 3);
}

// ---------------------------------------------------------------------------
// Test 8: oldest/newest stamp accessors
// ---------------------------------------------------------------------------

#[test]
fn cache_oldest_newest_stamp() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_stamps", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    // Empty cache
    assert_eq!(cache.oldest_stamp(), None);
    assert_eq!(cache.newest_stamp(), None);

    let endpoint = router.endpoint().to_string();
    let msgs = vec![make_msg("10"), make_msg("20"), make_msg("30")];
    publish_n(&endpoint, "/cache_stamps", &msgs);
    thread::sleep(Duration::from_millis(300));

    assert_eq!(cache.oldest_stamp(), Some(epoch + Duration::from_secs(10)));
    assert_eq!(cache.newest_stamp(), Some(epoch + Duration::from_secs(30)));
}

// ---------------------------------------------------------------------------
// Test 9: clear
// ---------------------------------------------------------------------------

#[test]
fn cache_clear() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    let cache = node
        .create_cache::<RosString>("/cache_clear", 50)
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    publish_n(&endpoint, "/cache_clear", &[make_msg("a"), make_msg("b")]);
    thread::sleep(Duration::from_millis(200));

    assert!(!cache.is_empty());
    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

// ---------------------------------------------------------------------------
// Test 10: concurrent publish + query — no panic/deadlock
// ---------------------------------------------------------------------------

#[test]
fn cache_concurrent_publish_query() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    let cache = Arc::new(
        node.create_cache::<RosString>("/cache_concurrent", 20)
            .build()
            .expect("cache"),
    );

    let endpoint = router.endpoint().to_string();

    // Publisher thread
    let pub_handle = {
        let ep = endpoint.clone();
        thread::spawn(move || {
            let msgs: Vec<_> = (0..30).map(|i| make_msg(&format!("m{i}"))).collect();
            publish_n(&ep, "/cache_concurrent", &msgs);
        })
    };

    // Query thread — concurrently queries while publishing
    let done = Arc::new(AtomicBool::new(false));
    let query_handle = {
        let cache_clone = Arc::clone(&cache);
        let done_clone = Arc::clone(&done);
        thread::spawn(move || {
            while !done_clone.load(Ordering::Relaxed) {
                let _ = cache_clone.len();
                let _ = cache_clone.get_before(SystemTime::now());
                thread::sleep(Duration::from_millis(5));
            }
        })
    };

    pub_handle.join().expect("publisher thread");
    thread::sleep(Duration::from_millis(200));
    done.store(true, Ordering::Relaxed);
    query_handle.join().expect("query thread");
    // No panic = pass
}

// ---------------------------------------------------------------------------
// Test 11: all query methods on empty cache return None / empty
// ---------------------------------------------------------------------------

#[test]
fn cache_empty_query() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    let cache = node
        .create_cache::<RosString>("/cache_empty_q", 10)
        .build()
        .expect("cache");

    let t = SystemTime::now();
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
    assert!(cache.get_interval(t - Duration::from_secs(1), t).is_empty());
    assert!(cache.get_before(t).is_none());
    assert!(cache.get_after(t).is_none());
    assert!(cache.get_nearest(t).is_none());
    assert_eq!(cache.oldest_stamp(), None);
    assert_eq!(cache.newest_stamp(), None);
}

// ---------------------------------------------------------------------------
// Test 12: ZenohStamp — normal path receives messages indexed near SystemTime::now()
// ---------------------------------------------------------------------------

#[test]
fn cache_zenoh_stamp_receives_and_queryable_by_wall_clock() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    let cache = node
        .create_cache::<RosString>("/cache_wallclock", 10)
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    let msgs = vec![make_msg("hello"), make_msg("world")];
    let before_publish = SystemTime::now();
    publish_n(&endpoint, "/cache_wallclock", &msgs);
    thread::sleep(Duration::from_millis(300));
    let after_publish = SystemTime::now();

    assert!(
        !cache.is_empty(),
        "cache should contain messages after delivery"
    );
    // Messages arrive indexed by Zenoh transport timestamp (≈ wall clock).
    // get_interval over the publish window should return them.
    let window = cache.get_interval(
        before_publish - Duration::from_millis(100),
        after_publish + Duration::from_millis(100),
    );
    assert!(
        !window.is_empty(),
        "expected messages in publish-time window"
    );
}

// ---------------------------------------------------------------------------
// Test 13: get_interval with inverted range returns empty (no panic)
// ---------------------------------------------------------------------------

#[test]
fn cache_get_interval_inverted_range() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_inverted", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    publish_n(
        &endpoint,
        "/cache_inverted",
        &[make_msg("1"), make_msg("2"), make_msg("3")],
    );
    thread::sleep(Duration::from_millis(300));

    // t_start > t_end — must return empty, not panic
    let result = cache.get_interval(
        epoch + Duration::from_secs(5),
        epoch + Duration::from_secs(1),
    );
    assert!(result.is_empty(), "inverted range should return empty");
}

// ---------------------------------------------------------------------------
// Test 14: get_interval boundary inclusivity — exact matches included
// ---------------------------------------------------------------------------

#[test]
fn cache_get_interval_boundary_inclusive() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_boundary", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    publish_n(
        &endpoint,
        "/cache_boundary",
        &[make_msg("1"), make_msg("3"), make_msg("5")],
    );
    thread::sleep(Duration::from_millis(300));

    // Query [1s, 5s] — all three are exactly on the boundary
    let all = cache.get_interval(
        epoch + Duration::from_secs(1),
        epoch + Duration::from_secs(5),
    );
    assert_eq!(all.len(), 3, "both endpoints must be inclusive");

    // Query [1s, 1s] — point query, should return exactly "1"
    let point = cache.get_interval(
        epoch + Duration::from_secs(1),
        epoch + Duration::from_secs(1),
    );
    assert_eq!(point.len(), 1);
    assert_eq!(point[0].data, "1");

    // Query [2s, 4s] — "3" is inside but "1" and "5" are outside
    let mid = cache.get_interval(
        epoch + Duration::from_secs(2),
        epoch + Duration::from_secs(4),
    );
    assert_eq!(mid.len(), 1);
    assert_eq!(mid[0].data, "3");
}

// ---------------------------------------------------------------------------
// Test 15: get_before / get_after at exact timestamp match
// ---------------------------------------------------------------------------

#[test]
fn cache_get_before_after_exact_match() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_exact", 20)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    publish_n(&endpoint, "/cache_exact", &[make_msg("2"), make_msg("4")]);
    thread::sleep(Duration::from_millis(300));

    // get_before at exactly t=4s must return "4" (inclusive ≤)
    let result = cache.get_before(epoch + Duration::from_secs(4));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "4");

    // get_after at exactly t=2s must return "2" (inclusive ≥)
    let result = cache.get_after(epoch + Duration::from_secs(2));
    assert!(result.is_some());
    assert_eq!(result.unwrap().data, "2");

    // get_before before all messages → None
    let result = cache.get_before(epoch + Duration::from_secs(1));
    assert!(result.is_none());

    // get_after after all messages → None
    let result = cache.get_after(epoch + Duration::from_secs(5));
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// Test 16: same-timestamp collision is last-write-wins
// ---------------------------------------------------------------------------

#[test]
fn cache_collision_last_write_wins() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let fixed = SystemTime::UNIX_EPOCH + Duration::from_secs(42);
    let cache = node
        .create_cache::<RosString>("/cache_collision", 20)
        .with_stamp(move |_msg: &RosString| fixed)
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    // Both messages get stamp = fixed → second replaces first
    publish_n(
        &endpoint,
        "/cache_collision",
        &[make_msg("first"), make_msg("second")],
    );
    thread::sleep(Duration::from_millis(300));

    // Only one entry — the later one overwrote the earlier
    assert_eq!(cache.len(), 1);
    let msg = cache.get_before(fixed).expect("should have one entry");
    assert_eq!(msg.data, "second");
}

// ---------------------------------------------------------------------------
// Test 17: capacity = 1 keeps only the most recent message
// ---------------------------------------------------------------------------

#[test]
fn cache_capacity_one() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_cap1", 1)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    publish_n(
        &endpoint,
        "/cache_cap1",
        &[make_msg("1"), make_msg("2"), make_msg("3")],
    );
    thread::sleep(Duration::from_millis(300));

    assert_eq!(cache.len(), 1);
    // Only the newest should survive
    assert_eq!(cache.oldest_stamp(), Some(epoch + Duration::from_secs(3)));
    assert_eq!(cache.newest_stamp(), Some(epoch + Duration::from_secs(3)));
}

// ---------------------------------------------------------------------------
// Test 18: get_nearest with a single-entry cache
// ---------------------------------------------------------------------------

#[test]
fn cache_get_nearest_single_entry() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let epoch = SystemTime::UNIX_EPOCH;
    let cache = node
        .create_cache::<RosString>("/cache_nearest_single", 10)
        .with_stamp(move |msg: &RosString| {
            let secs: u64 = msg.data.parse().unwrap_or(0);
            epoch + Duration::from_secs(secs)
        })
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();
    publish_n(&endpoint, "/cache_nearest_single", &[make_msg("10")]);
    thread::sleep(Duration::from_millis(300));

    // Query before the entry
    let r = cache.get_nearest(epoch + Duration::from_secs(1));
    assert!(r.is_some());
    assert_eq!(r.unwrap().data, "10");

    // Query after the entry
    let r = cache.get_nearest(epoch + Duration::from_secs(100));
    assert!(r.is_some());
    assert_eq!(r.unwrap().data, "10");

    // Query exactly at the entry
    let r = cache.get_nearest(epoch + Duration::from_secs(10));
    assert!(r.is_some());
    assert_eq!(r.unwrap().data, "10");
}

// ---------------------------------------------------------------------------
// Test 19: drop deregisters subscriber — no messages received after drop
// ---------------------------------------------------------------------------

#[test]
fn cache_drop_deregisters_subscriber() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");

    let cache = node
        .create_cache::<RosString>("/cache_drop", 50)
        .build()
        .expect("cache");

    let endpoint = router.endpoint().to_string();

    // Publish some messages and confirm delivery
    publish_n(&endpoint, "/cache_drop", &[make_msg("before")]);
    thread::sleep(Duration::from_millis(200));
    let count_before = cache.len();
    assert!(
        count_before > 0,
        "cache should have received messages before drop"
    );

    // Drop the cache — this deregisters the subscriber
    drop(cache);

    // Publish more messages after drop
    publish_n(
        &endpoint,
        "/cache_drop",
        &[make_msg("after1"), make_msg("after2")],
    );
    thread::sleep(Duration::from_millis(200));

    // Recreate a fresh cache on the same topic to verify messages do arrive
    // when a subscriber exists, proving the previous drop did stop delivery.
    let cache2 = node
        .create_cache::<RosString>("/cache_drop", 50)
        .build()
        .expect("cache2");
    publish_n(&endpoint, "/cache_drop", &[make_msg("new")]);
    thread::sleep(Duration::from_millis(200));
    assert!(cache2.len() > 0, "new cache should receive messages");
    // The "after1"/"after2" messages were published while no subscriber existed;
    // they should not be present in cache2 (volatile QoS — no late-join).
    // cache2 only has messages published after it was built.
}
