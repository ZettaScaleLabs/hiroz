//! A pool of reusable message allocations for the intra-process path.
//!
//! Bus delivery hands each subscriber an `Arc<T>`, so the per-send cost left is
//! allocating and filling the message. A pool removes the allocation when a
//! publisher refills the same messages each cycle.
//!
//! ```ignore
//! let mut pool = PayloadPool::new(16, || ByteMultiArray {
//!     data: vec![0; 1 << 20],
//!     ..Default::default()
//! });
//!
//! let Some(mut msg) = pool.acquire() else {
//!     return Ok(()); // every slot is still out: the caller decides what to do
//! };
//! msg.data[..8].copy_from_slice(&seq.to_le_bytes()); // written in place
//! publisher.publish_shared(msg.into_shared())?;
//! ```
//!
//! [`PayloadPool::acquire`](crate::payload_pool::PayloadPool::acquire) never
//! allocates a message: it returns `None` when every slot is out. There is no
//! blocking acquire, because bus delivery is inline and the holder of a slot is
//! often the caller itself.
//!
//! A slot returns to the pool when its last `Arc` is dropped:
//!
//! | used with | slot returns | why |
//! |---|---|---|
//! | the wire, or SHM | yes | serialization writes a new buffer, so the transport never holds the `Arc<T>` |
//! | `TRANSIENT_LOCAL` on a wire publisher | yes | the durability cache holds serialized samples |
//! | `Locality::Remote` | yes | bus and wire both release before `publish_shared` returns |
//! | many subscribers | yes | each gets a clone of one `Arc`, so fan-out costs one slot |
//! | a callback that republishes from the same pool | yes | `into_shared` ends the pool's borrow before the publish |
//! | a subscriber that stores its `Arc` | not while stored | counted in [`PoolStats::stuck`](crate::payload_pool::PoolStats::stuck) |
//! | [`ZPub::publish_moved`](crate::pubsub::ZPub::publish_moved) | never | it takes `T` by value, so the allocation leaves the pool |
//!
//! `acquire` takes `&mut self`: share a pool across threads behind a lock, or
//! give each thread its own. A message field that is a `ZBuf` is covered by the
//! `Arc` check only; whether a serializer shares that buffer is not tested here.

use std::sync::Arc;

/// How many consecutive times `acquire` must find a slot unavailable before it
/// is reported as stuck.
///
/// `acquire` stops at the first free slot, so it visits a held slot about once
/// per rotation, not once per call: with `n` slots expect the report after
/// roughly `STUCK_AFTER * (n - 1)` acquires. Under synchronous delivery a slot
/// returns before the next send, so a slot that is busy this often is being
/// *held*, not merely in flight.
pub const STUCK_AFTER: u32 = 64;

/// A fixed set of pre-allocated messages, reused across sends.
///
/// See the [module documentation](self) for what a pool is worth and when.
pub struct PayloadPool<T> {
    slots: Vec<Arc<T>>,
    /// Consecutive visits by `acquire` that found each slot unavailable.
    busy_streak: Vec<u32>,
    /// Whether a stuck slot has been reported, so the warning is not repeated
    /// on every send.
    warned: Vec<bool>,
    cursor: usize,
    exhaustions: u64,
}

/// A slot borrowed from a [`PayloadPool`], writable in place.
///
/// Deref to `T` to read, `DerefMut` to write, and [`Pooled::into_shared`] to
/// take the `Arc` for publishing.
pub struct Pooled<'a, T> {
    slot: &'a mut Arc<T>,
}

/// A snapshot of a pool's occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Slots the pool was built with.
    pub capacity: usize,
    /// Slots acquirable right now.
    pub available: usize,
    /// Slots that `acquire` found unavailable on [`STUCK_AFTER`] consecutive
    /// visits — almost always a subscriber that stored its `Arc`.
    ///
    /// This can over-report by a slot that has since been released but not yet
    /// reacquired: the scan stops at the first free slot, so a later one keeps
    /// its streak until the cursor reaches it. It corrects itself within one
    /// rotation.
    pub stuck: usize,
    /// Times `acquire` found nothing free.
    pub exhaustions: u64,
}

impl<T> PayloadPool<T> {
    /// Build `capacity` messages up front.
    ///
    /// `init` runs `capacity` times. Allocate the payload here — that is the
    /// allocation the pool exists to do once instead of per send.
    pub fn new(capacity: usize, mut init: impl FnMut() -> T) -> Self {
        Self {
            slots: (0..capacity).map(|_| Arc::new(init())).collect(),
            busy_streak: vec![0; capacity],
            warned: vec![false; capacity],
            cursor: 0,
            exhaustions: 0,
        }
    }

    /// Take a slot nobody else holds, or `None` when every slot is still out.
    ///
    /// The test is [`Arc::get_mut`] itself rather than a strong-count check
    /// followed by an unwrap. `get_mut` also requires that no `Weak` exists, so
    /// a subscriber holding a `Weak` makes this skip the slot, where a
    /// count-then-unwrap would panic on a precondition it never established.
    pub fn acquire(&mut self) -> Option<Pooled<'_, T>> {
        let n = self.slots.len();
        if n == 0 {
            self.exhaustions = self.exhaustions.saturating_add(1);
            return None;
        }

        let mut found = None;
        for step in 0..n {
            let i = (self.cursor + step) % n;
            if Arc::get_mut(&mut self.slots[i]).is_some() {
                found = Some(i);
                break;
            }
            self.busy_streak[i] = self.busy_streak[i].saturating_add(1);
            if self.busy_streak[i] >= STUCK_AFTER && !self.warned[i] {
                self.warned[i] = true;
                tracing::warn!(
                    slot = i,
                    consecutive = STUCK_AFTER,
                    "a pooled payload slot was unavailable on {} consecutive visits. A \
                     subscriber is most likely storing its Arc rather than letting it drop, \
                     which removes this slot from the pool until the Arc is dropped.",
                    STUCK_AFTER
                );
            }
        }

        match found {
            Some(i) => {
                self.busy_streak[i] = 0;
                self.warned[i] = false;
                self.cursor = (i + 1) % n;
                Some(Pooled {
                    slot: &mut self.slots[i],
                })
            }
            None => {
                self.exhaustions = self.exhaustions.saturating_add(1);
                None
            }
        }
    }

    /// Occupancy, for a caller that wants to react before exhaustion.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            capacity: self.slots.len(),
            available: self
                .slots
                .iter()
                .filter(|s| Arc::strong_count(s) == 1 && Arc::weak_count(s) == 0)
                .count(),
            stuck: self
                .busy_streak
                .iter()
                .filter(|&&n| n >= STUCK_AFTER)
                .count(),
            exhaustions: self.exhaustions,
        }
    }

    /// The number of slots the pool was built with.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
}

impl<T> std::ops::Deref for Pooled<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.slot
    }
}

impl<T> std::ops::DerefMut for Pooled<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::get_mut(self.slot)
            .expect("acquire established sole ownership and the borrow preserves it")
    }
}

impl<T> Pooled<'_, T> {
    /// Take the `Arc` for publishing.
    ///
    /// The slot becomes busy here: `acquire` skips it until every receiver has
    /// dropped its reference. The result is a plain `Arc<T>`, for
    /// `ZPub::publish_shared`, a channel or another thread.
    pub fn into_shared(self) -> Arc<T> {
        Arc::clone(self.slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, Debug)]
    struct Msg {
        stamp: u64,
    }

    #[test]
    fn a_returned_slot_is_reused_rather_than_reallocated() {
        let mut pool = PayloadPool::new(1, Msg::default);
        let held = pool.acquire().expect("fresh pool").into_shared();
        let addr = Arc::as_ptr(&held) as usize;
        assert!(pool.acquire().is_none(), "capacity one handed out two");
        drop(held);
        let again = pool
            .acquire()
            .expect("a slot whose receivers all dropped was not returned to the pool")
            .into_shared();
        assert_eq!(
            Arc::as_ptr(&again) as usize,
            addr,
            "the slot was reallocated: this pool is not reusing anything"
        );
    }

    #[test]
    fn a_retained_arc_removes_a_slot_and_is_eventually_reported() {
        let mut pool = PayloadPool::new(2, Msg::default);
        // A "subscriber" that stores its message instead of dropping it.
        let _retained = pool.acquire().expect("fresh pool").into_shared();
        assert_eq!(
            pool.stats().available,
            1,
            "the retained slot still counts as available"
        );

        for _ in 0..(STUCK_AFTER + 2) {
            drop(pool.acquire().map(|m| m.into_shared()));
        }
        assert_eq!(
            pool.stats().stuck,
            1,
            "a permanently held slot was never reported as stuck"
        );
    }

    #[test]
    fn a_held_slot_in_a_larger_pool_is_reported_after_threshold_visits_not_calls() {
        // `acquire` stops at the first free slot, so with four slots it visits a
        // held one only on every third call. `stuck` counts visits.
        let mut pool = PayloadPool::new(4, Msg::default);
        let _retained = pool.acquire().expect("fresh pool").into_shared();

        for _ in 0..(STUCK_AFTER + 2) {
            drop(pool.acquire().map(|m| m.into_shared()));
        }
        assert_eq!(
            pool.stats().stuck,
            0,
            "reported after as few calls as the threshold: the unit is no longer a visit"
        );

        for _ in 0..(STUCK_AFTER * 3) {
            drop(pool.acquire().map(|m| m.into_shared()));
        }
        assert_eq!(
            pool.stats().stuck,
            1,
            "a held slot was not reported after enough visits"
        );
    }

    #[test]
    fn exhaustion_returns_none_and_never_allocates() {
        let mut pool = PayloadPool::new(2, Msg::default);
        let _a = pool.acquire().expect("slot 0").into_shared();
        let _b = pool.acquire().expect("slot 1").into_shared();
        assert!(
            pool.acquire().is_none(),
            "the pool handed out a third slot from a capacity of two"
        );
        assert_eq!(pool.stats().exhaustions, 1);
        assert_eq!(pool.stats().available, 0);
        assert_eq!(pool.capacity(), 2, "capacity grew: acquire allocated");
    }

    #[test]
    fn a_weak_reference_is_skipped_rather_than_panicking() {
        // `Arc::get_mut` requires no `Weak` as well as one strong reference, so a
        // slot a `Weak` still points at is skipped, not handed out.
        let mut pool = PayloadPool::new(1, Msg::default);
        let shared = pool.acquire().expect("fresh pool").into_shared();
        let weak = Arc::downgrade(&shared);
        drop(shared);
        assert_eq!(weak.strong_count(), 1, "only the pool's own Arc remains");
        assert!(
            pool.acquire().is_none(),
            "acquire handed out a slot that still has a Weak pointing at it"
        );
        drop(weak);
        assert!(
            pool.acquire().is_some(),
            "the slot did not return once the Weak was dropped"
        );
    }

    #[test]
    fn writing_through_the_guard_lands_in_the_published_arc() {
        let mut pool = PayloadPool::new(1, Msg::default);
        let mut m = pool.acquire().expect("fresh pool");
        m.stamp = 4242;
        let published = m.into_shared();
        assert_eq!(published.stamp, 4242);
    }

    #[test]
    fn a_zero_capacity_pool_is_permanently_exhausted_rather_than_panicking() {
        let mut pool = PayloadPool::new(0, Msg::default);
        assert!(pool.acquire().is_none());
        assert_eq!(pool.stats().exhaustions, 1);
    }
}
