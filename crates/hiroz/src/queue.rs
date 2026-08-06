//! Bounded queue implementation for ROS depth QoS behavior.
//!
//! This module provides a thread-safe bounded queue that drops the OLDEST element
//! when full, matching the expected behavior of ROS 2 depth QoS.

use std::collections::VecDeque;
use std::time::Duration;

use event_listener::Event;
use parking_lot::{Condvar, Mutex};

/// A bounded queue that drops the OLDEST element when full (ROS depth QoS behavior).
///
/// Unlike channels that block or drop the newest element when full, this queue
/// maintains the most recent N elements, where N is the capacity.
pub struct BoundedQueue<T> {
    data: Mutex<VecDeque<T>>,
    /// Condvar for blocking recv operations
    not_empty: Condvar,
    /// Runtime-agnostic async notification
    event: Event,
    /// Maximum capacity (usize::MAX = unlimited for KeepAll)
    capacity: usize,
}

impl<T> BoundedQueue<T> {
    /// Create a new bounded queue with the specified capacity.
    ///
    /// A capacity of `usize::MAX` effectively makes the queue unbounded (KeepAll).
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
            not_empty: Condvar::new(),
            event: Event::new(),
            capacity,
        }
    }

    /// Push an item to the queue, dropping the OLDEST if at capacity.
    ///
    /// Returns `true` if an item was dropped, `false` otherwise.
    pub fn push(&self, item: T) -> bool {
        let mut data = self.data.lock();
        let dropped = if data.len() >= self.capacity {
            data.pop_front();
            true
        } else {
            false
        };
        data.push_back(item);
        self.not_empty.notify_one();
        self.event.notify(1);
        dropped
    }

    /// Blocking receive - waits until an item is available.
    pub fn recv(&self) -> T {
        let mut data = self.data.lock();
        while data.is_empty() {
            self.not_empty.wait(&mut data);
        }
        data.pop_front()
            .expect("queue should not be empty after wait")
    }

    /// Receive with timeout.
    ///
    /// Returns `Some(item)` if an item was received within the timeout,
    /// `None` if the timeout expired.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<T> {
        let mut data = self.data.lock();
        if data.is_empty() {
            let result = self.not_empty.wait_for(&mut data, timeout);
            if result.timed_out() || data.is_empty() {
                return None;
            }
        }
        data.pop_front()
    }

    /// Non-blocking receive.
    ///
    /// Returns `Some(item)` if an item was available, `None` otherwise.
    pub fn try_recv(&self) -> Option<T> {
        self.data.lock().pop_front()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.data.lock().is_empty()
    }

    /// Get the current number of items in the queue.
    pub fn len(&self) -> usize {
        self.data.lock().len()
    }

    /// Async receive - waits until an item is available.
    ///
    /// This method is cancel-safe: if the future is dropped before completion,
    /// no item will be lost.
    pub async fn recv_async(&self) -> T {
        loop {
            // Register listener before checking to avoid race
            let listener = self.event.listen();

            // Check if there's an item available
            if let Some(item) = self.try_recv() {
                return item;
            }

            // Wait for notification
            listener.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zero capacity retains one sample, not zero.
    ///
    /// `pubsub::dispatch_capacity` floors a zero history depth at 1 while the
    /// queue-mode path passes the 0 straight through, so the two sizing
    /// expressions differ. This pins the reason that divergence is harmless:
    /// `push` evicts *before* it inserts, so capacity 0 behaves as capacity 1
    /// for retention. If `push` is ever reordered to insert-then-evict, a
    /// zero-depth queue starts discarding every sample and this fails.
    #[test]
    fn zero_capacity_retains_one_sample() {
        let q = BoundedQueue::new(0);

        assert!(
            q.push(1),
            "capacity 0 reports a drop even on the first push"
        );
        assert_eq!(q.len(), 1, "capacity 0 must retain one sample, not zero");

        q.push(2);
        assert_eq!(q.len(), 1);
        assert_eq!(q.try_recv(), Some(2), "the newest sample is the one kept");
        assert!(q.is_empty());
    }

    /// The capacity-1 comparison the doc claims equivalence against: same
    /// retention, but no spurious drop report on the first push.
    #[test]
    fn capacity_one_retains_one_sample_without_reporting_a_drop() {
        let q = BoundedQueue::new(1);

        assert!(
            !q.push(1),
            "an empty capacity-1 queue must not report a drop"
        );
        assert_eq!(q.len(), 1);

        assert!(q.push(2), "the second push evicts the first");
        assert_eq!(q.len(), 1);
        assert_eq!(q.try_recv(), Some(2));
    }
}
