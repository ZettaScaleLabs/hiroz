use std::{sync::Arc, thread, time::Duration};

use hiroz::queue::BoundedQueue;

#[test]
fn test_push_pop_basic() {
    let q = BoundedQueue::new(5);
    assert!(!q.push(1));
    assert!(!q.push(2));
    assert_eq!(q.try_recv(), Some(1));
    assert_eq!(q.try_recv(), Some(2));
    assert_eq!(q.try_recv(), None);
}

#[test]
fn test_drop_oldest_on_overflow() {
    let q = BoundedQueue::new(3);
    assert!(!q.push(1));
    assert!(!q.push(2));
    assert!(!q.push(3));
    assert!(q.push(4)); // Should drop 1
    assert!(q.push(5)); // Should drop 2
    assert_eq!(q.try_recv(), Some(3));
    assert_eq!(q.try_recv(), Some(4));
    assert_eq!(q.try_recv(), Some(5));
    assert_eq!(q.try_recv(), None);
}

#[test]
fn test_recv_timeout_expires() {
    let q: BoundedQueue<i32> = BoundedQueue::new(5);
    let start = std::time::Instant::now();
    assert!(q.recv_timeout(Duration::from_millis(50)).is_none());
    assert!(start.elapsed() >= Duration::from_millis(50));
}

#[test]
fn test_recv_timeout_succeeds() {
    let q = Arc::new(BoundedQueue::new(5));
    let q2 = q.clone();

    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        q2.push(42);
    });

    let result = q.recv_timeout(Duration::from_millis(100));
    assert_eq!(result, Some(42));
    handle.join().unwrap();
}

#[test]
fn test_blocking_recv() {
    let q = Arc::new(BoundedQueue::new(5));
    let q2 = q.clone();

    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        q2.push(42);
    });

    let val = q.recv();
    assert_eq!(val, 42);
    handle.join().unwrap();
}

#[tokio::test]
async fn test_recv_async() {
    let q = Arc::new(BoundedQueue::new(5));
    let q2 = q.clone();

    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        q2.push(42);
    });

    let val = q.recv_async().await;
    assert_eq!(val, 42);
    handle.await.unwrap();
}

#[test]
fn test_is_empty_and_len() {
    let q = BoundedQueue::new(5);
    assert!(q.is_empty());
    assert_eq!(q.len(), 0);

    q.push(1);
    assert!(!q.is_empty());
    assert_eq!(q.len(), 1);

    q.push(2);
    assert_eq!(q.len(), 2);

    q.try_recv();
    assert_eq!(q.len(), 1);
}

#[test]
fn test_capacity_one() {
    let q = BoundedQueue::new(1);
    assert!(!q.push(1));
    assert!(q.push(2)); // Drops 1
    assert!(q.push(3)); // Drops 2
    assert_eq!(q.try_recv(), Some(3));
}

// === CI Regression Tests ===

/// Sanity check: ensure push+pop cycle is under 1µs per operation
#[test]
fn test_performance_sanity() {
    let queue = BoundedQueue::new(1000);
    let iterations = 10_000;

    let start = std::time::Instant::now();
    for i in 0..iterations {
        queue.push(i);
        queue.try_recv();
    }
    let elapsed = start.elapsed();
    let per_op = elapsed.as_nanos() / (iterations * 2) as u128;

    assert!(
        per_op < 1000,
        "Performance regression: {}ns per op (expected < 1000ns)",
        per_op
    );
}
