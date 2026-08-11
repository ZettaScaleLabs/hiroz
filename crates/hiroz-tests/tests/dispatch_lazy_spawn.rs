//! The drain thread starts on first use, not at subscriber construction.
//!
//! Three properties, each with a revert that makes it fail:
//!
//! | test | revert that fails it |
//! |---|---|
//! | `..._spawns_no_thread` | start the thread in `CallbackDispatcher::new` |
//! | `..._spawn_exactly_one_thread` | drop the `thread` mutex from `ensure_thread` |
//! | `..._while_the_first_sample_is_in_flight` | hold the spawn lock across `join()` in `Drop` |
//!
//! Thread counts come from `/proc/self/status`, so the first two are Linux-only.
//! The teardown test is portable and is the one that guards a *deadlock*, so it
//! is deliberately not gated.

use std::{
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;

/// Wall-clock budget for a teardown that must not block.
///
/// Two orders of magnitude above the work involved (dropping a subscriber with
/// at most one in-flight callback). Sized for headroom on a loaded CI box, not
/// tuned to an observed duration: the failure it guards is an *indefinite*
/// hang, so any finite budget separates pass from fail.
const TEARDOWN_BUDGET: Duration = Duration::from_secs(10);

#[cfg(target_os = "linux")]
fn thread_count() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Threads:") {
            return rest.trim().parse().expect("parse Threads");
        }
    }
    panic!("no Threads: field in /proc/self/status");
}

/// A callback subscriber that never receives a locally published sample costs
/// no thread.
///
/// This is the whole point of the change. Reverting to an eager spawn makes the
/// delta `N` instead of `0`.
#[cfg(target_os = "linux")]
#[test]
fn a_callback_subscriber_with_no_local_publish_spawns_no_thread() -> Result<()> {
    const N: usize = 8;

    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("lazy_spawn_none").build()?;

    // Let the session's own threads settle before taking the baseline.
    std::thread::sleep(Duration::from_millis(500));
    let before = thread_count();

    let mut subs = Vec::with_capacity(N);
    for i in 0..N {
        subs.push(
            node.create_sub::<RosString>(&format!("/lazy_none_{i}"))
                .build_with_callback(move |_msg| {})?,
        );
    }
    std::thread::sleep(Duration::from_millis(500));

    let after = thread_count();
    assert_eq!(
        after,
        before,
        "creating {N} callback subscribers started {} drain thread(s); none should start \
         until a sample is enqueued",
        after.saturating_sub(before)
    );

    drop(subs);
    Ok(())
}

/// Concurrent first enqueues start exactly one thread.
///
/// `ensure_thread` is reached from every enqueue. Without the mutex around the
/// handle slot, racing callers each spawn one and all but the last handle is
/// leaked — the process keeps threads nobody joins.
#[cfg(target_os = "linux")]
#[test]
fn concurrent_first_publishes_spawn_exactly_one_thread() -> Result<()> {
    const PUBLISHERS: usize = 8;

    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("lazy_spawn_race").build()?;

    let seen = Arc::new(AtomicUsize::new(0));
    let seen_cb = seen.clone();
    let _sub = node
        .create_sub::<RosString>("/lazy_race")
        .build_with_callback(move |_msg| {
            seen_cb.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = Arc::new(node.create_pub::<RosString>("/lazy_race").build()?);

    std::thread::sleep(Duration::from_millis(500));
    let before = thread_count();

    // Release all publishing threads at once so their first enqueues overlap.
    let barrier = Arc::new(Barrier::new(PUBLISHERS));
    let mut handles = Vec::with_capacity(PUBLISHERS);
    for i in 0..PUBLISHERS {
        let (b, p) = (barrier.clone(), publisher.clone());
        handles.push(std::thread::spawn(move || {
            b.wait();
            let _ = p.publish(&RosString {
                data: format!("m{i}"),
            });
        }));
    }
    for h in handles {
        h.join().expect("publisher thread");
    }
    std::thread::sleep(Duration::from_millis(1000));

    let after = thread_count();
    assert_eq!(
        after - before,
        1,
        "{PUBLISHERS} concurrent first publishes started {} drain threads; exactly one \
         dispatcher must exist per subscriber",
        after - before
    );
    Ok(())
}

/// Dropping a subscriber while its first sample is being enqueued must not hang.
///
/// The hazard is lock ordering in `Drop`: if it holds the spawn lock across
/// `join()`, a callback that publishes to its own topic blocks in
/// `ensure_thread` while `Drop` waits for that same thread. Reintroducing that
/// makes this test hang rather than fail, which is why it runs under a budget
/// in its own thread.
#[test]
fn dropping_a_subscriber_while_the_first_sample_is_in_flight_does_not_hang() -> Result<()> {
    let (done_tx, done_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let scenario = || -> Result<()> {
            let ctx = ZContextBuilder::default().build()?;
            let node = ctx.create_node("lazy_spawn_teardown").build()?;

            // The callback republishes to its own topic, so it re-enters the
            // enqueue path — and therefore `ensure_thread` — from the drain
            // thread itself. Built before the subscriber so its type is
            // inferred and never has to be named.
            let echo = Arc::new(node.create_pub::<RosString>("/lazy_teardown").build()?);
            let echo_cb = echo.clone();
            let depth = Arc::new(AtomicUsize::new(0));
            let depth_cb = depth.clone();

            let sub = node
                .create_sub::<RosString>("/lazy_teardown")
                .build_with_callback(move |_msg| {
                    // Bound the echo so the test cannot livelock if teardown is
                    // slow; the property under test is that `drop` returns, not
                    // how many samples flow.
                    if depth_cb.fetch_add(1, Ordering::SeqCst) < 64 {
                        let _ = echo_cb.publish(&RosString {
                            data: "echo".to_string(),
                        });
                    }
                })?;

            echo.publish(&RosString {
                data: "start".to_string(),
            })?;

            // Drop while the callback chain is live.
            drop(sub);
            Ok(())
        };
        let outcome = scenario();
        let _ = done_tx.send(outcome.is_ok());
    });

    match done_rx.recv_timeout(TEARDOWN_BUDGET) {
        Ok(true) => Ok(()),
        Ok(false) => panic!("scenario returned an error"),
        // Distinguish the two failure modes explicitly: a panicking scenario
        // drops the sender and reports `Disconnected`, which must not be
        // reported as the deadlock this test exists to catch.
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!(
            "dropping a subscriber during its first enqueue did not complete within {:?} — \
             teardown deadlocked",
            TEARDOWN_BUDGET
        ),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("scenario thread panicked before completing")
        }
    }
}
