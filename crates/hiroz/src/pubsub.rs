use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::{marker::PhantomData, sync::Arc};

use tracing::{debug, trace, warn};
use zenoh::liveliness::LivelinessToken;
use zenoh::{Result, Session, Wait, sample::Sample};

use crate::Builder;
use crate::attachment::{Attachment, GidArray};
use crate::common::DataHandler;
use crate::entity::{EndpointEntity, EndpointKind};
use crate::event::EventsManager;
use crate::graph::Graph;
use crate::impl_with_type_info;
use crate::queue::BoundedQueue;
use crate::topic_name;

use crate::msg::{SerdeCdrSerdes, ZDeserializer, ZMessage, ZSerializer};
use crate::qos::QosProfile;
use hiroz_protocol::qos::{QosDurability, QosHistory, QosReliability};
use std::sync::Mutex;
use zenoh_ext::{
    AdvancedPublisher, AdvancedPublisherBuilder, AdvancedPublisherBuilderExt, AdvancedSubscriber,
    AdvancedSubscriberBuilder, AdvancedSubscriberBuilderExt, CacheConfig, HistoryConfig,
    MissDetectionConfig, RecoveryConfig,
};

/// Sporadic heartbeat period for TransientLocal+Reliable publishers.
/// Matches rmw_zenoh_cpp's `SAMPLE_MISS_DETECTION_HEARTBEAT_PERIOD`.
const SAMPLE_MISS_HEARTBEAT_PERIOD: Duration = Duration::from_millis(500);

thread_local! {
    /// How many hiroz publish calls are currently on this thread's stack.
    ///
    /// Non-zero means: any sample this thread is *about* to deliver was produced
    /// by this same thread, synchronously, from inside `put`. See
    /// [`local_publish_active`].
    static LOCAL_PUBLISH_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII marker set for the duration of a hiroz publish.
///
/// There is no single choke point: each of `ZPub`'s four publish paths —
/// [`ZPub::publish`], [`ZPub::async_publish`], [`ZPub::publish_serialized`] and
/// [`ZPub::publish_sample`] — enters one of these itself and holds it across the
/// zenoh `put`. A fifth publish path added later must do the same, or
/// session-local delivery on that path runs inline on the publishing thread and
/// the deadlock this guard exists to prevent comes back.
///
/// Nesting is counted rather than flagged so that a publish issued from inside a
/// callback that is itself running on a thread already inside a publish restores
/// the right state on unwind.
pub(crate) struct LocalPublishGuard;

impl LocalPublishGuard {
    pub(crate) fn enter() -> Self {
        LOCAL_PUBLISH_DEPTH.with(|d| d.set(d.get() + 1));
        Self
    }
}

impl Drop for LocalPublishGuard {
    fn drop(&mut self) {
        LOCAL_PUBLISH_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Whether this thread is currently inside a hiroz publish.
///
/// This is the discriminator between the two ways a subscriber callback can be
/// reached, and it is what makes re-entrancy structurally impossible without
/// taxing the inter-process path:
///
/// * **true** — the sample is being delivered *synchronously on the publishing
///   thread*. Zenoh does this for same-session delivery (`Session::resolve_put`
///   drops the session lock and calls the local callbacks inline) and also for
///   two sessions sharing one process with a direct in-process route
///   (`send_push_consume` -> `route_data` -> the peer session's callbacks, no
///   thread hop). Running the user callback here is what allowed a callback that
///   publishes into its own topic graph to *recurse* instead of iterate. So on
///   this path hiroz enqueues and returns, exactly as zenoh's own `FifoChannel`
///   handler does, and the callback runs on the dispatcher thread.
///
/// * **false** — the sample arrived over a transport and is being delivered on a
///   zenoh RX worker (`ZRuntime::RX`, threads named `rx-N`), which is never an
///   application thread and never inside a hiroz publish. There is nothing to
///   re-enter, so the callback runs inline and the inter-process path pays only
///   this thread-local read.
///
/// Note this deliberately keys on the *publishing thread*, not on zenoh's
/// `Locality`. A `Locality::Remote`-tagged sample crossing two sessions inside
/// one process is still delivered inline on the publisher's thread, so an
/// `allowed_origin(SessionLocal)` split would miss it. The thread is the honest
/// signal; the origin is not.
fn local_publish_active() -> bool {
    LOCAL_PUBLISH_DEPTH.with(|d| d.get()) != 0
}

/// Backlog size at which an *unbounded* [`CallbackDispatcher`] first warns.
/// Doubles after each warning so a persistently slow callback does not flood the
/// log. Bounded dispatchers are excluded explicitly at the check rather than by
/// this value being out of their reach: a `KeepLast(1024)` subscriber has
/// exactly this capacity, so it would otherwise warn that its queue is lossless
/// right before dropping. Bounded dispatchers warn on drops instead.
const DISPATCH_BACKLOG_WARN_AT: usize = 1024;

/// Capacity a [`CallbackDispatcher`] must be given to be unbounded, i.e. lossless.
pub(crate) const DISPATCH_UNBOUNDED: usize = usize::MAX;

/// The dispatcher capacity implied by a subscriber's history QoS.
///
/// Matches what [`ZSubBuilder::build`] gives the queue-mode [`BoundedQueue`]:
/// `KeepLast(depth)` keeps `depth`, `KeepAll` keeps everything. A callback
/// subscriber and a queue subscriber declared with the same QoS therefore retain
/// the same number of undelivered samples, which is the only reading of ROS
/// `KEEP_LAST(depth)` that does not depend on which hiroz API the user happened
/// to pick.
///
/// The two are *not* the same expression, and the difference is confined to a
/// zero depth (the rmw spelling of "system default", which cannot be produced
/// through [`QosProfile`] but can arrive over the wire). Here it is floored at 1
/// rather than degenerating into "keep nothing"; the queue path passes the 0
/// through. Retention still agrees, because [`BoundedQueue::push`] evicts before
/// it inserts (`len >= capacity` → `pop_front`, then `push_back`), so a capacity
/// of 0 also retains exactly one sample — see `queue::tests::
/// zero_capacity_retains_one_sample`. What differs is bookkeeping, not data: at
/// capacity 0 every push reports a drop, including the first one into an empty
/// queue.
pub(crate) fn dispatch_capacity(qos: &hiroz_protocol::qos::QosProfile) -> usize {
    match qos.history {
        QosHistory::KeepLast(depth) => depth.max(1),
        QosHistory::KeepAll => DISPATCH_UNBOUNDED,
    }
}

struct DispatchState {
    /// Samples awaiting delivery, in the order zenoh decided to deliver them.
    pending: std::collections::VecDeque<Sample>,
    /// Set by [`CallbackDispatcher::drop`]: stop delivering and exit. Queued
    /// but undelivered samples are discarded -- see [`DispatchQueue::dequeue`]
    /// for why teardown does not drain them.
    closed: bool,
    /// Next backlog length that triggers a warning. Unbounded queues only.
    warn_at: usize,
    /// Samples discarded because the queue was at capacity.
    dropped: u64,
    /// Next `dropped` total that triggers a warning.
    warn_dropped_at: u64,
}

struct DispatchQueue {
    state: Mutex<DispatchState>,
    ready: std::sync::Condvar,
    topic: String,
    /// Maximum number of undelivered samples retained. [`DISPATCH_UNBOUNDED`]
    /// means lossless; anything smaller drops the *oldest* on overflow, exactly
    /// as [`BoundedQueue::push`] does. See [`CallbackDispatcher`]'s
    /// "Backpressure" section for which path gets which.
    capacity: usize,
}

impl DispatchQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, DispatchState> {
        // A panicking user callback must not wedge the subscriber: the queue
        // holds no invariant that a partial mutation could break.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The shim callback handed to zenoh. May run with zenoh-ext's state mutex
    /// held (advanced path) or on the publishing thread inside `put` (local
    /// path), so it must not do anything that could publish or block.
    fn enqueue(&self, sample: Sample) {
        let (backlog, dropped) = {
            let mut state = self.lock();
            if state.closed {
                return;
            }

            // Drop the oldest, never the newest and never the incoming sample:
            // the same choice `BoundedQueue::push` makes, and the same one ROS
            // `KEEP_LAST(depth)` describes. A bounded queue that *blocked* here
            // would re-create the original deadlock — see the type's docs.
            let dropped = if state.pending.len() >= self.capacity {
                state.pending.pop_front();
                state.dropped = state.dropped.saturating_add(1);
                if state.dropped >= state.warn_dropped_at {
                    state.warn_dropped_at = state.dropped.saturating_mul(2);
                    Some(state.dropped)
                } else {
                    None
                }
            } else {
                None
            };

            state.pending.push_back(sample);
            let len = state.pending.len();
            // Unbounded queues only. A bounded queue *can* reach
            // `DISPATCH_BACKLOG_WARN_AT` — nothing stops a subscriber declaring
            // `KeepLast(1024)` or deeper — and it would then log that the queue
            // is lossless and merely costs memory, which is the opposite of
            // what a bounded queue does. Bounded queues report drops instead;
            // that warning is immediately below and is the accurate one.
            let backlog = if self.capacity == DISPATCH_UNBOUNDED && len >= state.warn_at {
                state.warn_at = len.saturating_mul(2);
                Some(len)
            } else {
                None
            };
            (backlog, dropped)
        };
        self.ready.notify_one();
        if let Some(len) = backlog {
            warn!(
                topic = %self.topic,
                backlog = len,
                "subscriber delivery backlog is growing; the callback is slower than the \
                 publish rate. This queue is lossless, so the backlog costs memory."
            );
        }
        if let Some(total) = dropped {
            warn!(
                topic = %self.topic,
                dropped = total,
                capacity = self.capacity,
                "subscriber delivery queue is full; dropping the oldest undelivered sample. \
                 The callback is slower than the publish rate — raise the history depth or \
                 make the callback cheaper."
            );
        }
    }

    /// Blocks until a sample is available, or until the queue is closed
    /// (returns `None`, ending the drain loop).
    ///
    /// `closed` is checked **before** `pending`, and that ordering is the
    /// difference between a bounded and an unbounded teardown. Draining the
    /// backlog first meant `drop(subscriber)` ran a user callback for every
    /// queued sample before returning: on the unbounded (TransientLocal) path
    /// that is `backlog × callback_duration` with no ceiling -- a 1 kHz
    /// publisher against a 5 ms callback leaves ~30 000 samples queued after
    /// 30 s, so the drop blocks for minutes, silently. It could also block
    /// *forever*, if a callback waits on anything the dropping thread must
    /// supply.
    ///
    /// Dropping a subscriber means "stop delivering to me", so undelivered
    /// samples are discarded rather than forced through a callback the caller
    /// has already disposed of -- the same thing destroying an rclcpp
    /// subscription does. Teardown now costs at most one in-flight callback.
    fn dequeue(&self) -> Option<Sample> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(entry) = state.pending.pop_front() {
                return Some(entry);
            }
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Runs a subscriber's user callback on a dedicated thread, fed by a FIFO queue.
///
/// This is hiroz's equivalent of zenoh's `FifoChannel` handler, and of
/// zenoh-python's `Callback(indirect=True)` — which is what zenoh-python
/// installs by default when you hand `declare_subscriber` a plain callable. The
/// delivery thread enqueues and returns; user code runs here.
///
/// Two independent reasons a sample takes this path:
///
/// 1. **It was published by this same thread** ([`local_publish_active`]) — the
///    session-local case. Delivering inline would let a callback that publishes
///    into its own topic graph recurse instead of iterate. Enqueuing makes the
///    feedback loop *iterative*, which is why hiroz no longer needs a
///    re-entrancy depth cap: a callback simply cannot be reached from inside
///    `put`.
/// 2. **The subscriber is a zenoh-ext `AdvancedSubscriber`**, which invokes the
///    sample callback while holding the `std::sync::Mutex` that guards its
///    reordering state — and it *has* to: `handle_sample` interleaves
///    `callback.call(sample)` with mutation of `last_delivered` /
///    `pending_samples` (see `deliver_and_flush`, which calls the callback,
///    records the delivered sequence number, then drains newly-contiguous
///    pending samples calling the callback again). The guard cannot simply be
///    dropped before the call the way `Session::resolve_put` does, because the
///    lock protects exactly the state the delivery loop is walking.
///
/// A sample that is neither — i.e. one that arrived over a transport, on a
/// zenoh RX worker, for a plain subscriber — is delivered inline and never
/// touches this queue. That is deliberate: the RX thread is not an application
/// thread and holds no hiroz lock, so there is nothing to re-enter, and the
/// inter-process path must not pay for a hazard it does not have.
///
/// # Ordering
///
/// One producer path, one FIFO queue, one drain thread, so the user observes
/// exactly the order zenoh decided to deliver in. On the advanced path the shim
/// enqueues from inside `handle_sample`, i.e. under zenoh-ext's state mutex, so
/// enqueue order includes the several back-to-back deliveries a single
/// `deliver_and_flush` performs when it drains pending samples; the reordering
/// and recovery guarantees `AdvancedSubscriber` exists to provide are
/// unaffected, only the thread the callback runs on changes.
///
/// The one ordering property that is *not* preserved is between the two paths:
/// a plain subscriber that receives both local and remote publications on the
/// same topic now runs the local ones on this thread and the remote ones on an
/// RX thread, so their relative order is no longer guaranteed and the two can
/// overlap. Neither ROS 2 nor zenoh guarantees ordering across distinct
/// publishers, and a plain zenoh subscriber can already be invoked concurrently
/// from several RX workers, so this weakens no guarantee that was actually
/// being offered — but it is a real change and is called out here rather than
/// discovered later.
///
/// # Backpressure
///
/// The queue **never blocks its producer**. That is not a tuning choice: a
/// bounded queue that blocked would re-create the original deadlock in a new
/// form on both paths. On the advanced path the blocked thread sits inside
/// `sub_callback` holding zenoh-ext's state mutex; on the local path it sits
/// inside the user's own `publish()`, and in a closed feedback loop the drain
/// thread it waits on is the very thread that must publish for the queue to
/// drain. zenoh's own `FifoChannel` is bounded *and* blocking and documents
/// exactly this cost ("a slow subscriber could block the underlying Zenoh
/// thread", `fifo.rs`); hiroz does not adopt that failure mode.
///
/// What remains is a choice between unbounded (lossless, can grow without
/// limit) and bounded drop-oldest (lossy, constant memory). **The two paths get
/// different answers, because they make different promises:**
///
/// * **Plain path — bounded, drop-oldest, capacity from the subscriber's
///   history QoS** ([`dispatch_capacity`]). A plain subscriber is `Volatile`
///   with `KEEP_LAST(depth)`: it already promises only the last `depth`
///   undelivered samples, and hiroz's own queue-mode path enforces exactly that
///   with [`BoundedQueue`], from the same expression. A callback subscriber that
///   instead retained *every* undelivered sample would honour a QoS stricter
///   than the one it was declared with, and would let a tight local publish loop
///   with a slow callback grow the process until it died — a failure mode with
///   no upside, since the samples being retained are ones the declared QoS says
///   may be discarded. Drop-oldest also preserves the relative order of what
///   survives, so the ordering objection that applies to the advanced path does
///   not apply here.
///
/// * **Advanced path — same bound, and the same expression.** This matches
///   `rmw_zenoh_cpp`: `SubscriptionData::add_new_message` drops the oldest once
///   `message_queue_.size() >= adapted_qos_profile.depth`, for every arriving
///   sample, with **no `TransientLocal` exemption** — the check reads the
///   history policy only. Its advanced-subscriber cache is sized the same way
///   (`adv_sub_opts.history->max_samples = qos_.depth`).
///
///   An earlier revision left this path unbounded on *every* profile, reasoning
///   that dropping discards what miss-detection recovered. That is why `KeepAll`
///   maps to [`DISPATCH_UNBOUNDED`] — but applied to a declared
///   `KeepLast(depth)` it ignores the QoS, and since [`Self::always_shim`]
///   enqueues remote samples too, it also traded zenoh's transport backpressure
///   for unbounded growth.
///
///   Both implementations drop **silently** as far as the ROS event API is
///   concerned: upstream's `MESSAGE_LOST` is raised from *sequence-number gaps*
///   among arriving messages, which a depth-drop cannot produce. The escalating
///   `warn!` below is strictly more visible than upstream's debug log.
///
/// Two consequences are worth stating rather than discovering.
///
/// **Which samples the bound applies to differs by path.** On the plain path
/// only *locally published* samples pass through this queue; a sample arriving
/// over a transport is delivered inline on an RX worker and is backpressured by
/// zenoh instead. On the advanced path [`Self::always_shim`] enqueues
/// everything, remote included. So a slow callback loses local samples and
/// stalls remote ones on the plain path, and loses either on the advanced one.
/// That asymmetry is inherent to delivering the two on different threads — which
/// is what makes re-entrancy impossible without taxing the inter-process path.
///
/// **On the advanced path a `KeepAll` subscriber has no backpressure at all.**
/// Because the queue is genuinely unbounded there and remote samples go into it,
/// a publisher outpacing the callback grows `pending` without limit — the
/// escalating backlog warning is the only signal. That is the declared QoS being
/// honoured rather than a defect, but `KeepAll` on a slow callback is an
/// unbounded memory commitment and should be chosen deliberately.
pub struct CallbackDispatcher {
    queue: Arc<DispatchQueue>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CallbackDispatcher {
    /// Spawns the drain thread.
    ///
    /// `handler` is shared: the drain thread always calls it, and the *plain*
    /// path's shim additionally calls it inline for samples that did not
    /// originate on this thread. Use [`Self::always_shim`] or
    /// [`Self::local_only_shim`] to obtain the callback to hand to zenoh.
    ///
    /// `capacity` is the number of undelivered samples retained before the
    /// oldest is dropped. **All four construction sites pass
    /// [`dispatch_capacity`]** — the plain and advanced arms of both the typed
    /// builder (this module) and the FFI raw subscriber (`node.rs`) — so a
    /// callback subscriber retains what its history QoS declares regardless of
    /// which path it takes. See the "Backpressure" section.
    ///
    /// The FFI advanced arm was missed when the other three were converted. That
    /// arm *is* compiled on the PR gate, but never linted and never tested, and
    /// its re-entrancy detector had been deleted — so a wrong constant there is
    /// neither a compile error nor a lint, and nothing was left to catch it
    /// (#291). Building is not testing.
    pub(crate) fn spawn<F>(topic: &str, handler: Arc<F>, capacity: usize) -> Result<Self>
    where
        F: Fn(Sample) + Send + Sync + 'static,
    {
        let queue = Arc::new(DispatchQueue {
            state: Mutex::new(DispatchState {
                pending: std::collections::VecDeque::new(),
                closed: false,
                warn_at: DISPATCH_BACKLOG_WARN_AT,
                dropped: 0,
                warn_dropped_at: 1,
            }),
            ready: std::sync::Condvar::new(),
            topic: topic.to_string(),
            capacity,
        });

        let drain_queue = queue.clone();
        let drain_topic = topic.to_string();
        let thread = std::thread::Builder::new()
            .name("hiroz-sub-drain".to_string())
            .spawn(move || {
                while let Some(sample) = drain_queue.dequeue() {
                    // A panicking user callback must not kill the drain thread —
                    // that would silently stop all further delivery.
                    //
                    // This holds only where panics unwind. Under `panic = "abort"`
                    // — which this workspace's `[profile.opt]` sets — the panic
                    // aborts the process before `catch_unwind` can return `Err`,
                    // so neither the recovery below nor the log line happens. The
                    // guard is therefore effective for dev, test and `release`
                    // builds (including everything CI runs) and inert for `opt`.
                    // That is a deliberate consequence of choosing `abort` for
                    // that profile, not an oversight here: a build that opts into
                    // aborting on panic has opted out of surviving one.
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (*handler)(sample)))
                        .is_err()
                    {
                        tracing::error!(
                            topic = %drain_topic,
                            "subscriber callback panicked; dropping the sample and continuing"
                        );
                    }
                }
            })
            .map_err(|e| {
                zenoh::Error::from(format!("failed to spawn subscriber delivery thread: {e}"))
            })?;

        Ok(Self {
            queue,
            thread: Some(thread),
        })
    }

    /// A shim that enqueues **every** sample. Used for the advanced path, where
    /// zenoh-ext holds its state mutex across the callback regardless of where
    /// the sample came from.
    pub(crate) fn always_shim(&self) -> impl Fn(Sample) + Send + Sync + 'static {
        let queue = self.queue.clone();
        move |sample: Sample| queue.enqueue(sample)
    }

    /// A shim that enqueues only samples produced by the delivering thread
    /// itself, and invokes `handler` inline otherwise. Used for the plain path.
    ///
    /// The inline branch is the inter-process hot path: a sample that arrived
    /// over a transport is delivered on a zenoh RX worker, which is never inside
    /// a hiroz publish, so [`local_publish_active`] is false and the only cost
    /// added to that path is this one thread-local read.
    pub(crate) fn local_only_shim<F>(
        &self,
        handler: Arc<F>,
    ) -> impl Fn(Sample) + Send + Sync + 'static
    where
        F: Fn(Sample) + Send + Sync + 'static,
    {
        let queue = self.queue.clone();
        move |sample: Sample| {
            if local_publish_active() {
                queue.enqueue(sample);
            } else {
                handler(sample);
            }
        }
    }
}

impl Drop for CallbackDispatcher {
    fn drop(&mut self) {
        self.queue.lock().closed = true;
        self.queue.ready.notify_all();

        let Some(thread) = self.thread.take() else {
            return;
        };
        if thread.thread().id() == std::thread::current().id() {
            // The subscriber was dropped from inside its own callback. Joining
            // ourselves would deadlock; the thread will observe `closed` and
            // exit once this callback returns.
            return;
        }
        if thread.join().is_err() {
            warn!(
                topic = %self.queue.topic,
                "subscriber delivery thread terminated abnormally"
            );
        }
    }
}

/// Whether a QoS profile needs a zenoh-ext advanced subscriber/publisher.
///
/// The advanced entities exist for history replay, sample-miss detection and
/// recovery, and publisher/subscriber detection — all of which
/// [`apply_transient_local_sub`] and [`apply_transient_local_pub`] configure only
/// for `TransientLocal` durability. For the ROS 2 default (`Volatile`) an
/// unconfigured `AdvancedSubscriber` adds no protocol behaviour, but it *does*
/// run the user callback while holding a non-reentrant `std::sync::Mutex`
/// (`advanced_subscriber.rs`: `sub_callback` takes `zlock!(statesref)` and
/// `handle_sample` calls the callback under that guard). Combined with zenoh's
/// synchronous local delivery, that turns any publish from inside a callback
/// into a self-deadlock. So only pay for it when the QoS actually asks for it.
pub(crate) fn qos_needs_advanced(qos: &hiroz_protocol::qos::QosProfile) -> bool {
    matches!(qos.durability, QosDurability::TransientLocal)
}

/// The declared zenoh subscriber backing a [`ZSub`].
///
/// hiroz declares a plain subscriber unless the QoS profile actually configures
/// advanced features — see [`qos_needs_advanced`].
pub enum SubscriberHandle {
    /// A plain zenoh subscriber (the `Volatile` default).
    ///
    /// Samples that arrived over a transport run inline on the zenoh RX worker.
    /// Samples published by the delivering thread itself are handed to the
    /// dispatcher — see [`CallbackDispatcher`]. `dispatcher` is `None` for
    /// queue-mode subscribers, which run no user code on the delivery thread and
    /// so need no handoff.
    Plain {
        subscriber: zenoh::pubsub::Subscriber<()>,
        dispatcher: Option<CallbackDispatcher>,
    },
    /// A zenoh-ext advanced subscriber, used for `TransientLocal` durability.
    ///
    /// `dispatcher` is `Some` only when the handler runs user code. zenoh-ext
    /// holds its state lock across the callback, so user code must be moved off
    /// that thread — but a queue-mode handler only enqueues into a
    /// [`BoundedQueue`] and re-enters nothing, so it can run under that lock
    /// safely and needs no thread of its own.
    Advanced {
        /// Boxed because it is several times larger than the plain variant.
        ///
        /// Declared first so it drops first: undeclaring the subscriber stops
        /// new samples from being enqueued before the dispatcher drains and
        /// joins.
        subscriber: Box<AdvancedSubscriber<()>>,
        dispatcher: Option<CallbackDispatcher>,
    },
}

impl std::fmt::Debug for SubscriberHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain { .. } => f.write_str("SubscriberHandle::Plain"),
            Self::Advanced { .. } => f.write_str("SubscriberHandle::Advanced"),
        }
    }
}

/// Query timeout for TransientLocal subscribers' initial history fetch.
/// Matches rmw_zenoh_cpp's `query_timeout_ms = u64::max()` literally
/// (`Duration::from_millis(u64::MAX)`, not `Duration::MAX`, to avoid any
/// truncation surprises inside zenoh-ext's internal `as_millis()` paths).
const TRANSIENT_LOCAL_QUERY_TIMEOUT: Duration = Duration::from_millis(u64::MAX);

fn cache_depth_from_history(history: QosHistory) -> usize {
    // Mirrors rmw_zenoh_cpp's `QoS::best_available_qos` (`qos.cpp:107`):
    // a zero-valued depth (the rmw representation of "unspecified" or
    // KEEP_ALL) is rewritten to `RMW_ZENOH_DEFAULT_HISTORY_DEPTH` (42)
    // before being passed to `cache.max_samples`. The cap value lives
    // alongside hiroz's other QoS constants — see `crate::qos`.
    match history {
        QosHistory::KeepLast(d) if d > 0 => d,
        QosHistory::KeepLast(_) => crate::qos::KEEP_ALL_CACHE_DEPTH,
        QosHistory::KeepAll => crate::qos::KEEP_ALL_CACHE_DEPTH,
    }
}

/// Apply hiroz TransientLocal QoS to an `AdvancedPublisherBuilder`.
///
/// Mirrors rmw_zenoh_cpp's `rmw_publisher_data.cpp`: enables
/// `publisher_detection` + `cache`, and `sample_miss_detection` when
/// `Reliable`. No-op for `Volatile`.
pub(crate) fn apply_transient_local_pub<'a, 'b, 'c>(
    mut builder: AdvancedPublisherBuilder<'a, 'b, 'c>,
    qos: &hiroz_protocol::qos::QosProfile,
) -> AdvancedPublisherBuilder<'a, 'b, 'c> {
    if !matches!(qos.durability, QosDurability::TransientLocal) {
        return builder;
    }
    let depth = cache_depth_from_history(qos.history);
    builder = builder
        .publisher_detection()
        .cache(CacheConfig::default().max_samples(depth));
    if matches!(qos.reliability, QosReliability::Reliable) {
        builder = builder.sample_miss_detection(
            MissDetectionConfig::default().sporadic_heartbeat(SAMPLE_MISS_HEARTBEAT_PERIOD),
        );
    }
    builder
}

/// Apply hiroz TransientLocal QoS to an `AdvancedSubscriberBuilder`.
///
/// Mirrors rmw_zenoh_cpp's `rmw_subscription_data.cpp`: enables history
/// query with late-publisher detection, `subscriber_detection`, a
/// `u64::MAX`-millisecond query timeout, and heartbeat recovery when
/// `Reliable`. No-op for `Volatile`.
pub(crate) fn apply_transient_local_sub<'a, 'b, 'c, H>(
    mut builder: AdvancedSubscriberBuilder<'a, 'b, 'c, H>,
    qos: &hiroz_protocol::qos::QosProfile,
) -> AdvancedSubscriberBuilder<'a, 'b, 'c, H> {
    if !matches!(qos.durability, QosDurability::TransientLocal) {
        return builder;
    }
    let depth = cache_depth_from_history(qos.history);
    builder = builder
        .history(
            HistoryConfig::default()
                .detect_late_publishers()
                .max_samples(depth),
        )
        .query_timeout(TRANSIENT_LOCAL_QUERY_TIMEOUT)
        .subscriber_detection();
    if matches!(qos.reliability, QosReliability::Reliable) {
        builder = builder.recovery(RecoveryConfig::default().heartbeat());
    }
    builder
}

/// A typed ROS 2-style publisher. Send messages with [`publish`](ZPub::publish)
/// (synchronous) or [`async_publish`](ZPub::async_publish) (async).
///
/// Create a publisher via [`ZNode::create_pub`](crate::node::ZNode::create_pub).
pub struct ZPub<T: ZMessage, S: ZSerializer> {
    pub entity: EndpointEntity,
    // TODO: replace this with the sample sn
    sn: AtomicUsize,
    // TODO: replace this with zenoh's global entity id
    gid: GidArray,
    inner: AdvancedPublisher<'static>,
    _lv_token: LivelinessToken,
    with_attachment: bool,
    clock: crate::time::ZClock,
    events_mgr: Arc<Mutex<EventsManager>>,
    shm_config: Option<Arc<crate::shm::ShmConfig>>,
    /// Schema for dynamic message publishing.
    pub dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Cached Zenoh encoding for this publisher (performance optimization).
    /// If set, this encoding will be used for all published messages.
    encoding: Option<Arc<zenoh::bytes::Encoding>>,
    graph: Arc<Graph>,
    _phantom_data: PhantomData<(T, S)>,
}

impl<T: ZMessage, S: ZSerializer> std::fmt::Debug for ZPub<T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZPub")
            .field("entity", &self.entity)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ZPubBuilder<T, S = SerdeCdrSerdes<T>> {
    pub(crate) entity: EndpointEntity,
    pub(crate) session: Arc<Session>,
    pub(crate) graph: Arc<Graph>,
    pub(crate) clock: crate::time::ZClock,
    pub(crate) with_attachment: bool,
    pub(crate) shm_config: Option<Arc<crate::shm::ShmConfig>>,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    /// Schema for dynamic message publishing.
    /// When set, the schema will be registered with the type description service.
    pub(crate) dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Encoding format for this publisher.
    /// If set, all published messages will use this encoding.
    pub(crate) encoding: Option<crate::encoding::Encoding>,
    pub(crate) _phantom_data: PhantomData<(T, S)>,
}

impl_with_type_info!(ZPubBuilder<T, S>);
impl_with_type_info!(ZSubBuilder<T, S>);

impl<T, S> ZPubBuilder<T, S> {
    pub fn with_qos(mut self, qos: QosProfile) -> Self {
        self.entity.qos = qos.to_protocol_qos();
        self
    }

    pub fn with_attachment(mut self, with_attachment: bool) -> Self {
        self.with_attachment = with_attachment;
        self
    }

    /// Override SHM configuration for this publisher only.
    ///
    /// This overrides any SHM configuration inherited from the node or context.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::shm::{ShmConfig, ShmProviderBuilder};
    /// use hiroz::Builder;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// let provider = Arc::new(ShmProviderBuilder::new(20 * 1024 * 1024).build()?);
    /// let config = ShmConfig::new(provider).with_threshold(5_000);
    ///
    /// let publisher = node.create_pub::<hiroz_msgs::std_msgs::String>("topic")
    ///     .with_shm_config(config)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_config(mut self, config: crate::shm::ShmConfig) -> Self {
        self.shm_config = Some(Arc::new(config));
        self
    }

    /// Disable SHM for this publisher.
    ///
    /// Even if SHM is enabled at the node or context level, this publisher
    /// will not use shared memory.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().with_shm_enabled()?.build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// // Context has SHM enabled, but disable for this publisher
    /// let publisher = node.create_pub::<hiroz_msgs::std_msgs::String>("small_messages")
    ///     .without_shm()
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn without_shm(mut self) -> Self {
        self.shm_config = None;
        self
    }

    pub fn with_serdes<S2>(self) -> ZPubBuilder<T, S2> {
        ZPubBuilder {
            entity: self.entity,
            session: self.session,
            graph: self.graph,
            clock: self.clock,
            with_attachment: self.with_attachment,
            shm_config: self.shm_config,
            keyexpr_format: self.keyexpr_format.clone(),
            dyn_schema: self.dyn_schema,
            encoding: self.encoding,
            _phantom_data: PhantomData,
        }
    }

    /// Set the encoding format for published messages.
    ///
    /// This encoding will be transmitted with each message, allowing subscribers
    /// to determine the serialization format at runtime.
    ///
    /// # Performance
    ///
    /// The Zenoh encoding is cached during `build()` to avoid repeated conversion
    /// overhead on every `publish()` call.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use hiroz::encoding::Encoding;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// // Publish with Protobuf encoding
    /// let publisher = node.create_pub::<hiroz_msgs::geometry_msgs::Point>("/topic")
    ///     .with_encoding(Encoding::protobuf().with_schema("geometry_msgs/msg/Point"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_encoding(mut self, encoding: crate::encoding::Encoding) -> Self {
        self.encoding = Some(encoding);
        self
    }

    /// Set the dynamic message schema for runtime-typed publishers.
    ///
    /// When a schema is set and the node has a type description service enabled,
    /// the schema will be automatically registered with the service during build.
    /// This allows other nodes to query for this type's description.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let publisher = node
    ///     .create_pub_impl::<DynamicMessage>("topic", None)
    ///     .with_serdes::<DynamicSerdeCdrSerdes>()
    ///     .with_dyn_schema(schema)
    ///     .build()?;
    /// ```
    pub fn with_dyn_schema(mut self, schema: Arc<crate::dynamic::schema::MessageSchema>) -> Self {
        // Only compute and set type_info if it hasn't been set already.
        // Typed publishers (create_pub) already have type_info set via T::type_info();
        // don't overwrite it with the schema-derived value.
        if self.entity.type_info.is_none() {
            self.entity.type_info = Some(crate::dynamic::schema_type_info(&schema));
        }

        self.dyn_schema = Some(schema);
        self
    }
}

impl<T, S> Builder for ZPubBuilder<T, S>
where
    T: ZMessage + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    type Output = ZPub<T, S>;

    #[tracing::instrument(name = "pub_build", skip(self), fields(
        topic = %self.entity.topic,
        qos_reliability = ?self.entity.qos.reliability,
        qos_durability = ?self.entity.qos.durability
    ))]
    fn build(mut self) -> Result<Self::Output> {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from("publisher build requires node identity"));
        };
        // Qualify the topic name according to ROS 2 rules
        let qualified_topic =
            topic_name::qualify_topic_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        self.entity.topic = qualified_topic.clone();
        debug!("[PUB] Qualified topic: {}", qualified_topic);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone(); // Deref and clone the KeyExpr
        debug!("[PUB] Key expression: {}", key_expr);

        // Map QoS to Zenoh publisher settings
        let mut pub_builder = self.session.declare_publisher(key_expr);

        // Map reliability: Reliable uses Block, BestEffort uses Drop
        match self.entity.qos.reliability {
            QosReliability::Reliable => {
                pub_builder = pub_builder.congestion_control(zenoh::qos::CongestionControl::Block);
                debug!("[PUB] QoS: Reliable (Block)");
            }
            QosReliability::BestEffort => {
                pub_builder = pub_builder.congestion_control(zenoh::qos::CongestionControl::Drop);
                debug!("[PUB] QoS: BestEffort (Drop)");
            }
        }

        // Build an AdvancedPublisher and apply TransientLocal config if needed.
        let pub_builder = pub_builder.advanced();
        debug!(
            "[PUB] Durability: {:?}, history: {:?}",
            self.entity.qos.durability, self.entity.qos.history
        );
        let pub_builder = apply_transient_local_pub(pub_builder, &self.entity.qos);
        let inner = pub_builder.wait()?;
        debug!("[PUB] Publisher ready: topic={}", self.entity.topic);

        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;
        let gid = crate::entity::endpoint_gid(&self.entity)
            .expect("local endpoint always has node identity");

        // Cache the Zenoh encoding if specified (performance optimization)
        let encoding = self.encoding.map(|enc| Arc::new(enc.to_zenoh_encoding()));

        if let Some(ref enc) = encoding {
            debug!("[PUB] Using encoding: {}", enc);
        }

        Ok(ZPub {
            entity: self.entity,
            sn: AtomicUsize::new(0),
            inner,
            _lv_token: lv_token,
            gid,
            clock: self.clock,
            events_mgr: Arc::new(Mutex::new(EventsManager::new(gid))),
            with_attachment: self.with_attachment,
            shm_config: self.shm_config,
            dyn_schema: self.dyn_schema,
            encoding,
            graph: self.graph,
            _phantom_data: Default::default(),
        })
    }
}

impl<T, S> ZPub<T, S>
where
    T: ZMessage + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    /// Wait until at least `count` subscribers are matched on this publisher's topic,
    /// or until `timeout` elapses.
    ///
    /// Returns `true` if the required number of subscribers appeared within the
    /// timeout, `false` otherwise.
    ///
    /// This mirrors rclcpp's `rcl_wait_for_subscribers()` pattern: the publisher
    /// registers a graph-change notification *before* sampling the subscriber count,
    /// so no arrival is missed between the check and the wait.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Ensure at least one subscriber is ready before publishing.
    /// assert!(publisher.wait_for_subscription(1, Duration::from_secs(5)).await);
    /// ```
    pub async fn wait_for_subscription(&self, count: usize, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Arm the notification *before* reading the count to avoid a TOCTOU
            // race where a subscriber arrives between the count check and the await.
            let notified = self.graph.change_notify.notified();
            tokio::pin!(notified);

            let n = self
                .graph
                .get_entities_by_topic(EndpointKind::Subscription, &self.entity.topic)
                .len();
            if n >= count {
                return true;
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }

            // Sleep until either a graph change fires or the deadline passes.
            if tokio::time::timeout(remaining, &mut notified)
                .await
                .is_err()
            {
                // Timeout — do one final check in case a late notification was missed.
                return self
                    .graph
                    .get_entities_by_topic(EndpointKind::Subscription, &self.entity.topic)
                    .len()
                    >= count;
            }
        }
    }

    fn new_attachment(&self) -> Attachment {
        let sn = self.sn.fetch_add(1, Ordering::Relaxed);
        trace!(
            "[PUB] Creating attachment: sn={}, gid={:02x?}",
            sn,
            &self.gid[..4]
        );
        Attachment::with_clock(sn as _, self.gid, &self.clock)
    }

    /// Serialize and publish `msg` on the topic. Blocks until the put completes.
    ///
    /// Use [`async_publish`](ZPub::async_publish) when calling from async code to
    /// avoid blocking the executor.
    #[tracing::instrument(name = "publish", skip(self, msg), fields(
        topic = %self.entity.topic,
        sn = self.sn.load(Ordering::Acquire),
        payload_len = tracing::field::Empty,
        used_shm = tracing::field::Empty
    ))]
    pub fn publish(&self, msg: &T) -> Result<()> {
        use zenoh_buffers::buffer::Buffer;

        // Try direct SHM serialization if configured
        let (zbuf, actual_size) = if let Some(ref shm_cfg) = self.shm_config {
            let estimated_size = msg.estimated_serialized_size();

            // Only use SHM if estimated size meets threshold
            if estimated_size >= shm_cfg.threshold() {
                match S::serialize_to_shm(msg, estimated_size, shm_cfg.provider()) {
                    Ok((zbuf, actual_size)) => {
                        tracing::Span::current().record("used_shm", true);
                        debug!(
                            "[PUB] Serialized {}B directly to SHM (estimated: {}B)",
                            actual_size, estimated_size
                        );
                        (zbuf, actual_size)
                    }
                    Err(e) => {
                        tracing::Span::current().record("used_shm", false);
                        warn!(
                            "[PUB] Direct SHM serialization failed: {}. Using regular memory",
                            e
                        );
                        let zbuf = S::serialize_to_zbuf(msg);
                        let size = zbuf.len();
                        (zbuf, size)
                    }
                }
            } else {
                tracing::Span::current().record("used_shm", false);
                trace!(
                    "[PUB] Estimated size {}B < threshold {}B, using regular memory",
                    estimated_size,
                    shm_cfg.threshold()
                );
                let zbuf = S::serialize_to_zbuf(msg);
                let size = zbuf.len();
                (zbuf, size)
            }
        } else {
            tracing::Span::current().record("used_shm", false);
            let zbuf = S::serialize_to_zbuf(msg);
            let size = zbuf.len();
            (zbuf, size)
        };

        tracing::Span::current().record("payload_len", actual_size);

        let zbytes = zenoh::bytes::ZBytes::from(zbuf);

        let mut put_builder = self.inner.put(zbytes);

        // Set encoding if configured (performance: uses cached Arc to avoid clone overhead)
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            let att = self.new_attachment();
            let sn = att.sequence_number;
            put_builder = put_builder.attachment(att);
            trace!("[PUB] Attached sn={}", sn);
        }

        let _local = LocalPublishGuard::enter();
        put_builder.wait()
    }

    /// Serialize and publish `msg` on the topic. Yields to the async executor
    /// while the put is in progress, making this safe to call from within
    /// a Tokio task without blocking the thread.
    pub async fn async_publish(&self, msg: &T) -> Result<()> {
        // Try direct SHM serialization if configured
        let zbuf = if let Some(ref shm_cfg) = self.shm_config {
            let estimated_size = msg.estimated_serialized_size();

            if estimated_size >= shm_cfg.threshold() {
                match S::serialize_to_shm(msg, estimated_size, shm_cfg.provider()) {
                    Ok((zbuf, _actual_size)) => zbuf,
                    Err(_) => S::serialize_to_zbuf(msg),
                }
            } else {
                S::serialize_to_zbuf(msg)
            }
        } else {
            S::serialize_to_zbuf(msg)
        };

        let zbytes = zenoh::bytes::ZBytes::from(zbuf);
        let mut put_builder = self.inner.put(zbytes);

        // Set encoding if configured
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            put_builder = put_builder.attachment(self.new_attachment());
        }
        // The guard must cover the delivery, and delivery happens in
        // `into_future`, not at the await: zenoh's `PublicationBuilder` future is
        // `std::future::ready(self.wait())`, so the put — including any inline
        // local-subscriber dispatch — completes before a future exists to poll.
        // Scoping the guard here rather than across the `.await` also keeps this
        // future `Send`, which a thread-local guard held across an await point
        // would not.
        let fut = {
            let _local = LocalPublishGuard::enter();
            std::future::IntoFuture::into_future(put_builder)
        };
        fut.await
    }

    /// Publish pre-serialized data directly
    ///
    /// Accepts any type that implements `Into<ZBytes>`:
    /// - `&[u8]` - byte slice
    /// - `Vec<u8>` - owned bytes
    /// - `ZBuf` - zero-copy buffer (preferred for performance)
    /// - `ZBytes` - zenoh bytes
    pub fn publish_serialized(&self, data: impl Into<zenoh::bytes::ZBytes>) -> Result<()> {
        let mut put_builder = self.inner.put(data);

        // Set encoding if configured
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            put_builder = put_builder.attachment(self.new_attachment());
        }
        let _local = LocalPublishGuard::enter();
        put_builder.wait()
    }

    pub fn publish_sample(&self, msg: &Sample) -> Result<()> {
        let payload = msg.payload().to_bytes();
        // NOTE: pass by reference to avoid copy
        let mut put_builder = self.inner.put(&payload);

        // Set encoding if configured
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            put_builder = put_builder.attachment(self.new_attachment());
        }
        let _local = LocalPublishGuard::enter();
        put_builder.wait()
    }

    pub fn events_mgr(&self) -> &Arc<Mutex<EventsManager>> {
        &self.events_mgr
    }

    /// Get a reference to the endpoint entity for this publisher.
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
    }
}

// Specialized implementation for DynamicMessage publisher
impl ZPub<crate::dynamic::DynamicMessage, crate::dynamic::DynamicSerdeCdrSerdes> {
    /// Get the dynamic schema used by this publisher.
    ///
    /// Returns `None` if the publisher was not created with `.with_dyn_schema()`.
    pub fn schema(&self) -> Option<&crate::dynamic::schema::MessageSchema> {
        self.dyn_schema.as_ref().map(|s| s.as_ref())
    }
}

pub struct ZSubBuilder<T, S = SerdeCdrSerdes<T>> {
    pub(crate) entity: EndpointEntity,
    pub(crate) session: Arc<Session>,
    pub(crate) graph: Arc<Graph>,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    pub(crate) dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    pub(crate) locality: Option<zenoh::sample::Locality>,
    /// Expected encoding for received messages.
    /// If set, the subscriber will validate that received samples match this encoding.
    pub(crate) expected_encoding: Option<crate::encoding::Encoding>,
    pub(crate) _phantom_data: PhantomData<(T, S)>,
}

impl<T, S> ZSubBuilder<T, S>
where
    T: ZMessage,
{
    pub fn with_qos(mut self, qos: QosProfile) -> Self {
        self.entity.qos = qos.to_protocol_qos();
        self
    }

    pub fn with_serdes<S2>(self) -> ZSubBuilder<T, S2> {
        ZSubBuilder {
            entity: self.entity,
            session: self.session,
            graph: self.graph,
            keyexpr_format: self.keyexpr_format.clone(),
            dyn_schema: self.dyn_schema,
            locality: self.locality,
            expected_encoding: self.expected_encoding,
            _phantom_data: PhantomData,
        }
    }

    /// Set the locality restriction for this subscription.
    ///
    /// This restricts the subscription to only receive samples from publishers
    /// with the specified locality (local/remote/any).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use zenoh::sample::Locality;
    ///
    /// let subscriber = node
    ///     .create_sub::<String>("/topic")
    ///     .with_locality(Locality::Remote)  // Only receive from remote publishers
    ///     .build()?;
    /// ```
    pub fn with_locality(mut self, locality: zenoh::sample::Locality) -> Self {
        self.locality = Some(locality);
        self
    }

    /// Set the expected encoding for received messages.
    ///
    /// When set, the subscriber will validate that incoming samples have matching
    /// encoding metadata. If the encoding doesn't match, a warning is logged.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use hiroz::encoding::Encoding;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// // Expect Protobuf encoding
    /// let sub = node.create_sub::<hiroz_msgs::geometry_msgs::Point>("/topic")
    ///     .with_encoding(Encoding::protobuf().with_schema("geometry_msgs/msg/Point"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_encoding(mut self, encoding: crate::encoding::Encoding) -> Self {
        self.expected_encoding = Some(encoding);
        self
    }

    /// Set the dynamic message schema for runtime-typed messages.
    ///
    /// This is required when using `DynamicMessage` with `DynamicSerdeCdrSerdes`.
    /// The schema will be used to deserialize incoming messages.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let subscriber = node
    ///     .create_sub::<DynamicMessage>("/topic")
    ///     .with_serdes::<DynamicSerdeCdrSerdes>()
    ///     .with_dyn_schema(schema)
    ///     .build()?;
    /// ```
    pub fn with_dyn_schema(mut self, schema: Arc<crate::dynamic::schema::MessageSchema>) -> Self {
        // Only compute and set type_info if it hasn't been set already
        // (e.g., from create_dyn_sub_auto which provides the publisher's hash)
        if self.entity.type_info.is_none() {
            self.entity.type_info = Some(crate::dynamic::schema_type_info(&schema));
        }

        self.dyn_schema = Some(schema);
        self
    }

    /// Build a raw Zenoh subscriber with a sample-level callback, returning
    /// the subscriber and liveliness token.
    ///
    /// This is the canonical subscriber setup path used by [`ZCache`] so that
    /// topic qualification, key-expression construction, and liveliness-token
    /// declaration are not duplicated.
    ///
    /// [`ZCache`]: crate::cache::ZCache
    pub(crate) fn build_raw_subscriber<F>(
        mut self,
        callback: F,
    ) -> Result<(
        zenoh::pubsub::Subscriber<()>,
        zenoh::liveliness::LivelinessToken,
    )>
    where
        F: Fn(Sample) + Send + Sync + 'static,
    {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from(
                "subscriber build requires node identity",
            ));
        };
        let qualified_topic =
            crate::topic_name::qualify_topic_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        self.entity.topic = qualified_topic.clone();
        debug!("[CACHE] Qualified topic: {}", qualified_topic);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone();
        debug!("[CACHE] Key expression: {}", key_expr);

        let sub = self
            .session
            .declare_subscriber(key_expr)
            .callback(callback)
            .wait()?;

        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        Ok((sub, lv_token))
    }

    /// Internal method that all build variants use.
    fn build_internal<Q>(
        mut self,
        handler: DataHandler<Sample>,
        queue: Option<Arc<BoundedQueue<Q>>>,
    ) -> Result<ZSub<T, Q, S>>
    where
        S: ZDeserializer,
    {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from(
                "subscriber build requires node identity",
            ));
        };
        let qualified_topic =
            topic_name::qualify_topic_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        self.entity.topic = qualified_topic.clone();
        debug!("[SUB] Qualified topic: {}", qualified_topic);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone(); // Deref and clone the KeyExpr
        debug!(
            "[SUB] Key expression: {}, qos={:?}",
            key_expr, self.entity.qos
        );

        // Wrap handler with encoding validation. No re-entrancy accounting is
        // needed: a user callback is never reached from inside `put` — see
        // `CallbackDispatcher`.
        let expected_encoding = self.expected_encoding.clone();
        let runs_user_code = handler.runs_user_code();
        let validated_handler = Arc::new(move |sample: Sample| {
            // Validate encoding if expected encoding is set
            if let Some(ref expected) = expected_encoding {
                let encoding_str = sample.encoding().to_string();
                if let Some(received) =
                    crate::encoding::Encoding::from_zenoh_encoding(&encoding_str)
                {
                    if &received != expected {
                        tracing::warn!(
                            "Encoding mismatch: expected {:?}, received {:?}",
                            expected,
                            received
                        );
                    }
                } else {
                    tracing::debug!("Unknown encoding format: {}", encoding_str);
                }
            }
            handler.handle(sample)
        });

        // Only go through zenoh-ext when the QoS profile actually configures
        // advanced features. See `qos_needs_advanced`.
        let inner = if qos_needs_advanced(&self.entity.qos) {
            debug!("[SUB] Using AdvancedSubscriber (TransientLocal durability)");
            // `AdvancedSubscriber` holds its state lock across the callback and
            // cannot avoid it, so *user* code is enqueued and runs on the
            // dispatcher's thread. See `CallbackDispatcher`.
            //
            // Capacity comes from the history QoS, exactly as on the plain path.
            // An earlier revision passed `DISPATCH_UNBOUNDED` here, on the
            // grounds that dropping would discard the samples miss-detection
            // recovered. That argument holds for `KeepAll` — which
            // `dispatch_capacity` still maps to `DISPATCH_UNBOUNDED` — but it
            // was applied to every profile, so a `KeepLast(10)` subscriber got
            // an unbounded queue. Since `always_shim` enqueues *remote* samples
            // too, that traded zenoh's transport backpressure for unbounded
            // in-process growth: a publisher outpacing a slow callback grew
            // `pending` until the process died, with only a doubling-threshold
            // `warn!` for a signal. Honouring the declared depth keeps the
            // lossless guarantee where the user asked for it and bounds it
            // where they did not.
            //
            // A queue-mode handler is exempt. It only pushes into a
            // `BoundedQueue` and re-enters nothing, so running it under
            // zenoh-ext's lock is safe — and giving it a dispatcher would add a
            // thread, a wake and a second queue in front of the bounded one to
            // every TransientLocal rmw subscription, for nothing.
            let dispatcher = if runs_user_code {
                Some(CallbackDispatcher::spawn(
                    &qualified_topic,
                    validated_handler.clone(),
                    dispatch_capacity(&self.entity.qos),
                )?)
            } else {
                None
            };
            // Boxed to a common type: `always_shim` returns an opaque `impl Fn`,
            // so the two arms cannot share a `match` unerased.
            let callback: Box<dyn Fn(Sample) + Send + Sync + 'static> = match dispatcher.as_ref() {
                Some(d) => Box::new(d.always_shim()),
                None => Box::new(move |sample: Sample| validated_handler(sample)),
            };
            let mut sub_builder = self.session.declare_subscriber(key_expr).callback(callback);
            if let Some(locality) = self.locality {
                sub_builder = sub_builder.allowed_origin(locality);
                debug!("[SUB] Locality restriction: {:?}", locality);
            }
            let sub_builder = apply_transient_local_sub(sub_builder.advanced(), &self.entity.qos);
            SubscriberHandle::Advanced {
                subscriber: Box::new(sub_builder.wait()?),
                dispatcher,
            }
        } else if runs_user_code {
            // A plain subscriber holds no lock across the callback, but zenoh
            // still delivers a same-thread publication *inline* — so a callback
            // that publishes into its own topic graph would recurse. Hand those
            // samples to the dispatcher; deliver everything else inline, which
            // keeps the inter-process path at one thread-local read. Bounded at
            // the history depth, drop-oldest — the same `KEEP_LAST(depth)` the
            // queue-mode path enforces with `BoundedQueue`.
            let dispatcher = CallbackDispatcher::spawn(
                &qualified_topic,
                validated_handler.clone(),
                dispatch_capacity(&self.entity.qos),
            )?;
            let mut sub_builder = self
                .session
                .declare_subscriber(key_expr)
                .callback(dispatcher.local_only_shim(validated_handler));
            if let Some(locality) = self.locality {
                sub_builder = sub_builder.allowed_origin(locality);
                debug!("[SUB] Locality restriction: {:?}", locality);
            }
            SubscriberHandle::Plain {
                subscriber: sub_builder.wait()?,
                dispatcher: Some(dispatcher),
            }
        } else {
            // Queue mode: the delivery thread only enqueues, so there is no user
            // code to move off it and no dispatcher to pay for.
            let mut sub_builder = self
                .session
                .declare_subscriber(key_expr)
                .callback(move |sample: Sample| validated_handler(sample));
            if let Some(locality) = self.locality {
                sub_builder = sub_builder.allowed_origin(locality);
                debug!("[SUB] Locality restriction: {:?}", locality);
            }
            SubscriberHandle::Plain {
                subscriber: sub_builder.wait()?,
                dispatcher: None,
            }
        };

        let gid = crate::entity::endpoint_gid(&self.entity)
            .expect("local endpoint always has node identity");
        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        debug!("[SUB] Subscriber ready: topic={}", self.entity.topic);

        Ok(ZSub {
            entity: self.entity,
            _inner: inner,
            _lv_token: lv_token,
            queue,
            events_mgr: Arc::new(Mutex::new(EventsManager::new(gid))),
            graph: self.graph,
            dyn_schema: self.dyn_schema,
            expected_encoding: self.expected_encoding,
            _phantom_data: Default::default(),
        })
    }

    /// Build a subscriber with a callback that processes deserialized messages directly.
    ///
    /// This method creates a subscriber that invokes the provided callback for each
    /// received message, bypassing the internal queue. The callback receives the
    /// deserialized message directly. Liveliness tokens and event management are
    /// preserved.
    ///
    /// # Ownership
    ///
    /// The returned [`ZSub`] **must be kept alive** for as long as the subscription
    /// should remain active. Dropping it undeclares the Zenoh subscriber and the
    /// liveliness token (removing the node from the ROS graph).
    ///
    /// **Binding layers handle this automatically**: `hiroz-py` and `hiroz-go` store
    /// the handle inside the node (matching `rmw_zenoh_cpp`'s `NodeData::subs_`
    /// pattern), so Python/Go callers do not need to assign the return value.
    /// Rust callers must store the `ZSub` in their node or context.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called with each deserialized message
    ///
    /// # Returns
    ///
    /// A `ZSub` with no internal queue (callback-only mode)
    pub fn build_with_callback<F>(self, callback: F) -> Result<ZSub<T, (), S>>
    where
        F: Fn(S::Output) + Send + Sync + 'static,
        S: for<'a> ZDeserializer<Input<'a> = &'a [u8]> + 'static,
    {
        let expected_encoding = self.expected_encoding.clone();
        let callback = Arc::new(move |sample: Sample| {
            // Validate encoding if expected encoding is set
            if let Some(ref expected) = expected_encoding {
                let encoding_str = sample.encoding().to_string();
                if let Some(received) =
                    crate::encoding::Encoding::from_zenoh_encoding(&encoding_str)
                {
                    if &received != expected {
                        tracing::warn!(
                            "Encoding mismatch: expected {:?}, received {:?}",
                            expected,
                            received
                        );
                    }
                } else {
                    tracing::debug!("Unknown encoding format: {}", encoding_str);
                }
            }

            let payload = sample.payload().to_bytes();
            match S::deserialize(&payload) {
                Ok(msg) => callback(msg),
                Err(e) => tracing::error!("Failed to deserialize message: {}", e),
            }
        });

        self.build_internal(DataHandler::Callback(callback), None)
    }

    #[cfg(feature = "rmw")]
    pub fn build_with_notifier<F>(self, notify: F) -> Result<ZSub<T, Sample, S>>
    where
        F: Fn() + Send + Sync + 'static,
        S: ZDeserializer,
    {
        let queue_size = match self.entity.qos.history {
            QosHistory::KeepLast(depth) => depth,
            QosHistory::KeepAll => usize::MAX,
        };
        let queue = Arc::new(BoundedQueue::new(queue_size));

        self.build_internal(
            DataHandler::QueueWithNotifier {
                queue: queue.clone(),
                notifier: Arc::new(notify),
            },
            Some(queue),
        )
    }
}

impl<T, S> Builder for ZSubBuilder<T, S>
where
    T: ZMessage + 'static + Sync + Send,
    S: ZDeserializer,
{
    type Output = ZSub<T, Sample, S>;

    fn build(self) -> Result<Self::Output> {
        let queue_size = match self.entity.qos.history {
            QosHistory::KeepLast(depth) => depth,
            QosHistory::KeepAll => usize::MAX,
        };
        let queue = Arc::new(BoundedQueue::new(queue_size));

        self.build_internal(DataHandler::Queue(queue.clone()), Some(queue))
    }
}

pub struct ZSub<T: ZMessage, Q, S: ZDeserializer> {
    pub entity: EndpointEntity,
    pub queue: Option<Arc<BoundedQueue<Q>>>,
    _inner: SubscriberHandle,
    _lv_token: LivelinessToken,
    events_mgr: Arc<Mutex<EventsManager>>,
    graph: Arc<Graph>,
    /// Schema for dynamic message deserialization.
    /// Required when using `DynamicMessage` with `DynamicSerdeCdrSerdes`.
    pub dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Expected encoding for validation.
    pub expected_encoding: Option<crate::encoding::Encoding>,
    _phantom_data: PhantomData<(T, Q, S)>,
}

impl<T: ZMessage, Q, S: ZDeserializer> std::fmt::Debug for ZSub<T, Q, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZSub")
            .field("entity", &self.entity)
            .finish_non_exhaustive()
    }
}

impl<T, S> ZSub<T, Sample, S>
where
    T: ZMessage,
    S: ZDeserializer,
{
    /// Receive the next serialized message (raw sample)
    pub fn recv_serialized(&self) -> Result<Sample> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        Ok(queue.recv())
    }

    /// Async receive the next serialized message (raw sample)
    pub async fn async_recv_serialized(&self) -> Result<Sample> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        Ok(queue.recv_async().await)
    }

    /// Receive the next serialized message with timeout
    pub fn recv_serialized_timeout(&self, timeout: Duration) -> Result<Sample> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        queue
            .recv_timeout(timeout)
            .ok_or_else(|| crate::error::Error::timeout(timeout))
    }

    pub fn events_mgr(&self) -> &Arc<Mutex<EventsManager>> {
        &self.events_mgr
    }

    /// Get a reference to the endpoint entity for this subscriber.
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
    }

    /// Check if there are messages available in the queue
    pub fn is_ready(&self) -> bool {
        self.queue.as_ref().map(|q| !q.is_empty()).unwrap_or(false)
    }

    /// Wait until at least `count` publishers are matched on this subscriber's topic,
    /// or until `timeout` elapses.
    ///
    /// Returns `true` if the required number of publishers appeared within the
    /// timeout, `false` otherwise.
    ///
    /// This mirrors `ZPub::wait_for_subscription` but from the subscriber side.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Ensure at least one publisher is ready before receiving.
    /// assert!(subscriber.wait_for_publisher(1, Duration::from_secs(5)).await);
    /// ```
    pub async fn wait_for_publisher(&self, count: usize, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.graph.change_notify.notified();
            tokio::pin!(notified);

            let n = self
                .graph
                .get_entities_by_topic(EndpointKind::Publisher, &self.entity.topic)
                .len();
            if n >= count {
                return true;
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }

            if tokio::time::timeout(remaining, &mut notified)
                .await
                .is_err()
            {
                return self
                    .graph
                    .get_entities_by_topic(EndpointKind::Publisher, &self.entity.topic)
                    .len()
                    >= count;
            }
        }
    }
}

impl<T, S> ZSub<T, Sample, S>
where
    T: ZMessage,
    S: for<'a> ZDeserializer<Input<'a> = &'a [u8]>,
{
    /// Receive and deserialize the next message (aligned with ROS behavior)
    #[tracing::instrument(name = "recv", skip(self), fields(
        topic = %self.entity.topic,
        payload_len = tracing::field::Empty
    ))]
    pub fn recv(&self) -> Result<S::Output> {
        trace!("[SUB] Waiting for message");

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        let sample = queue.recv();
        let payload = sample.payload().to_bytes();

        tracing::Span::current().record("payload_len", payload.len());
        debug!("[SUB] Received message");

        S::deserialize(&payload).map_err(|e| zenoh::Error::from(e.to_string()))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<S::Output> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        let sample = queue
            .recv_timeout(timeout)
            .ok_or_else(|| crate::error::Error::timeout(timeout))?;
        let payload = sample.payload().to_bytes();
        S::deserialize(&payload).map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Async receive and deserialize the next message
    pub async fn async_recv(&self) -> Result<S::Output> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        let sample = queue.recv_async().await;
        let payload = sample.payload().to_bytes();
        S::deserialize(&payload).map_err(|e| zenoh::Error::from(e.to_string()))
    }
}

// Specialized implementation for DynamicMessage
impl ZSub<crate::dynamic::DynamicMessage, Sample, crate::dynamic::DynamicSerdeCdrSerdes> {
    /// Receive and deserialize the next dynamic message.
    ///
    /// This method requires that the subscriber was built with `.with_dyn_schema()`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The subscriber was built with a callback (no queue available)
    /// - The `dyn_schema` was not set via `.with_dyn_schema()`
    /// - Deserialization fails
    #[tracing::instrument(name = "recv_dynamic", skip(self), fields(
        topic = %self.entity.topic,
        payload_len = tracing::field::Empty
    ))]
    pub fn recv(&self) -> Result<crate::dynamic::DynamicMessage> {
        let schema = self.dyn_schema.as_ref().ok_or_else(|| {
            zenoh::Error::from(
                "dyn_schema required for DynamicMessage (use .with_dyn_schema() when building)",
            )
        })?;

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        trace!("[SUB] Waiting for dynamic message");
        let sample = queue.recv();
        let payload = sample.payload().to_bytes();

        tracing::Span::current().record("payload_len", payload.len());
        debug!("[SUB] Received dynamic message");

        crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Receive a dynamic message with timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<crate::dynamic::DynamicMessage> {
        let schema = self
            .dyn_schema
            .as_ref()
            .ok_or_else(|| zenoh::Error::from("dyn_schema required for DynamicMessage"))?;

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        let sample = queue
            .recv_timeout(timeout)
            .ok_or_else(|| crate::error::Error::timeout(timeout))?;
        let payload = sample.payload().to_bytes();

        crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Async receive a dynamic message.
    pub async fn async_recv(&self) -> Result<crate::dynamic::DynamicMessage> {
        let schema = self
            .dyn_schema
            .as_ref()
            .ok_or_else(|| zenoh::Error::from("dyn_schema required for DynamicMessage"))?;

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        let sample = queue.recv_async().await;
        let payload = sample.payload().to_bytes();

        crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Try to receive a dynamic message without blocking.
    pub fn try_recv(&self) -> Option<Result<crate::dynamic::DynamicMessage>> {
        let schema = self.dyn_schema.as_ref()?;
        let queue = self.queue.as_ref()?;

        match queue.try_recv() {
            Some(sample) => {
                let payload = sample.payload().to_bytes();
                let result = crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
                    .map_err(|e| zenoh::Error::from(e.to_string()));
                Some(result)
            }
            None => None,
        }
    }

    /// Get the dynamic schema.
    pub fn schema(&self) -> Option<&crate::dynamic::schema::MessageSchema> {
        self.dyn_schema.as_ref().map(|s| s.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Topic name qualification (leading '/' is added when missing)
    // -----------------------------------------------------------------------

    #[test]
    fn test_qualify_absolute_topic_unchanged() {
        let result = crate::topic_name::qualify_topic_name("/chatter", "/", "node").unwrap();
        assert_eq!(result, "/chatter");
    }

    #[test]
    fn test_qualify_relative_topic_adds_leading_slash() {
        let result = crate::topic_name::qualify_topic_name("chatter", "/", "node").unwrap();
        assert_eq!(result, "/chatter");
    }

    #[test]
    fn test_qualify_topic_with_namespace() {
        let result = crate::topic_name::qualify_topic_name("chatter", "/ns", "node").unwrap();
        assert_eq!(result, "/ns/chatter");
    }

    #[test]
    fn test_qualify_topic_nested_ns() {
        let result = crate::topic_name::qualify_topic_name("/ns/sub/topic", "/", "node").unwrap();
        assert_eq!(result, "/ns/sub/topic");
    }

    // -----------------------------------------------------------------------
    // QoS override is stored in builder entity.qos
    // QoS defaults: Reliable, Volatile, KeepLast(10)
    // -----------------------------------------------------------------------

    #[test]
    fn test_qos_reliability_encoding() {
        // Reliable is the default, BestEffort maps to protocol value
        let best_effort = QosProfile {
            reliability: crate::qos::QosReliability::BestEffort,
            ..Default::default()
        };
        let proto = best_effort.to_protocol_qos();
        assert_eq!(
            proto.reliability,
            hiroz_protocol::qos::QosReliability::BestEffort
        );
    }

    #[test]
    fn test_qos_durability_encoding() {
        let transient = QosProfile {
            durability: crate::qos::QosDurability::TransientLocal,
            ..Default::default()
        };
        let proto = transient.to_protocol_qos();
        assert_eq!(
            proto.durability,
            hiroz_protocol::qos::QosDurability::TransientLocal
        );
    }

    #[test]
    fn test_qos_keep_last_depth_preserved_in_protocol() {
        use std::num::NonZeroUsize;
        let qos = QosProfile {
            history: crate::qos::QosHistory::KeepLast(NonZeroUsize::new(5).unwrap()),
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(proto.history, hiroz_protocol::qos::QosHistory::KeepLast(5));
    }

    #[test]
    fn test_endpoint_entity_topic_field() {
        let entity = hiroz_protocol::entity::EndpointEntity {
            id: 0,
            node: None,
            kind: hiroz_protocol::entity::EndpointKind::Publisher,
            topic: "/my_topic".to_string(),
            type_info: None,
            qos: Default::default(),
        };
        assert_eq!(entity.topic, "/my_topic");
    }
}
