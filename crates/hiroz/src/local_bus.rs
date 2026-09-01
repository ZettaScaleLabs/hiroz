//! Intra-process message bus — **prototype**.
//!
//! Zenoh's payload is bytes by definition, so every hiroz message is CDR-encoded
//! on publish and decoded on receive even when both endpoints live in the same
//! process. [`Locality::SessionLocal`](zenoh::sample::Locality) removes the
//! transport underneath that, but not the encoding.
//!
//! This module removes the encoding, for the one case where it is safe to: the
//! publisher and the subscriber are in the same zenoh session and agree on the
//! exact same concrete Rust type. The publisher hands out an `Arc<T>` and every
//! matching local subscriber gets a refcount bump. Nothing is serialized and the
//! payload is never copied.
//!
//! # Why this cannot be the default
//!
//! Bytes are what buy ROS its late binding: a topic is a name, and the peer is
//! discovered at run time and may be another process, another language, another
//! ROS version or another host. An `Arc<T>` survives none of that. So this is an
//! opt-in fast path beside the wire path, never a replacement for it — the same
//! shape as rclcpp's intra-process comm.
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
//! The last row is the one that has been tried and withdrawn. A publisher does
//! not ask whether anyone remote is listening, because the question has no
//! answer: a plain zenoh subscriber declares no ROS liveliness token, so the
//! graph cannot see it and there is no count to subtract our own from. Taking
//! the bus on a wrong inference loses that subscriber's messages silently. So
//! the caller says, with `with_intra_process_only()` or a `Locality`, and the
//! bus is taken only when that assertion makes the wire redundant.
//!
//! # Keying, and why a publisher resolves it once
//!
//! Channels are keyed by `(session zid, topic key expression)`. The zid is what
//! makes "same session" true rather than merely "same process" — two
//! [`ZContext`](crate::context::ZContext)s in one process open two sessions and
//! must not see each other's traffic. Both `ZPub` and `ZSub` already hold an
//! `Arc<Session>`, so this needs no plumbing through the node tree.
//!
//! A publisher takes its [`crate::local_bus::Channel`] handle when it is built and never touches
//! the registry again. Resolving per message would mean hashing a
//! fully-qualified ROS key expression on every publish, which at small payloads
//! is a visible share of the whole path.
//!
//! # Locking
//!
//! Callbacks are **never** invoked with the registry lock held. A subscriber
//! callback commonly publishes — that is exactly what the pong side of a
//! ping/pong does — and re-entering the bus under its own read guard is the
//! deadlock this workspace has already fixed three times elsewhere. The list is
//! cloned out, the guard dropped, and only then are the callbacks called.

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
/// re-enters `Channel::publish` on the same stack. A pong that publishes to a
/// *different* topic is the motivating case and terminates. A callback that
/// publishes to its **own** topic does not: on the wire that loop passes through
/// zenoh's queues and shows up as an endless stream of messages, but here it is
/// direct recursion and ends in a stack overflow.
///
/// Eight is chosen to be far above any legitimate chain — a pipeline of eight
/// nodes each publishing from the previous one's callback, on one thread — and
/// far below the depth at which the stack is in danger.
/// Public so a test can assert the exact bound rather than a loose ceiling:
/// a hand-copied constant lets a change to this value slip past unnoticed.
///
/// # What this does not bound
///
/// It bounds the **stack**, on **one thread**. Two things escape it, and both
/// are inherent rather than oversights:
///
/// - **A callback that spawns a thread and publishes there** starts at depth
///   zero, because the counter is thread-local. Unbounded recursion then
///   exhausts threads rather than the stack, and this guard never trips.
/// - **A topic cycle across the wire is not bounded at all.** A
///   `Locality::Remote` publisher runs both routes, so a nested delivery emits
///   a wire message per nesting level; a peer that echoes amplifies again. That
///   is a property of publish/subscribe — any ROS 2 node that publishes to a
///   topic it subscribes to does the same, and no client library prevents it.
///   Suppressing the wire half for nested deliveries was considered and
///   rejected: a nested publish is a distinct message the callback chose to
///   send, so dropping it would trade a loud problem for a silent one.
///
/// The guard exists to turn a stack overflow into a dropped message and a
/// greppable log. It is not a cycle detector.
pub const MAX_DELIVERY_DEPTH: u32 = 8;

thread_local! {
    /// Delivery depth for the current thread. Not per channel: a cycle across
    /// two topics recurses just as fatally as one topic into itself.
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

/// Takes the payload **by value**, so the single-subscriber case can move it.
///
/// With a `&ErasedPayload` the callback had to clone before downcasting, which
/// is a refcount pair on every message. One subscriber is the overwhelmingly
/// common case, and it can be handed the only reference instead.
type LocalCallback = Arc<dyn Fn(ErasedPayload) + Send + Sync>;

/// A callback that takes the message **owned and mutable**.
///
/// The shared path hands every receiver the same read-only `Arc`, which is
/// right when several of them want it and wrong when exactly one does: a sole
/// receiver could have been given the value itself, free to mutate or consume
/// it. This is that path.
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

/// The subscriber list for one `(session, topic)`, resolved once.
///
/// A publisher takes an `Arc<Channel>` when it is built and never consults the
/// registry again. That matters: looking a topic up per message means hashing a
/// fully-qualified ROS key expression — a long string — on every publish, which
/// at 64 B payloads is a visible share of the whole path.
pub struct Channel {
    /// Read on every publish, written only when a subscriber comes or goes.
    ///
    /// This was an `RwLock<Vec<Entry>>`, and the publish path paid for it twice:
    /// the lock itself, and an `Arc` clone of each matching callback so the
    /// guard could be dropped before any callback ran. That clone was not
    /// optional — invoking a callback under the guard is the re-entrancy
    /// deadlock this workspace has fixed repeatedly, because a subscriber
    /// callback that publishes is the normal case rather than the exotic one.
    ///
    /// A snapshot removes both. A publisher loads the current list and calls
    /// straight through it; a subscriber coming or going swaps in a new list and
    /// leaves any publish already in flight running against the old one. That is
    /// the same visibility the clone gave, without the atomics, and it cannot
    /// deadlock because nothing is held.
    entries: ArcSwap<Vec<Entry>>,
}

/// What one intra-process publish did.
///
/// `NoTaker` and `DepthExceeded` both mean nothing was delivered, and they must
/// not be conflated: a caller may fall back to the wire on `NoTaker`, but doing
/// so on `DepthExceeded` re-enters the same callback on a zenoh thread with a
/// fresh depth counter and loops forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Delivery {
    /// Handed to this many subscribers.
    Sent(usize),
    /// No subscriber of this type is on the bus.
    NoTaker,
    /// Refused: delivery is already nested `MAX_DELIVERY_DEPTH` deep.
    DepthExceeded,
}

/// Which routes one publish took, and what the bus did on its route.
///
/// A caller cannot reconstruct this from a delivered count: zero means "no
/// taker", "refused at depth" and "there is no bus on this publisher", and the
/// first two must stay distinct — see [`Delivery`].
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

/// Invoke one subscriber callback, isolated.
///
/// Two things happen here that must happen at every user-code call site.
///
/// The crate contract in [`crate::reentrancy`] says every invocation of user
/// code is routed through `invoke_user_callback!`, so that calling out while a
/// tracked lock is held is caught in debug builds. Bus delivery is synchronous
/// on the publishing thread, which makes that hazard *more* reachable than the
/// wire path, not less.
///
/// And a panic is contained — **under `panic = "unwind"`, which is the
/// default**. A build that sets `panic = "abort"` cannot catch anything: the
/// process ends at the panic and this isolation is inert. That is worth knowing
/// before relying on it in an embedded or abort-configured deployment.
///
/// A panic is contained. On the wire, a panicking callback kills one zenoh
/// task. Here it would unwind into the application's publishing thread and skip
/// every subscriber after it in the snapshot. Delivery order is snapshot order,
/// so which siblings got censored would vary run to run.
fn invoke_isolated(site: &'static str, f: impl FnOnce()) -> bool {
    crate::reentrancy::assert_no_guards_held(site);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => true,
        Err(_payload) => {
            // Re-raise the crate's own re-entrancy violation instead of
            // logging it. `assert_no_guards_held` reports by panicking, and a
            // *nested* delivery runs inside this catch_unwind — so without
            // this, the detector built to catch "a user callback ran while a
            // lock was held" is silently downgraded to a log line in exactly
            // the case the bus makes most reachable: a callback that publishes.
            #[cfg(debug_assertions)]
            // A type, not a message prefix: a subscriber can panic with any
            // string it likes, so matching text would let user code force a
            // re-raise and defeat the isolation this function provides.
            if _payload.downcast_ref::<ReentrancyViolation>().is_some() {
                std::panic::resume_unwind(_payload);
            }
            tracing::error!(
                "[BUS] a subscriber callback panicked during intra-process delivery at {site}; \
                 the panic was contained and delivery continued to the remaining subscribers"
            );
            false
        }
    }
}

impl Channel {
    /// Deliver `payload` to every subscriber here whose type matches, returning
    /// how many were called.
    ///
    /// Callbacks are **never** invoked with the lock held. A subscriber callback
    /// commonly publishes — that is exactly what the pong side of a ping/pong
    /// does — and re-entering under its own read guard is the deadlock this
    /// workspace has already fixed three times elsewhere.
    /// How many subscribers of `T` on this channel want the message **by value**.
    ///
    /// A publisher needs this to tell apart two outcomes that both surface as
    /// [`Delivery::NoTaker`]: nobody is listening, and somebody is listening
    /// whom the shared path structurally cannot serve. [`Channel::publish`]
    /// filters on `is_shared`, so an owned subscriber is invisible to it — and
    /// an `intra_process_only` publisher has no wire behind the bus to catch the
    /// message. Without this, that difference is a silent drop reported as success.
    ///
    /// It cannot be repaired by handing them a clone: `ZMessage` is
    /// `Send + Sync + Sized`, not `Clone`.
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

    pub fn publish<T>(&self, payload: Arc<T>) -> Delivery
    where
        T: Any + Send + Sync + 'static,
    {
        // Refuse to recurse without bound. A callback that publishes back onto
        // its own topic would otherwise overflow the stack; returning here turns
        // that into a dropped message and a loud log, which is recoverable and
        // greppable.
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
            // One subscriber is the overwhelmingly common case: call straight
            // through the snapshot, hand over the only reference, and touch no
            // refcount at all beyond the one the caller already holds.
            None => {
                if invoke_isolated("local_bus::publish", || first.call_shared(erased)) {
                    Delivery::Sent(1)
                } else {
                    // The only subscriber panicked. Reporting Sent(1) here told
                    // the caller a message was delivered when none was.
                    Delivery::NoTaker
                }
            }
            // More than one receiver genuinely needs a reference each, so the
            // clones start here and not before.
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
                // The count is subscribers that returned normally, not
                // subscribers invoked. A panicking one is logged above.
                //
                // Zero is NoTaker, never Sent(0): the sole-subscriber arm above
                // already reports NoTaker for the same outcome, and two spellings
                // of "nothing was delivered" mean a caller that branches on
                // NoTaker silently misses one of them.
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
    /// Returns the payload back as `Err` when it cannot be delivered that way,
    /// so the caller still owns it and can fall back rather than lose it. That
    /// happens when no owning subscriber of this type is registered, or when
    /// anything else is subscribed as well: with a second receiver the message
    /// cannot be given away, and silently downgrading to a shared delivery
    /// would defeat the point of having asked for ownership.
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
            // Deliberately NOT Err(payload): the caller treats that as "no
            // owning receiver" and falls back, which re-enters this callback on
            // a zenoh thread with a fresh depth counter and loops forever. The
            // message is dropped, and `DepthExceeded` says so — reporting it as
            // a delivery would hide a dropped message behind a success.
            return Ok(Delivery::DepthExceeded);
        }
        DELIVERY_DEPTH.with(|d| d.set(depth + 1));
        let _depth_guard = DepthGuard;

        // Mirror the shared path: a callback that panicked delivered nothing, so
        // reporting Sent(1) would tell the caller a message landed when none did.
        if invoke_isolated("local_bus::publish_owned", || cb(Box::new(payload))) {
            Ok(Delivery::Sent(1))
        } else {
            Ok(Delivery::NoTaker)
        }
    }

    /// How many subscribers this channel has, regardless of type. Diagnostics
    /// only; the publish path does not use it.
    pub fn subscriber_count(&self) -> usize {
        self.entries.load().len()
    }
}

/// Registry of channels, keyed by session then topic.
///
/// Channels are created on demand and never removed. One empty `Channel` per
/// `(session, topic)` ever used is a bounded, trivial cost, and keeping them
/// means a publisher's handle stays valid across a subscriber coming and going.
type Registry = HashMap<ZenohId, HashMap<String, Arc<Channel>>>;

static BUS: LazyLock<RwLock<Registry>> = LazyLock::new(|| RwLock::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Resolve the channel for `(zid, topic)`, creating it if needed.
///
/// Call this once, when a publisher or subscriber is built — not per message.
/// How many channels the registry currently holds, across every session.
///
/// Exposed so the reclamation in [`channel`] can be observed. Without a way to
/// read this, "the registry no longer grows without bound" is a claim no test
/// can make: the leak is invisible from the outside, which is why it survived
/// this long.
pub fn total_channels() -> usize {
    let bus = match BUS.read() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };
    bus.values().map(|topics| topics.len()).sum()
}

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

    // Reclaim while we already hold the write lock. A channel is removable only
    // when the registry is its sole owner and it has no subscribers: every
    // `ZPub` and `ZSub` holds its own `Arc<Channel>`, so a strong count of one
    // proves no endpoint can still reach it, and a later builder gets a fresh
    // channel that nobody is split from.
    //
    // Without this the map only ever grows. It is not merely one entry per
    // `ZContext` — `channel()` runs in *every* `ZPubBuilder::build()`, so a
    // process that publishes on dynamically-named topics accumulates an entry
    // per topic, for its lifetime, even after every publisher is dropped.
    // Sweep every session, not just this one. Scoping it to `zid` left each
    // retired session holding its final channel for the process lifetime, which
    // is the session-churn half of the leak rather than a fix for it.
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
/// `ZSub` holds one, so the registration follows the subscriber's lifetime and a
/// dropped subscriber receives no *further* deliveries.
/// It does not quiesce: a delivery already in flight on another thread runs to
/// completion against the snapshot it loaded, so the callback can still run after
/// `drop` has returned. That is memory-safe, because the snapshot owns the closure
/// and everything it captured. It is not a barrier, so do not tear down a resource
/// the callback merely observes on the strength of having dropped the subscriber.
pub struct LocalSubscription {
    channel: Arc<Channel>,
    id: u64,
}

impl Drop for LocalSubscription {
    fn drop(&mut self) {
        // Swap in a list without this entry. A publish already in flight keeps
        // running against the snapshot it loaded, exactly as it did when the
        // list was cloned out from under a lock.
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
        // The publisher already matched on TypeId, so this downcast holds. It is
        // still checked rather than assumed — an unchecked cast here would turn
        // a bookkeeping bug into undefined behaviour.
        match payload.downcast::<T>() {
            Ok(typed) => callback(typed),
            Err(_) => tracing::error!(
                "[LOCAL] payload type did not match subscriber type after a TypeId match"
            ),
        }
    });

    // Copy on write: rare, and it keeps the publish path free of locks. `rcu`
    // may run this closure more than once under contention, so it clones rather
    // than moving what it needs.
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
/// It is served only when it is the sole subscriber on the channel for that
/// type; see [`Channel::publish_owned`]. Registering one alongside a shared
/// subscriber is allowed, and simply means the owned path cannot give anything
/// away, so the publisher falls back to the shared or wire path.
pub fn subscribe_owned<T, F>(channel: Arc<Channel>, callback: F) -> LocalSubscription
where
    T: Any + Send + 'static,
    F: Fn(T) + Send + Sync + 'static,
{
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let erased: OwnedCallback = Arc::new(move |payload: Box<dyn Any + Send>| {
        // The publisher matched on TypeId, so this downcast holds. It is still
        // checked: an unchecked cast would turn a bookkeeping bug into
        // undefined behaviour.
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
