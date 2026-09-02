//! A pool of reusable message allocations for the intra-process path.
//!
//! Publishing over the [`crate::local_bus`] hands the subscriber an `Arc<T>`
//! rather than serializing, which removes the encode and the transport. What
//! remains is the allocation: a publisher that builds a fresh message per send
//! still pays for the allocation, the first touch of that memory, and the write
//! that fills it. Removing that was worth **−95% at 128 KiB and −99% at 1 MiB**
//! on the intra-process benchmark.
//!
//! Take those figures for what they measure. They remove the allocation, the
//! first touch *and* the payload write, because the benchmark rewrites eight
//! bytes of a buffer filled once at construction. A task that must genuinely
//! produce new content each cycle keeps most of that cost. A pool pays when you
//! republish, forward, or refill a fixed region — not when you generate.
//!
//! # Using it
//!
//! ```ignore
//! let mut pool = PayloadPool::new(16, ByteMultiArray::default);
//!
//! let Some(mut msg) = pool.acquire() else {
//!     // Every slot is still out. Allocate, drop the frame, or apply your own
//!     // back-pressure — the pool will not choose for you.
//!     return Ok(());
//! };
//! msg.stamp = now;                                   // written in place
//! publisher.publish_shared(msg.into_shared())?;
//! ```
//!
//! # What it will not do
//!
//! [`PayloadPool::acquire`](crate::payload_pool::PayloadPool::acquire) **never allocates**. An invisible fallback is the
//! defect this type exists to prevent: a pool that quietly allocates when it is
//! exhausted performs like no pool at all and says nothing about it.
//!
//! There is no blocking acquire. Bus delivery is synchronous on the publishing
//! thread, so the party that frees a slot is frequently the caller itself; a
//! publisher blocking on a slot only its own return can release would wait
//! forever. Waiting becomes meaningful only once a subscriber can hold a
//! message across a queue, and belongs with that design rather than before it.
//!
//! # Where it fits in hiroz
//!
//! The pool holds `Arc<T>` and nothing else — no session, no keyexpr, no
//! liveliness token — so discovery, the ROS graph and QoS negotiation neither
//! see it nor are affected by it. What matters is only whether anything
//! downstream *retains* a message, because a retained `Arc` is a slot out of
//! circulation.
//!
//! | it works with | why |
//! |---|---|
//! | the wire (`publish_shared` on a plain publisher) | serialization writes a fresh `ZBuf`, so the transport queues a copy and never holds the pooled buffer |
//! | shared memory | `serialize_to_shm` likewise copies into the SHM provider's own buffer |
//! | `TRANSIENT_LOCAL` on a wire publisher | the durability cache keeps serialized samples, not the `Arc<T>` |
//! | `Locality::Remote` (bus and wire together) | both routes release before `publish_shared` returns |
//! | many subscribers | each gets a clone of the same `Arc`; fan-out costs one slot, not N |
//! | a callback that republishes from the same pool | `into_shared` ends the pool's borrow *before* the publish, so inline delivery can re-enter it |
//!
//! | it does not work with | why |
//! |---|---|
//! | [`crate::pubsub::ZPub::publish_owned`] | takes `T` by value and gives the message away; the allocation would leave the pool for good |
//! | `TRANSIENT_LOCAL` + `with_intra_process_only()` | refused by the publisher itself, pool or not — there is no wire to hold the history |
//! | a subscriber that stores its `Arc` | a permanent capacity loss, reported through [`PoolStats::stuck`](crate::payload_pool::PoolStats::stuck) |
//! | sharing one pool across threads without a lock | `acquire` needs `&mut self`; wrap it, or give each thread its own |
//!
//! Two of these deserve a sentence more.
//!
//! **The wire is safe by copy, not by luck.** Nothing in the type system stops
//! a future serializer from splicing the payload's existing `ZSlice` instead of
//! writing a copy of it — and if one did, zenoh's TX queue would hold the
//! pooled allocation while the publisher reused it. That is why the copy is
//! pinned by a test asserting the slot comes back, rather than left as a
//! comment.
//!
//! **Rewriting bytes in place is the one part that needs more than std**, and
//! it needs nothing from hiroz. A pooled message whose fields are plain — a
//! `String`, a fixed array, a `Vec` replaced wholesale — works against
//! released zenoh today. Only overwriting bytes *inside* an existing `ZBuf`
//! requires `opt_mut_slice`, which no released `zenoh-buffers` has; a
//! workspace wanting it patches that crate and reaches the accessor through
//! [`ZBuf`](crate::zbuf::ZBuf), which is a newtype over zenoh's own. That
//! accessor refuses when anything else still references the buffer, a second
//! guard behind the `Arc` check here.
//!
//! # The failure mode to watch
//!
//! A subscriber that *stores* its `Arc` — rather than reading it and letting it
//! drop — keeps that slot out of circulation permanently. The pool cannot stop
//! this; it makes it visible. [`PoolStats::stuck`](crate::payload_pool::PoolStats::stuck) counts slots that have been
//! unavailable across many consecutive acquires, which is the signature of a
//! retained message rather than of momentary traffic.

use std::sync::Arc;

/// How many consecutive acquire attempts a slot must be unavailable for before
/// it is reported as stuck.
///
/// Under synchronous delivery a slot returns before the next send, so a slot
/// that is busy this often is being *held*, not merely in flight.
pub const STUCK_AFTER: u32 = 64;

/// A fixed set of pre-allocated messages, reused across sends.
///
/// See the [module documentation](self) for what a pool is worth and when.
pub struct PayloadPool<T> {
    slots: Vec<Arc<T>>,
    /// Consecutive acquires during which each slot was unavailable.
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
    /// Slots unavailable for [`STUCK_AFTER`] consecutive acquires — almost
    /// always a subscriber that stored its `Arc`.
    ///
    /// This can over-report by a slot that has since been released but not yet
    /// reacquired: the scan stops at the first free slot, so a later one keeps
    /// its streak until the cursor reaches it. It corrects itself within one
    /// rotation. Counting exactly would mean sweeping every slot on every
    /// acquire, which is the cost this pool exists to avoid.
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
    /// **Never allocates.** `None` is a decision for the caller, not something
    /// to paper over.
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
                    "a pooled payload slot has been held for {} consecutive acquires. A \
                     subscriber is most likely storing its Arc rather than letting it drop, \
                     which removes this slot from the pool permanently. Effective capacity \
                     is now one lower.",
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
    /// This is the moment the slot becomes busy: the refcount goes from one to
    /// two, and `acquire` skips it until every receiver has dropped its
    /// reference. Keeping that at the call site — rather than inside a
    /// `publish` method — is deliberate, so a reader can see where capacity is
    /// consumed.
    ///
    /// The result is a plain `Arc<T>`, so it suits anything that takes one:
    /// `ZPub::publish_shared`, a channel, another thread.
    ///
    /// **Not `ZPub::publish_owned`.** That takes `T` by value and hands a sole
    /// receiver the message to own and mutate, which is the opposite of
    /// pooling — the allocation would leave the pool and never come back.
    /// Pooling and giving away are alternatives, not a sequence.
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
    fn a_slot_is_reused_rather_than_reallocated() {
        let mut pool = PayloadPool::new(2, Msg::default);
        let addr = {
            let held = pool.acquire().expect("fresh pool").into_shared();
            Arc::as_ptr(&held) as usize
        };
        // Round the ring back to the same slot; the first is free again.
        let _second = pool.acquire().expect("second slot").into_shared();
        let again = pool.acquire().expect("first slot, returned").into_shared();
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
        // The hand-rolled version tested `strong_count == 1` and then unwrapped
        // `Arc::get_mut`, which ALSO requires no `Weak`. A subscriber holding a
        // Weak panicked the sender.
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
        assert_eq!(m.stamp, 4242, "Deref does not see the write");
        let published = m.into_shared();
        assert_eq!(published.stamp, 4242);
    }

    #[test]
    fn a_returned_slot_becomes_acquirable_again() {
        let mut pool = PayloadPool::new(1, Msg::default);
        let held = pool.acquire().expect("fresh pool").into_shared();
        assert!(pool.acquire().is_none(), "capacity one handed out two");
        drop(held);
        assert!(
            pool.acquire().is_some(),
            "a slot whose receivers all dropped was not returned to the pool"
        );
    }

    #[test]
    fn a_zero_capacity_pool_is_permanently_exhausted_rather_than_panicking() {
        let mut pool = PayloadPool::new(0, Msg::default);
        assert!(pool.acquire().is_none());
        assert_eq!(pool.stats().exhaustions, 1);
    }
}
