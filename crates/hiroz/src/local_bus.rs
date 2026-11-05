//! Intra-process message bus (prototype).
//!
//! Zenoh payloads are bytes, so every hiroz message is CDR-encoded on publish and
//! decoded on receive, even when both endpoints share a process.
//! [`Locality::SessionLocal`](zenoh::sample::Locality) removes the transport but
//! not the encoding. This module removes the encoding when the publisher and the
//! subscriber are in the same zenoh session and use the same concrete Rust type.
//! The publisher hands out an `Arc<T>` and each matching subscriber gets a
//! refcount bump. Nothing is serialized or copied.
//!
//! # Why this is not the default
//!
//! Bytes give ROS late binding: the peer may be another process, language, ROS
//! version or host, and an `Arc<T>` survives none of that. This is an opt-in fast
//! path beside the wire path, not a replacement for it.
//!
//! # Scope
//!
//! | | here | not here |
//! |---|---|---|
//! | audience | every same-session subscriber registered on this channel | subscribers reached only over the wire |
//! | type check | exact [`core::any::TypeId`] | any structural or version-tolerant match |
//! | mutability | shared `Arc<T>`, or moved to a sole receiver | a receiver mutating a payload others still hold |
//! | choosing the path | the caller asserts the audience | inferring it |
//!
//! The audience is never inferred. A plain zenoh subscriber declares no ROS
//! liveliness token, so the graph cannot see it, and a wrong inference would lose
//! its messages silently. The caller asserts the audience with
//! `with_intra_process_only()` or a `Locality`. The bus replaces the wire only
//! when that assertion makes the wire redundant.
//!
//! # Keying
//!
//! Channels are keyed by `(session zid, topic key expression)`. The zid makes
//! "same session" true rather than "same process": two
//! [`ZContext`](crate::context::ZContext)s in one process open two sessions and
//! do not see each other's traffic. A publisher resolves its
//! [`crate::local_bus::Channel`] once, at build time, and never touches the
//! registry again.
//!
//! # Locking
//!
//! Delivery never invokes a callback while holding a lock. A callback may
//! publish, and re-entering the bus under a held guard would deadlock. Each
//! publish reads a snapshot of the subscriber list.

use std::{
    any::{Any, TypeId},
    cell::Cell,
    collections::HashMap,
    sync::{
        Arc, LazyLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

// Only the debug-only re-raise below reads this, so the import is gated too.
#[cfg(debug_assertions)]
use crate::reentrancy::ReentrancyViolation;

use arc_swap::ArcSwap;

use tracing::debug;
use zenoh::session::ZenohId;

/// How deep a chain of callback-driven publishes may go before the bus refuses.
///
/// Delivery runs inline on the publishing thread, so a callback that publishes
/// re-enters `Channel::publish` on the same stack. A callback that publishes to
/// its own topic would recurse until the stack overflows. Past this depth the bus
/// drops the message and logs an error. Eight is far above any legitimate chain.
///
/// Public so a test can assert the exact bound.
///
/// # What this does not bound
///
/// - A callback that spawns a thread and publishes there starts at depth zero,
///   because the counter is thread-local.
/// - A topic cycle across the wire. A `Locality::Remote` publisher runs both
///   routes, so a nested delivery emits one wire message per nesting level.
///   Suppressing the wire half would drop a message the callback chose to send.
///
/// The guard is not a cycle detector.
///
/// cbindgen:ignore
pub const MAX_DELIVERY_DEPTH: u32 = 8;

thread_local! {
    /// Delivery depth for the current thread, shared by all channels.
    static DELIVERY_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Restores the delivery depth even if a callback panics.
struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DELIVERY_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// An erased payload. Always an `Arc<T>` for the `T` named by `type_id`.
pub type ErasedPayload = Arc<dyn Any + Send + Sync>;

/// Takes the payload **by value**, so a sole subscriber can move it instead of cloning.
type LocalCallback = Arc<dyn Fn(ErasedPayload) + Send + Sync>;

/// A callback that takes the message owned.
///
/// Served only to a sole receiver. The shared path hands every receiver a
/// read-only `Arc`.
type OwnedCallback = Arc<dyn Fn(Box<dyn Any + Send>) + Send + Sync>;

#[derive(Clone)]
enum Sink {
    /// Receives `Arc<T>`; any number may coexist on a topic.
    Shared(LocalCallback),
    /// Receives `T` by value. Served only when it is the sole subscriber.
    Owned(OwnedCallback),
}

#[derive(Clone)]
struct Entry {
    id: u64,
    type_id: TypeId,
    sink: Sink,
}

impl Entry {
    #[inline]
    fn is_shared(&self) -> bool {
        matches!(self.sink, Sink::Shared(_))
    }

    #[inline]
    fn call_shared(&self, payload: ErasedPayload) {
        if let Sink::Shared(cb) = &self.sink {
            cb(payload);
        }
    }
}

/// The subscriber list for one `(session, topic)`.
///
/// A publisher resolves it once, at build time. See the module docs.
pub struct Channel {
    /// Snapshot of the subscribers. A publish loads it and calls straight through.
    /// A subscriber coming or going swaps in a new list, and a publish already in
    /// flight keeps the old one. No lock is held during a callback.
    entries: ArcSwap<Vec<Entry>>,
}

/// What one intra-process publish did.
///
/// `NoTaker` means no callback returned normally. That covers two cases: no
/// subscriber matched, and every callback that ran panicked. In the second case
/// a callback has run and may have had effects, such as keeping the `Arc`.
/// `NoTaker` is therefore not a safe signal to retry or to fall back to the wire.
///
/// `DepthExceeded` means the bus dropped the message before any callback ran.
/// A fallback to the wire must not follow it: the fallback re-enters the same
/// callback with a fresh depth counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// This many callbacks returned normally. A callback that panicked is not
    /// counted, so the number of callbacks that ran can be higher.
    Sent(usize),
    /// No callback returned normally: no subscriber of this type is on the bus,
    /// or every callback that ran panicked.
    NoTaker,
    /// Refused: delivery is already nested `MAX_DELIVERY_DEPTH` deep.
    DepthExceeded,
}


/// Which routes one publish took, and what the bus did on its route.
///
/// A delivered count cannot say this: zero covers "no callback returned",
/// "refused at depth" and "no bus on this publisher".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Published {
    /// zenoh only. The audience is not knowable from here.
    Wire,
    /// The intra-process bus only, with this outcome.
    Bus(Delivery),
    /// Both, to disjoint audiences: the bus for this session, the wire for
    /// everyone else. Only a `Locality::Remote` publisher does this.
    BusAndWire(Delivery),
}

/// Invoke one subscriber callback, isolated. Returns whether it returned normally.
///
/// Debug builds check that no tracked lock is held; see [`crate::reentrancy`].
///
/// A panic is contained, so one panicking subscriber neither skips the rest of
/// the snapshot nor unwinds into the publisher. Containment needs
/// `panic = "unwind"`. With `panic = "abort"` the process ends at the panic.
fn invoke_isolated(site: &'static str, f: impl FnOnce()) -> bool {
    crate::reentrancy::assert_no_guards_held(site);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => true,
        Err(_payload) => {
            // Re-raise the crate's own re-entrancy violation. A nested delivery runs
            // inside this `catch_unwind`, and logging it would hide the detector.
            #[cfg(debug_assertions)]
            // Match the type, not a message string: a subscriber could panic with any
            // string and force a re-raise.
            if _payload.downcast_ref::<ReentrancyViolation>().is_some() {
                std::panic::resume_unwind(_payload);
            }
            tracing::error!(
                "[BUS] a subscriber callback panicked during intra-process delivery at {site}; \
                 the panic was contained and delivery continued to the remaining subscribers"
            );
            // Dropping the payload is user code too, and its destructor may panic.
            // Contain that, and leak the second payload rather than drop it unguarded.
            if let Err(second) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                drop(_payload)
            })) {
                std::mem::forget(second);
            }
            false
        }
    }
}

impl Channel {
    /// How many subscribers of `T` on this channel want the message **by value**.
    ///
    /// Lets a publisher tell "nobody is listening" from "an owned subscriber is
    /// listening that [`Channel::publish`] skips". Without it, an
    /// `intra_process_only` publisher would drop the message and report success.
    /// A clone cannot serve them: `ZMessage` is not `Clone`.
    pub fn owned_receivers<T>(&self) -> usize
    where
        T: Any + 'static,
    {
        let wanted = TypeId::of::<T>();
        self.entries
            .load()
            .iter()
            .filter(|e| e.type_id == wanted && !e.is_shared())
            .count()
    }

    /// Deliver `payload` to every shared subscriber of type `T`.
    ///
    /// Returns how many callbacks returned normally. No callback runs with a lock
    /// held. [`MAX_DELIVERY_DEPTH`] bounds the nesting.
    pub fn publish<T>(&self, payload: Arc<T>) -> Delivery
    where
        T: Any + Send + Sync + 'static,
    {
        // Refuse unbounded recursion: drop the message and log, instead of
        // overflowing the stack.
        let depth = DELIVERY_DEPTH.with(|d| d.get());
        if depth >= MAX_DELIVERY_DEPTH {
            tracing::error!(
                depth,
                max = MAX_DELIVERY_DEPTH,
                "intra-process delivery nested too deeply; refusing to recurse further. \
                 A subscriber callback is publishing onto a topic that reaches itself. \
                 The message was dropped."
            );
            return Delivery::DepthExceeded;
        }
        DELIVERY_DEPTH.with(|d| d.set(depth + 1));
        let _depth_guard = DepthGuard;

        let wanted = TypeId::of::<T>();

        let entries = self.entries.load();
        let mut matching = entries
            .iter()
            .filter(|e| e.type_id == wanted)
            .filter(|e| e.is_shared());
        let Some(first) = matching.next() else {
            return Delivery::NoTaker;
        };
        let second = matching.next();

        let erased: ErasedPayload = payload;
        match second {
            // One subscriber: call through the snapshot and hand over the only
            // reference, with no refcount traffic.
            None => {
                if invoke_isolated("local_bus::publish", || first.call_shared(erased)) {
                    Delivery::Sent(1)
                } else {
                    // The only subscriber ran and panicked. Its effects are not
                    // undone, so this outcome is not a retry signal.
                    Delivery::NoTaker
                }
            }
            // Two or more receivers need a reference each, so cloning starts here.
            Some(second) => {
                let e1 = erased.clone();
                let e2 = erased.clone();
                let mut count = 0;
                count += invoke_isolated("local_bus::publish", || first.call_shared(e1)) as usize;
                count += invoke_isolated("local_bus::publish", || second.call_shared(e2)) as usize;
                for entry in matching {
                    let ec = erased.clone();
                    count += invoke_isolated("local_bus::publish", || entry.call_shared(ec)) as usize;
                }
                // Count callbacks that returned normally. Zero is NoTaker, never
                // Sent(0), as in the sole-subscriber arm: two spellings of "nothing
                // delivered" would make a caller that branches on NoTaker miss one.
                if count == 0 {
                    Delivery::NoTaker
                } else {
                    Delivery::Sent(count)
                }
            }
        }
    }

    /// Hand `payload` to a sole owning subscriber, by value.
    ///
    /// Returns the payload as `Err` when it cannot go that way: no owning subscriber
    /// of this type is registered, or another subscriber of the type is. The caller
    /// keeps the payload and can fall back. The bus does not downgrade to a shared
    /// delivery, because that would defeat the request for ownership.
    pub fn publish_owned<T>(&self, payload: T) -> core::result::Result<Delivery, T>
    where
        T: Any + Send + 'static,
    {
        let wanted = TypeId::of::<T>();
        let entries = self.entries.load();

        let mut of_type = entries.iter().filter(|e| e.type_id == wanted);
        let Some(only) = of_type.next() else {
            return Err(payload);
        };
        if of_type.next().is_some() {
            return Err(payload);
        }
        let Sink::Owned(cb) = &only.sink else {
            return Err(payload);
        };

        let depth = DELIVERY_DEPTH.with(|d| d.get());
        if depth >= MAX_DELIVERY_DEPTH {
            tracing::error!(
                depth,
                max = MAX_DELIVERY_DEPTH,
                "intra-process delivery nested too deeply; refusing to recurse further"
            );
            // Not Err(payload): the caller would treat that as "no owning receiver"
            // and fall back, re-entering this callback with a fresh depth counter.
            // Drop the message and report DepthExceeded.
            return Ok(Delivery::DepthExceeded);
        }
        DELIVERY_DEPTH.with(|d| d.set(depth + 1));
        let _depth_guard = DepthGuard;

        // A panicked callback delivered nothing, as on the shared path.
        if invoke_isolated("local_bus::publish_owned", || cb(Box::new(payload))) {
            Ok(Delivery::Sent(1))
        } else {
            Ok(Delivery::NoTaker)
        }
    }

    /// How many subscribers this channel has, of any type. Diagnostics only.
    pub fn subscriber_count(&self) -> usize {
        self.entries.load().len()
    }
}

/// Registry of channels, keyed by session then topic.
///
/// A channel that no endpoint holds is dropped the next time [`channel`]
/// misses, not when the last endpoint goes.
type Registry = HashMap<ZenohId, HashMap<String, Arc<Channel>>>;

static BUS: LazyLock<RwLock<Registry>> = LazyLock::new(|| RwLock::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// How many channels the registry holds, across every session.
///
/// Lets a test observe the reclamation in [`channel`].
pub fn total_channels() -> usize {
    let bus = match BUS.read() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };
    bus.values().map(|topics| topics.len()).sum()
}

/// How many channels the registry holds for one session. Diagnostics only.
///
/// Unlike [`total_channels`], other sessions in the process do not disturb it.
pub fn session_channels(zid: ZenohId) -> usize {
    let bus = match BUS.read() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };
    bus.get(&zid).map(|topics| topics.len()).unwrap_or(0)
}

/// Resolve the channel for `(zid, topic)`, creating it if needed.
///
/// Call this once, when a publisher or subscriber is built — not per message.
pub fn channel(zid: ZenohId, topic: &str) -> Arc<Channel> {
    // Fast path: it usually exists, and a read lock lets concurrent builders through.
    {
        let bus = match BUS.read() {
            Ok(b) => b,
            Err(e) => e.into_inner(),
        };
        if let Some(existing) = bus.get(&zid).and_then(|topics| topics.get(topic)) {
            return existing.clone();
        }
    }

    let mut bus = match BUS.write() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };

    // Reclaim while holding the write lock. A channel is removable when the
    // registry is its sole owner and it has no subscribers: every `ZPub` and
    // `ZSub` holds its own `Arc<Channel>`, so a strong count of one means no
    // endpoint can reach it. Sweep every session, because `channel()` runs in
    // every `ZPubBuilder::build()` and a retired session would otherwise keep
    // its last channel.
    bus.retain(|_, topics| {
        topics.retain(|_, ch| Arc::strong_count(ch) > 1 || !ch.entries.load().is_empty());
        !topics.is_empty()
    });

    bus.entry(zid)
        .or_default()
        .entry(topic.to_owned())
        .or_insert_with(|| {
            Arc::new(Channel {
                entries: ArcSwap::from_pointee(Vec::new()),
            })
        })
        .clone()
}

/// Keeps a local subscription alive. Dropping it unregisters.
///
/// `ZSub` holds one, so a dropped subscriber receives no further deliveries.
/// It does not quiesce: a delivery in flight on another thread runs to
/// completion against the snapshot it loaded, so the callback can run after
/// `drop` returns. That is memory-safe, because the snapshot owns the closure
/// and its captures. Do not tear down a resource the callback observes on the
/// strength of having dropped the subscriber.
pub struct LocalSubscription {
    channel: Arc<Channel>,
    id: u64,
}

impl Drop for LocalSubscription {
    fn drop(&mut self) {
        // Swap in a list without this entry. A publish in flight keeps its snapshot.
        let id = self.id;
        self.channel.entries.rcu(|current| {
            current
                .iter()
                .filter(|e| e.id != id)
                .cloned()
                .collect::<Vec<_>>()
        });
    }
}

/// Register a subscriber on `channel` for payloads of type `T`.
pub fn subscribe<T, F>(channel: Arc<Channel>, callback: F) -> LocalSubscription
where
    T: Any + Send + Sync + 'static,
    F: Fn(Arc<T>) + Send + Sync + 'static,
{
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let erased: LocalCallback = Arc::new(move |payload: ErasedPayload| {
        // The publisher matched on TypeId, so this holds. The downcast stays
        // checked: an unchecked cast would turn a bookkeeping bug into undefined
        // behaviour.
        match payload.downcast::<T>() {
            Ok(typed) => callback(typed),
            Err(_) => tracing::error!(
                "[LOCAL] payload type did not match subscriber type after a TypeId match"
            ),
        }
    });

    // Copy on write: rare, and it keeps the publish path lock-free. `rcu` may
    // rerun the closure, so it clones.
    let type_id = TypeId::of::<T>();
    channel.entries.rcu(|current| {
        let mut next = Vec::with_capacity(current.len() + 1);
        next.extend(current.iter().cloned());
        next.push(Entry {
            id,
            type_id,
            sink: Sink::Shared(erased.clone()),
        });
        next
    });

    debug!("[LOCAL] subscribed id={id}");
    LocalSubscription { channel, id }
}

/// How many local subscribers `topic` has on `zid`. Diagnostics only.
pub fn subscriber_count(zid: ZenohId, topic: &str) -> usize {
    let bus = match BUS.read() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };
    bus.get(&zid)
        .and_then(|topics| topics.get(topic))
        .map(|c| c.subscriber_count())
        .unwrap_or(0)
}

/// Register `callback` to receive messages of type `T` **by value**.
///
/// Served only when it is the sole subscriber of that type on the channel; see
/// [`Channel::publish_owned`]. Registering one beside a shared subscriber is
/// allowed, but then the owned path cannot give anything away.
pub fn subscribe_owned<T, F>(channel: Arc<Channel>, callback: F) -> LocalSubscription
where
    T: Any + Send + 'static,
    F: Fn(T) + Send + Sync + 'static,
{
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let erased: OwnedCallback = Arc::new(move |payload: Box<dyn Any + Send>| {
        // The publisher matched on TypeId, so this holds. The downcast stays
        // checked: an unchecked cast would turn a bookkeeping bug into undefined
        // behaviour.
        match payload.downcast::<T>() {
            Ok(typed) => callback(*typed),
            Err(_) => tracing::error!(
                "[LOCAL] payload type did not match an owned subscriber after a TypeId match"
            ),
        }
    });

    let type_id = TypeId::of::<T>();
    channel.entries.rcu(|current| {
        let mut next = Vec::with_capacity(current.len() + 1);
        next.extend(current.iter().cloned());
        next.push(Entry {
            id,
            type_id,
            sink: Sink::Owned(erased.clone()),
        });
        next
    });

    debug!("[LOCAL] subscribed id={id} (owned)");
    LocalSubscription { channel, id }
}
