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
//! # Scope of the prototype
//!
//! | | this prototype | a production version |
//! |---|---|---|
//! | audience | every same-session subscriber registered here | same, plus the wire for remote ones, decided from the graph |
//! | type check | exact [`TypeId`] | same |
//! | mutability | shared `Arc<T>`, read-only for all receivers | move a unique payload when there is exactly one receiver |
//! | choosing the path | an explicit `with_intra_process_only()` on the publisher | inferred: use the wire only while remote subscribers exist |
//!
//! The last row is the real gap. A publisher here does not ask whether anyone
//! remote is listening; it is told. See issue #36.
//!
//! # Keying, and why a publisher resolves it once
//!
//! Channels are keyed by `(session zid, topic key expression)`. The zid is what
//! makes "same session" true rather than merely "same process" — two
//! [`ZContext`](crate::context::ZContext)s in one process open two sessions and
//! must not see each other's traffic. Both `ZPub` and `ZSub` already hold an
//! `Arc<Session>`, so this needs no plumbing through the node tree.
//!
//! A publisher takes its [`Channel`] handle when it is built and never touches
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
/// far below the depth at which the stack is in danger. See issue #40.
const MAX_DELIVERY_DEPTH: u32 = 8;

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

#[derive(Clone)]
struct Entry {
    id: u64,
    type_id: TypeId,
    callback: LocalCallback,
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

impl Channel {
    /// Deliver `payload` to every subscriber here whose type matches, returning
    /// how many were called.
    ///
    /// Callbacks are **never** invoked with the lock held. A subscriber callback
    /// commonly publishes — that is exactly what the pong side of a ping/pong
    /// does — and re-entering under its own read guard is the deadlock this
    /// workspace has already fixed three times elsewhere.
    pub fn publish<T>(&self, payload: Arc<T>) -> usize
    where
        T: Any + Send + Sync + 'static,
    {
        // Refuse to recurse without bound. A callback that publishes back onto
        // its own topic would otherwise overflow the stack; returning here turns
        // that into a dropped message and a loud log, which is recoverable and
        // greppable. See issue #40.
        let depth = DELIVERY_DEPTH.with(|d| d.get());
        if depth >= MAX_DELIVERY_DEPTH {
            tracing::error!(
                depth,
                max = MAX_DELIVERY_DEPTH,
                "intra-process delivery nested too deeply; refusing to recurse further. \
                 A subscriber callback is publishing onto a topic that reaches itself. \
                 The message was dropped."
            );
            return 0;
        }
        DELIVERY_DEPTH.with(|d| d.set(depth + 1));
        let _depth_guard = DepthGuard;

        let wanted = TypeId::of::<T>();

        let entries = self.entries.load();
        let mut matching = entries.iter().filter(|e| e.type_id == wanted);
        let Some(first) = matching.next() else {
            return 0;
        };
        let second = matching.next();

        let erased: ErasedPayload = payload;
        match second {
            // One subscriber is the overwhelmingly common case: call straight
            // through the snapshot, hand over the only reference, and touch no
            // refcount at all beyond the one the caller already holds.
            None => {
                (first.callback)(erased);
                1
            }
            // More than one receiver genuinely needs a reference each, so the
            // clones start here and not before.
            Some(second) => {
                (first.callback)(erased.clone());
                (second.callback)(erased.clone());
                let mut count = 2;
                for entry in matching {
                    (entry.callback)(erased.clone());
                    count += 1;
                }
                count
            }
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
/// dropped subscriber cannot be called back into.
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
            callback: erased.clone(),
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
