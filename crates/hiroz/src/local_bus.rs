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
//! # Keying
//!
//! Entries are keyed by `(session zid, topic key expression)`. The zid is what
//! makes "same session" true rather than merely "same process" — two
//! [`ZContext`](crate::context::ZContext)s in one process open two sessions and
//! must not see each other's traffic. Both `ZPub` and `ZSub` already hold an
//! `Arc<Session>`, so this needs no plumbing through the node tree.
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
    collections::HashMap,
    sync::{
        Arc, LazyLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use tracing::debug;
use zenoh::session::ZenohId;

/// An erased payload. Always an `Arc<T>` for the `T` named by `type_id`.
pub type ErasedPayload = Arc<dyn Any + Send + Sync>;

type LocalCallback = Arc<dyn Fn(&ErasedPayload) + Send + Sync>;

#[derive(Clone)]
struct Entry {
    id: u64,
    type_id: TypeId,
    callback: LocalCallback,
}

/// Nested rather than keyed by `(ZenohId, String)` on purpose: a tuple key would
/// force the publisher to allocate a `String` on every `publish` just to look
/// itself up. `HashMap<String, _>` can be probed with a `&str`, so this shape
/// makes the hot path allocation-free.
type Bus = HashMap<ZenohId, HashMap<String, Vec<Entry>>>;

static BUS: LazyLock<RwLock<Bus>> = LazyLock::new(|| RwLock::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Keeps a local subscription alive. Dropping it unregisters.
///
/// `ZSub` holds one, so the registration follows the subscriber's lifetime and
/// a dropped subscriber cannot be called back into.
pub struct LocalSubscription {
    zid: ZenohId,
    topic: String,
    id: u64,
}

impl Drop for LocalSubscription {
    fn drop(&mut self) {
        let mut bus = match BUS.write() {
            Ok(b) => b,
            // A poisoned registry means some callback panicked. Unregistering is
            // still the right move; leaving a dead entry would let a later
            // publish call into a dropped subscriber.
            Err(e) => e.into_inner(),
        };
        let Some(topics) = bus.get_mut(&self.zid) else {
            return;
        };
        if let Some(entries) = topics.get_mut(&self.topic) {
            entries.retain(|e| e.id != self.id);
            if entries.is_empty() {
                topics.remove(&self.topic);
            }
        }
        if topics.is_empty() {
            bus.remove(&self.zid);
        }
    }
}

/// Register a local subscriber for `topic` on `zid`, for payloads of type `T`.
pub fn subscribe<T, F>(zid: ZenohId, topic: &str, callback: F) -> LocalSubscription
where
    T: Any + Send + Sync + 'static,
    F: Fn(Arc<T>) + Send + Sync + 'static,
{
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let erased: LocalCallback = Arc::new(move |payload: &ErasedPayload| {
        // The publisher already matched on TypeId, so this downcast holds. It is
        // still checked rather than assumed — an unchecked cast here would turn
        // a bookkeeping bug into undefined behaviour.
        match payload.clone().downcast::<T>() {
            Ok(typed) => callback(typed),
            Err(_) => tracing::error!(
                "[LOCAL] payload type did not match subscriber type after a TypeId match"
            ),
        }
    });

    let mut bus = match BUS.write() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };
    bus.entry(zid)
        .or_default()
        .entry(topic.to_owned())
        .or_default()
        .push(Entry {
            id,
            type_id: TypeId::of::<T>(),
            callback: erased,
        });
    debug!("[LOCAL] subscribed id={id} topic={topic}");
    LocalSubscription {
        zid,
        topic: topic.to_owned(),
        id,
    }
}

/// Deliver `payload` to every local subscriber on `topic` whose type matches.
///
/// Returns how many callbacks were invoked. Zero means nothing local was
/// listening — the caller decides whether that is fine or whether the message
/// needs to go on the wire instead.
pub fn publish<T>(zid: ZenohId, topic: &str, payload: Arc<T>) -> usize
where
    T: Any + Send + Sync + 'static,
{
    let wanted = TypeId::of::<T>();

    // Clone the matching callbacks out and DROP the guard before invoking any of
    // them. A callback that publishes re-enters this function; holding the read
    // guard across the call is a self-deadlock waiting for a writer to queue.
    //
    // One subscriber is the overwhelmingly common case, so it is carried without
    // allocating a Vec — at 64 B payloads a heap allocation per message is a
    // measurable fraction of the whole path.
    let (single, many): (Option<LocalCallback>, Option<Vec<LocalCallback>>) = {
        let bus = match BUS.read() {
            Ok(b) => b,
            Err(e) => e.into_inner(),
        };
        let Some(entries) = bus.get(&zid).and_then(|topics| topics.get(topic)) else {
            return 0;
        };
        let mut matching = entries.iter().filter(|e| e.type_id == wanted);
        let Some(first) = matching.next() else {
            return 0;
        };
        match matching.next() {
            None => (Some(first.callback.clone()), None),
            Some(second) => {
                let mut all = vec![first.callback.clone(), second.callback.clone()];
                all.extend(matching.map(|e| e.callback.clone()));
                (None, Some(all))
            }
        }
    };

    let erased: ErasedPayload = payload;
    match (single, many) {
        (Some(cb), _) => {
            cb(&erased);
            1
        }
        (None, Some(all)) => {
            for cb in &all {
                cb(&erased);
            }
            all.len()
        }
        (None, None) => 0,
    }
}

/// How many local subscribers `topic` has on `zid`, regardless of type.
///
/// For diagnostics and tests; the publish path does not use it.
pub fn subscriber_count(zid: ZenohId, topic: &str) -> usize {
    let bus = match BUS.read() {
        Ok(b) => b,
        Err(e) => e.into_inner(),
    };
    bus.get(&zid)
        .and_then(|topics| topics.get(topic))
        .map(|e| e.len())
        .unwrap_or(0)
}
