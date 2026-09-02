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
    /// A non-zero count means this thread produced any sample it is *about* to
    /// deliver. It produced that sample synchronously, from inside `put`. See
    /// [`local_publish_active`].
    static LOCAL_PUBLISH_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII marker that hiroz holds for the duration of a publish.
///
/// hiroz has no single choke point for publishing. Each of `ZPub`'s four
/// publish paths enters a guard itself and holds it across the zenoh `put`:
/// [`ZPub::publish`], [`ZPub::async_publish`], [`ZPub::publish_serialized`] and
/// [`ZPub::publish_sample`].
///
/// A fifth publish path added later must do the same. If it does not,
/// session-local delivery on that path runs inline on the publishing thread.
/// The deadlock this guard prevents then comes back (#249).
///
/// The guard counts nesting rather than sets a flag. A callback can run on a
/// thread that is already inside a publish. A publish issued from that callback
/// then restores the correct depth when its own guard drops.
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
/// hiroz can reach a subscriber callback in two ways. This function
/// discriminates between them. That discrimination is what makes re-entrancy
/// structurally impossible without a cost on the inter-process path.
///
/// * **true** — zenoh delivers the sample *synchronously on the publishing
///   thread*. It does this for same-session delivery: `Session::resolve_put`
///   drops the session lock and calls the local callbacks inline. It also does
///   this for two sessions that share one process with a direct in-process
///   route. That route is `send_push_consume` -> `route_data` -> the peer
///   session's callbacks, with no thread hop. A user callback that runs here
///   can publish into its own topic graph and *recurse* instead of iterate
///   (#249). So hiroz
///   enqueues and returns on this path, exactly as zenoh's own `FifoChannel`
///   handler does. The callback then runs on the dispatcher thread.
///
/// * **false** — the sample arrived over a transport. A zenoh RX worker
///   delivers it (`ZRuntime::RX`, threads named `rx-N`). That worker is never
///   an application thread and never inside a hiroz publish. Nothing can
///   re-enter, so the callback runs inline. The inter-process path pays only
///   this thread-local read.
///
/// This function deliberately keys on the *publishing thread*, not on zenoh's
/// `Locality`. Zenoh still delivers a `Locality::Remote`-tagged sample inline on
/// the publisher's thread when that sample crosses two sessions inside one
/// process. An `allowed_origin(SessionLocal)` split would therefore miss it. The
/// thread is the reliable signal. The origin is not.
fn local_publish_active() -> bool {
    LOCAL_PUBLISH_DEPTH.with(|d| d.get()) != 0
}

/// Backlog size at which an *unbounded* [`CallbackDispatcher`] first warns.
///
/// The threshold doubles after each warning. A persistently slow callback
/// therefore does not flood the log.
///
/// The check excludes bounded dispatchers explicitly. It does not assume that
/// this value is out of their reach: a `KeepLast(1024)` subscriber has exactly
/// this capacity. Such a subscriber would otherwise report that its queue is
/// lossless immediately before it drops a sample. Bounded dispatchers warn on
/// drops instead.
const DISPATCH_BACKLOG_WARN_AT: usize = 1024;

/// The capacity that makes a [`CallbackDispatcher`] unbounded, i.e. lossless.
pub(crate) const DISPATCH_UNBOUNDED: usize = usize::MAX;

/// The dispatcher capacity implied by a subscriber's history QoS.
///
/// This matches what [`ZSubBuilder::build`] gives the queue-mode
/// [`BoundedQueue`]. `KeepLast(depth)` keeps `depth` samples. `KeepAll` keeps
/// everything. A callback subscriber and a queue subscriber with the same QoS
/// therefore retain the same number of undelivered samples. Retention does not
/// depend on which hiroz API the caller chose.
///
/// The two are *not* the same expression. The difference applies only to a zero
/// depth. Zero is the rmw spelling of "system default". [`QosProfile`] cannot
/// produce it, but it can arrive over the wire. This function floors it at 1;
/// the queue path passes it through.
///
/// Retention still agrees. [`BoundedQueue::push`] evicts before it inserts
/// (`len >= capacity` → `pop_front`, then `push_back`), so a capacity of 0 also
/// retains exactly one sample. Only the bookkeeping differs: at capacity 0
/// every push reports a drop, including the first push into an empty queue.
pub(crate) fn dispatch_capacity(qos: &hiroz_protocol::qos::QosProfile) -> usize {
    match qos.history {
        QosHistory::KeepLast(depth) => depth.max(1),
        QosHistory::KeepAll => DISPATCH_UNBOUNDED,
    }
}

struct DispatchState {
    /// Samples awaiting delivery, in the order zenoh decided to deliver them.
    pending: std::collections::VecDeque<Sample>,
    /// [`CallbackDispatcher::drop`] sets this: stop delivering and exit. The
    /// dispatcher discards queued but undelivered samples -- see
    /// [`DispatchQueue::dequeue`] for why teardown does not drain them.
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
    /// Set by the drain thread as its last action, once the drain loop has
    /// exited and the final user callback has returned.
    ///
    /// [`CallbackDispatcher::close`] waits on this rather than on
    /// [`std::thread::JoinHandle::join`], because `join` has no timed form on
    /// stable Rust and `close` must honour a deadline. It lives in its own
    /// mutex so that a waiting `close` never contends with delivery.
    finished: Mutex<bool>,
    /// Signalled with `finished`.
    done: std::sync::Condvar,
    topic: String,
    /// Maximum number of undelivered samples the queue retains.
    /// [`DISPATCH_UNBOUNDED`] means lossless. Any smaller capacity drops the
    /// *oldest* sample on overflow, exactly as [`BoundedQueue::push`] does. See
    /// [`CallbackDispatcher`]'s "Backpressure" section for which path gets
    /// which.
    capacity: usize,
}

impl DispatchQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, DispatchState> {
        // A panicking user callback must not wedge the subscriber: the queue
        // holds no invariant that a partial mutation could break.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The shim callback that hiroz hands to zenoh.
    ///
    /// This function may run with zenoh-ext's state mutex held (advanced path).
    /// It may also run on the publishing thread inside `put` (local path). It
    /// must therefore never publish and never block.
    fn enqueue(&self, sample: Sample) {
        let (backlog, dropped) = {
            let mut state = self.lock();
            if state.closed {
                return;
            }

            // Drop the oldest sample. Never drop the newest. Never drop the
            // incoming one. `BoundedQueue::push` makes the same choice. ROS
            // `KEEP_LAST(depth)` describes the same choice. A bounded queue
            // that *blocked* here would re-create the original deadlock — see
            // the type's docs.
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
            // `DISPATCH_BACKLOG_WARN_AT`: nothing stops a subscriber from
            // declaring `KeepLast(1024)` or deeper. It would then log that the
            // queue is lossless and costs only memory. A bounded queue does the
            // opposite. Bounded queues report drops instead. That warning is
            // immediately below. It is the accurate one.
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

    /// Blocks until a sample arrives, or until the queue closes. A closed queue
    /// returns `None`, which ends the drain loop.
    ///
    /// This function checks `closed` **before** `pending`. That order is the
    /// difference between a bounded and an unbounded teardown.
    ///
    /// An earlier version drained the backlog first. `drop(subscriber)` then
    /// ran a user callback for every queued sample before it returned. On the
    /// unbounded (TransientLocal) path that cost is
    /// `backlog × callback_duration` with no ceiling. A 1 kHz publisher against
    /// a 5 ms callback leaves about 30 000 samples queued after 30 s, so the
    /// drop blocks silently for minutes. It can also block *forever* if a
    /// callback waits on anything the dropping thread must supply.
    ///
    /// Dropping a subscriber means "stop delivering to me". The dispatcher
    /// therefore discards undelivered samples. It does not force them through a
    /// callback the caller has already disposed of. Destroying an rclcpp
    /// subscription does the same. Teardown costs at most one in-flight
    /// callback.
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

/// Runs a subscriber's user callback on a dedicated thread. A FIFO queue feeds
/// that thread.
///
/// This type is hiroz's equivalent of zenoh's `FifoChannel` handler. It is also
/// the equivalent of zenoh-python's `Callback(indirect=True)`. zenoh-python
/// installs that handler by default when you hand `declare_subscriber` a plain
/// callable. The delivery thread enqueues and returns. User code runs on this
/// type's thread.
///
/// A sample takes this path for either of two independent reasons.
///
/// 1. **This same thread published it** ([`local_publish_active`]) — the
///    session-local case. Inline delivery would let a callback that publishes
///    into its own topic graph recurse instead of iterate. The queue makes that
///    feedback loop *iterative*. hiroz therefore needs no re-entrancy depth
///    cap: nothing can reach a callback from inside `put`.
/// 2. **The subscriber is a zenoh-ext `AdvancedSubscriber`.** It invokes the
///    sample callback while it holds the `std::sync::Mutex` that guards its
///    reordering state. It *has* to. `handle_sample` interleaves
///    `callback.call(sample)` with mutation of `last_delivered` and
///    `pending_samples`. `deliver_and_flush` calls the callback, records the
///    delivered sequence number, then drains newly-contiguous pending samples
///    and calls the callback again. zenoh-ext cannot drop the guard before the
///    call the way `Session::resolve_put` does. The lock protects exactly the
///    state the delivery loop walks.
///
/// A sample that matches neither reason arrived over a transport, on a zenoh RX
/// worker, for a plain subscriber. hiroz delivers it inline. It never touches
/// this queue. That is deliberate. The RX thread is not an application thread
/// and holds no hiroz lock, so nothing can re-enter. The inter-process path must
/// not pay for a hazard it does not have.
///
/// # Ordering
///
/// There is one producer path, one FIFO queue and one drain thread. The user
/// therefore observes exactly the order zenoh chose to deliver in.
///
/// On the advanced path the shim enqueues from inside `handle_sample`, under
/// zenoh-ext's state mutex. Enqueue order therefore includes the several
/// back-to-back deliveries that one `deliver_and_flush` performs when it drains
/// pending samples. This changes only the thread the callback runs on. It does
/// not change the reordering and recovery guarantees that `AdvancedSubscriber`
/// exists to provide.
///
/// One ordering property does *not* hold: order between the two paths. A plain
/// subscriber can receive both local and remote publications on one topic. It
/// now runs the local ones on this thread and the remote ones on an RX thread.
/// Their relative order is no longer guaranteed, and the two can overlap.
///
/// This weakens no guarantee that hiroz was actually offering. Neither ROS 2 nor
/// zenoh guarantees ordering across distinct publishers. Several RX workers can
/// already invoke a plain zenoh subscriber concurrently. The change is real, so
/// this section states it rather than leaving a reader to find it later.
///
/// # Backpressure
///
/// The queue **never blocks its producer**. That is not a tuning choice. A
/// bounded queue that blocked would re-create the original deadlock in a new
/// form on both paths.
///
/// On the advanced path the blocked thread sits inside `sub_callback` and holds
/// zenoh-ext's state mutex. On the local path it sits inside the user's own
/// `publish()`. In a closed feedback loop, the drain thread it waits on is the
/// very thread that must publish for the queue to drain.
///
/// zenoh's own `FifoChannel` is bounded *and* blocking. It documents exactly
/// this cost: "a slow subscriber could block the underlying Zenoh thread"
/// (`fifo.rs`). hiroz does not adopt that failure mode.
///
/// That leaves a choice between unbounded (lossless, grows without limit) and
/// bounded drop-oldest (lossy, constant memory). **Both paths take the same
/// bound from the same history depth. They differ only in which samples reach
/// it:**
///
/// * **Plain path — bounded, drop-oldest, capacity from the subscriber's
///   history QoS** ([`dispatch_capacity`]). A plain subscriber is `Volatile`
///   with `KEEP_LAST(depth)`. It already promises only the last `depth`
///   undelivered samples. The queue-mode path enforces exactly that with
///   [`BoundedQueue`], from the same history depth. The two expressions differ
///   only for a zero depth, which [`dispatch_capacity`] documents.
///
///   A callback subscriber that retained *every* undelivered sample would
///   honour a QoS stricter than its declared one. It would also let a tight
///   local publish loop with a slow callback grow the process until it dies.
///   The declared QoS permits it to discard the samples it would retain, so
///   that trade has no upside. Drop-oldest also preserves the relative order of
///   the samples that survive.
///
/// * **Advanced path — the same bound, from the same history depth.** This matches
///   `rmw_zenoh_cpp`. Its `SubscriptionData::add_new_message` drops the oldest
///   sample once `message_queue_.size() >= adapted_qos_profile.depth`. It does
///   so for every arriving sample, with **no `TransientLocal` exemption**: the
///   check reads the history policy only. Upstream sizes its advanced-subscriber
///   cache the same way (`adv_sub_opts.history->max_samples = qos_.depth`).
///
///   `KeepAll` maps to [`DISPATCH_UNBOUNDED`] because that profile asks for
///   losslessness. A declared `KeepLast(depth)` does not, so this path honours
///   the depth. [`Self::always_shim`] enqueues remote samples too, so an
///   unbounded queue here would also replace zenoh's transport backpressure with
///   unbounded in-process growth.
///
///   Both implementations drop **silently**, as the ROS event API sees it.
///   Upstream raises `MESSAGE_LOST` from *sequence-number gaps* between
///   arriving messages. A depth-drop cannot produce such a gap. hiroz raises no
///   `MESSAGE_LOST` event either (#292). The escalating `warn!` below is the
///   only signal here. It is more visible than upstream's debug log.
///
/// Two consequences follow. This section states them rather than leaving a
/// reader to find them later.
///
/// **The bound applies to different samples on each path.** On the plain path
/// only *locally published* samples pass through this queue. zenoh delivers a
/// sample that arrived over a transport inline on an RX worker, and
/// backpressures it at the transport instead. On the advanced path
/// [`Self::always_shim`] enqueues everything, remote samples included.
///
/// A slow callback therefore loses local samples and stalls remote ones on the
/// plain path. It loses either kind on the advanced path. That asymmetry follows
/// from delivering the two on different threads, which is what makes re-entrancy
/// impossible without a cost on the inter-process path.
///
/// **On the advanced path a `KeepAll` subscriber has no backpressure at all.**
/// The queue is genuinely unbounded there, and remote samples enter it. A
/// publisher that outpaces the callback grows `pending` without limit. The
/// escalating backlog warning is the only signal. This honours the declared QoS
/// and is not a defect. Even so, `KeepAll` on a slow callback commits unbounded
/// memory. Choose it deliberately.
pub struct CallbackDispatcher {
    queue: Arc<DispatchQueue>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CallbackDispatcher {
    /// Spawns the drain thread.
    ///
    /// Two callers share `handler`. The drain thread always calls it. The
    /// *plain* path's shim also calls it inline, for samples that did not
    /// originate on this thread. Use [`Self::always_shim`] or
    /// [`Self::local_only_shim`] to obtain the callback to hand to zenoh.
    ///
    /// `capacity` is the number of undelivered samples the queue retains before
    /// it drops the oldest. **All four construction sites pass
    /// [`dispatch_capacity`]:**
    ///
    /// * the plain arm of the typed builder (this module),
    /// * the advanced arm of the typed builder,
    /// * the plain arm of the FFI raw subscriber (`node.rs`),
    /// * the advanced arm of the FFI raw subscriber.
    ///
    /// A callback subscriber therefore retains what its history QoS declares on
    /// every path. See the "Backpressure" section.
    ///
    /// Keep all four sites in step by hand. The PR gate compiles the FFI arms
    /// but does not lint or test them, so a wrong constant there fails no check
    /// (#291).
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
            finished: Mutex::new(false),
            done: std::sync::Condvar::new(),
            topic: topic.to_string(),
            capacity,
        });

        let drain_queue = queue.clone();
        let drain_topic = topic.to_string();
        let thread = std::thread::Builder::new()
            .name("hiroz-sub-drain".to_string())
            .spawn(move || {
                while let Some(sample) = drain_queue.dequeue() {
                    // A panicking user callback must not kill the drain thread.
                    // If it did, the subscriber would stop delivering silently.
                    //
                    // This guard works only where panics unwind. This
                    // workspace's `[profile.opt]` sets `panic = "abort"`, so
                    // there the panic aborts the process before `catch_unwind`
                    // can return `Err`. Neither the recovery nor the log line
                    // runs on that profile.
                    //
                    // The guard is therefore effective for dev, test and
                    // `release` builds. CI runs all three. The guard is inert
                    // for `opt`. A build that opts into aborting on panic has
                    // opted out of surviving one.
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (*handler)(sample)))
                        .is_err()
                    {
                        tracing::error!(
                            topic = %drain_topic,
                            "subscriber callback panicked; dropping the sample and continuing"
                        );
                    }
                }
                // Last action of the thread. `close` waits for this. It is set
                // even when the loop exits because the queue closed mid-flight,
                // which is the case `close` exists to observe.
                {
                    let mut finished = drain_queue
                        .finished
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *finished = true;
                }
                drain_queue.done.notify_all();
            })
            .map_err(|e| {
                zenoh::Error::from(format!("failed to spawn subscriber delivery thread: {e}"))
            })?;

        Ok(Self {
            queue,
            thread: Some(thread),
        })
    }

    /// A shim that enqueues **every** sample.
    ///
    /// The advanced path uses this shim. zenoh-ext holds its state mutex across
    /// the callback whatever the sample's origin.
    pub(crate) fn always_shim(&self) -> impl Fn(Sample) + Send + Sync + 'static {
        let queue = self.queue.clone();
        move |sample: Sample| queue.enqueue(sample)
    }

    /// A shim that enqueues only the samples the delivering thread produced
    /// itself. It calls `handler` inline for every other sample. The plain path
    /// uses this shim.
    ///
    /// The inline branch is the inter-process hot path. A zenoh RX worker
    /// delivers a sample that arrived over a transport. That worker is never
    /// inside a hiroz publish, so [`local_publish_active`] returns false. This
    /// one thread-local read is the only cost the path pays.
    pub(crate) fn local_only_shim<F>(
        &self,
        handler: Arc<F>,
    ) -> impl Fn(Sample) + Send + Sync + 'static
    where
        F: Fn(Sample) + Send + Sync + 'static,
    {
        let queue = self.queue.clone();
        let topic = self.queue.topic.clone();
        move |sample: Sample| {
            if local_publish_active() {
                queue.enqueue(sample);
            } else {
                // The inline branch runs on a zenoh RX worker, so it needs the
                // same guard the drain loop has. Without one, a panicking
                // callback unwinds out of hiroz and into zenoh's receive path.
                //
                // This is the DEFAULT profile, which is what makes it matter:
                // `qos_needs_advanced` is true only for `TransientLocal`, and
                // the ROS 2 default is `Volatile`. Guarding only the drain
                // thread therefore left every remote sample on the common path
                // unguarded -- the half that carries inter-process traffic.
                //
                // The `panic = "abort"` caveat on the drain loop's guard
                // applies here identically: a build that opts into aborting on
                // a panic has opted out of surviving one.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(sample)))
                    .is_err()
                {
                    tracing::error!(
                        topic = %topic,
                        "subscriber callback panicked on the inline path; dropping the \
                         sample and continuing"
                    );
                }
            }
        }
    }
}

/// What [`CallbackDispatcher::close`], [`SubscriberHandle::close`] and
/// [`ZSub::close`] observed. Branch on it; do not ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The drain thread exited before the deadline. No user callback from this
    /// subscriber can still be running.
    Joined,
    /// The deadline expired first. The drain thread was **detached**, not
    /// killed: a user callback from this subscriber may still be running, and
    /// will run to completion. No further callback starts.
    TimedOut,
}

impl Drop for CallbackDispatcher {
    /// Stops delivery and returns at once. **It never joins the drain thread.**
    ///
    /// Dropping a subscriber guarantees that no *new* callback starts. It does
    /// not guarantee that a callback already running has finished. Call
    /// [`CallbackDispatcher::close`] (or [`ZSub::close`]) to wait for that.
    ///
    /// An earlier version joined here. That made `drop(sub)` block for the
    /// whole of an in-flight callback, with no timeout and no log, and it made
    /// every Python drop site depend on remembering `py.allow_threads` — the
    /// callback body is `Python::with_gil`, so joining it while holding the GIL
    /// waits for a thread that is waiting for the GIL. See
    /// #296 (tag G2) for the decision.
    ///
    /// Detaching is not a new state. This type already detached when the caller
    /// dropped the subscriber from inside its own callback, because joining a
    /// thread from itself deadlocks. That special case is gone: it is now the
    /// general rule, so it needs no separate branch.
    ///
    /// Detaching cannot be memory-unsafe. The handler is `Arc<F>` with
    /// `F: Send + Sync + 'static` and the queue is an `Arc`, so the detached
    /// thread borrows nothing from the dropping scope.
    fn drop(&mut self) {
        self.queue.lock().closed = true;
        self.queue.ready.notify_all();
        // Dropping the `JoinHandle` detaches the thread. It does not stop the
        // thread; it stops *waiting* for it. The thread observes `closed` on
        // its next `dequeue` and exits.
        drop(self.thread.take());
    }
}

impl CallbackDispatcher {
    /// Stops delivery and waits, for at most `deadline`, until the drain thread
    /// has exited.
    ///
    /// This is the explicit barrier that [`Drop`] no longer provides. It
    /// consumes the dispatcher, so it cannot be called twice.
    ///
    /// It **discards the undelivered backlog**, exactly as [`Drop`] does. This
    /// is a teardown barrier, not a flush. Draining first would re-create the
    /// unbounded teardown cost that [`DispatchQueue::dequeue`]'s
    /// `closed`-before-`pending` order exists to remove.
    ///
    /// Returns [`CloseOutcome::Joined`] when the thread exited in time, and
    /// [`CloseOutcome::TimedOut`] when it did not — in which case the thread is
    /// detached and its in-flight callback runs to completion.
    ///
    /// Calling this from inside the subscriber's own callback returns
    /// [`CloseOutcome::TimedOut`] immediately. That thread cannot finish while
    /// it is waiting for itself, so there is nothing to wait for.
    pub fn close(mut self, deadline: Duration) -> CloseOutcome {
        self.queue.lock().closed = true;
        self.queue.ready.notify_all();

        let Some(thread) = self.thread.take() else {
            return CloseOutcome::Joined;
        };
        if thread.thread().id() == std::thread::current().id() {
            return CloseOutcome::TimedOut;
        }

        let start = std::time::Instant::now();
        let mut finished = self
            .queue
            .finished
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while !*finished {
            let Some(remaining) = deadline.checked_sub(start.elapsed()) else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            let (guard, timed_out) = self
                .queue
                .done
                .wait_timeout(finished, remaining)
                .unwrap_or_else(|e| e.into_inner());
            finished = guard;
            if timed_out.timed_out() {
                break;
            }
        }
        let exited = *finished;
        drop(finished);

        if !exited {
            warn!(
                topic = %self.queue.topic,
                ?deadline,
                "subscriber delivery thread did not exit before the close deadline; \
                 detaching it. Its in-flight callback will run to completion."
            );
            return CloseOutcome::TimedOut;
        }
        if thread.join().is_err() {
            warn!(
                topic = %self.queue.topic,
                "subscriber delivery thread terminated abnormally"
            );
        }
        CloseOutcome::Joined
    }
}

/// Whether a QoS profile needs a zenoh-ext `AdvancedSubscriber`.
///
/// Both callers are subscriber paths. `ZPubBuilder::build` calls `.advanced()`
/// unconditionally and does not consult this function.
///
/// The advanced entities exist for history replay, sample-miss detection and
/// recovery, and entity detection. [`apply_transient_local_sub`] and
/// [`apply_transient_local_pub`] configure all of these for `TransientLocal`
/// durability only.
///
/// For the ROS 2 default (`Volatile`) an unconfigured `AdvancedSubscriber` adds
/// no protocol behaviour. It *does* run the user callback while it holds a
/// non-reentrant `std::sync::Mutex`. In `advanced_subscriber.rs`, `sub_callback`
/// takes `zlock!(statesref)`. `handle_sample` then calls the callback under that
/// guard. zenoh delivers a session-local sample synchronously on the publishing
/// thread. A publish from inside such a callback therefore deadlocks that thread
/// against itself (#249). A `Volatile` subscriber pays the lock and gains
/// nothing, so hiroz declares a plain subscriber for it instead.
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
    /// A sample that arrived over a transport runs inline on the zenoh RX
    /// worker. hiroz hands a sample the delivering thread published itself to
    /// the dispatcher — see [`CallbackDispatcher`]. `dispatcher` is `None` for
    /// queue-mode subscribers. They run no user code on the delivery thread, so
    /// they need no handoff.
    Plain {
        /// This field comes first in the declaration so that it drops first.
        ///
        /// Rust drops struct fields in declaration order, so this order is a
        /// proof obligation rather than a style choice. See the same field on
        /// [`SubscriberHandle::Advanced`] for why the undeclare must precede
        /// the dispatcher's teardown.
        subscriber: zenoh::pubsub::Subscriber<()>,
        dispatcher: Option<CallbackDispatcher>,
    },
    /// A zenoh-ext advanced subscriber, used for `TransientLocal` durability.
    ///
    /// `dispatcher` is `Some` only when the handler runs user code. zenoh-ext
    /// holds its state lock across the callback, so hiroz must move user code
    /// off that thread. A queue-mode handler only enqueues into a
    /// [`BoundedQueue`] and re-enters nothing. It can therefore run under that
    /// lock safely and needs no thread of its own.
    Advanced {
        /// Boxed because it is several times larger than the plain variant.
        ///
        /// This field comes first in the declaration so that it drops first.
        /// Undeclaring the
        /// subscriber stops new samples from entering the queue. Only then does
        /// the dispatcher discard its backlog and release its thread.
        ///
        /// Rust drops struct fields in declaration order, so this order is a
        /// proof obligation rather than a style choice. The guarantee is weaker
        /// than it looks. zenoh undeclares with `wait_callbacks: false`, so a
        /// sample can still arrive afterwards. `enqueue` returns early once
        /// `closed` is set, which makes such a sample harmless.
        subscriber: Box<AdvancedSubscriber<()>>,
        dispatcher: Option<CallbackDispatcher>,
    },
}

impl SubscriberHandle {
    /// Undeclares the zenoh subscriber, then waits for at most `deadline` for
    /// the dispatcher's drain thread to exit. See [`CallbackDispatcher::close`].
    ///
    /// A queue-mode subscriber has no dispatcher and runs no user code on the
    /// delivery thread, so it reports [`CloseOutcome::Joined`] at once.
    pub fn close(self, deadline: Duration) -> CloseOutcome {
        // Undeclare first, for the same reason the field order does it: no new
        // sample should enter a queue that is about to be discarded.
        let dispatcher = match self {
            Self::Plain {
                subscriber,
                dispatcher,
            } => {
                drop(subscriber);
                dispatcher
            }
            Self::Advanced {
                subscriber,
                dispatcher,
            } => {
                drop(subscriber);
                dispatcher
            }
        };
        match dispatcher {
            Some(dispatcher) => dispatcher.close(deadline),
            None => CloseOutcome::Joined,
        }
    }
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
        // The guard must cover the delivery. Delivery happens in `into_future`,
        // not at the await. zenoh's `PublicationBuilder` future is
        // `std::future::ready(self.wait())`, so the put completes before a
        // future exists to poll. That put includes any inline local-subscriber
        // dispatch.
        //
        // Scoping the guard here rather than across the `.await` also keeps
        // this future `Send`. A thread-local guard held across an await point
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

        // Wrap the handler with encoding validation. This needs no re-entrancy
        // accounting: nothing reaches a user callback from inside `put` — see
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

        // Go through zenoh-ext only when the QoS profile actually configures
        // advanced features. See `qos_needs_advanced`.
        let inner = if qos_needs_advanced(&self.entity.qos) {
            debug!("[SUB] Using AdvancedSubscriber (TransientLocal durability)");
            // `AdvancedSubscriber` holds its state lock across the callback and
            // cannot avoid it. hiroz therefore enqueues *user* code and runs it
            // on the dispatcher's thread. See `CallbackDispatcher`.
            //
            // Capacity comes from the history QoS, exactly as on the plain
            // path. Do not pass `DISPATCH_UNBOUNDED` here. `always_shim`
            // enqueues *remote* samples too, so an unbounded queue on every
            // profile trades zenoh's transport backpressure for unbounded
            // in-process growth. `dispatch_capacity` still maps `KeepAll` to
            // `DISPATCH_UNBOUNDED`, which is where the user asked to be
            // lossless. See `CallbackDispatcher`'s "Backpressure" section.
            //
            // A queue-mode handler is exempt. It only pushes into a
            // `BoundedQueue` and re-enters nothing, so it runs safely under
            // zenoh-ext's lock. A dispatcher would add a thread, a wake and a
            // second queue in front of the bounded one, on every TransientLocal
            // rmw subscription, for nothing.
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
            // A plain subscriber holds no lock across the callback. zenoh still
            // delivers a same-thread publication *inline*, so a callback that
            // publishes into its own topic graph would recurse (#249). Hand
            // those samples to the dispatcher. Deliver everything else inline,
            // which keeps the inter-process path at one thread-local read. The
            // dispatcher bounds its queue at the history depth and drops the
            // oldest sample. That is the same `KEEP_LAST(depth)` the queue-mode
            // path enforces with `BoundedQueue`.
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
    /// # Dropping is not a barrier
    ///
    /// **Dropping a subscriber guarantees that no *new* callback starts. It does
    /// not guarantee that a callback already running has finished.** The drop
    /// returns at once: it closes the queue, discards the undelivered backlog
    /// and detaches the drain thread.
    ///
    /// A callback that was already running therefore runs to completion, and may
    /// still be running after `drop(sub)` returns. Do not assume that dropping
    /// the subscriber releases a resource your callback captured.
    ///
    /// Call [`ZSub::close`] when you need the barrier. It waits, with a deadline
    /// you choose, and tells you whether the thread exited or was detached.
    ///
    /// Dropping from inside the callback itself is safe, and needs no special
    /// case: the drop never waits for anything.
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

impl<T: ZMessage, Q, S: ZDeserializer> ZSub<T, Q, S> {
    /// Tears the subscriber down and waits, for at most `deadline`, until no
    /// callback of this subscriber is still running.
    ///
    /// Use this when you need a barrier. Plain `drop` gives you none: it
    /// guarantees that no *new* callback starts, and returns without waiting
    /// for one that is already running.
    ///
    /// This **discards the undelivered backlog**, exactly as `drop` does. It is
    /// a teardown barrier, not a flush.
    ///
    /// ```text
    /// match sub.close(Duration::from_secs(5)) {
    ///     CloseOutcome::Joined   => // nothing of mine is still running
    ///     CloseOutcome::TimedOut => // a callback is still running; it was detached
    /// }
    /// ```
    pub fn close(self, deadline: Duration) -> CloseOutcome {
        let ZSub {
            _inner, _lv_token, ..
        } = self;
        let outcome = _inner.close(deadline);
        // Same order the field declarations impose on `drop`: the subscriber
        // goes away, then the liveliness token leaves the ROS graph.
        drop(_lv_token);
        outcome
    }
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
    // Advanced-subscriber gating
    //
    // Reverting `qos_needs_advanced` to return `true` unconditionally puts the
    // non-reentrant zenoh-ext lock back on the ROS 2 default profile, which is
    // what #249 was. These pin the decision the rest of the fix rests on.
    //
    // They do not pin the wiring -- that the decision reaches
    // `SubscriberHandle::Plain` -- because `ZSub` holds its handle privately.
    // That gap is #296's first item.
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // D1 -- the teardown contract (#296, tag G2)
    //
    // The contract: `Drop` sets `closed`, notifies, and NEVER joins. It returns
    // at once. `close(deadline)` gives the caller the barrier when they want it,
    // and reports which of the two things happened.
    //
    // `ci/revert-d1.patch` restores the join in `Drop`. Under that revert
    // `drop_returns_while_a_callback_is_still_running` must fail. Every wait
    // below has its own deadline, so the reverted run fails rather than hangs.
    // -----------------------------------------------------------------------

    /// A one-shot latch the test controls. `wait_for` returns whether the latch
    /// was set before the deadline, so a failing run reports rather than hangs.
    struct Latch {
        set: Mutex<bool>,
        cv: std::sync::Condvar,
    }

    impl Latch {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                set: Mutex::new(false),
                cv: std::sync::Condvar::new(),
            })
        }

        fn set(&self) {
            *self.set.lock().unwrap() = true;
            self.cv.notify_all();
        }

        fn wait_for(&self, deadline: Duration) -> bool {
            let start = std::time::Instant::now();
            let mut set = self.set.lock().unwrap();
            while !*set {
                let Some(remaining) = deadline.checked_sub(start.elapsed()) else {
                    break;
                };
                if remaining.is_zero() {
                    break;
                }
                let (guard, res) = self.cv.wait_timeout(set, remaining).unwrap();
                set = guard;
                if res.timed_out() {
                    break;
                }
            }
            *set
        }
    }

    /// Long enough that a genuinely blocking teardown cannot pass by luck,
    /// short enough that a red run reports quickly.
    const D1_SHORT: Duration = Duration::from_secs(5);
    /// The upper bound on a parked callback. Only a red run ever waits this
    /// long, and only to clean up after the assertion has already been made.
    const D1_LONG: Duration = Duration::from_secs(60);

    fn d1_sample() -> Sample {
        zenoh::sample::SampleBuilder::put(
            zenoh::key_expr::KeyExpr::try_from("hiroz/test/d1").unwrap(),
            vec![0u8],
        )
        .into()
    }

    /// Spawns a dispatcher whose callback parks on `release` until the test
    /// lets it go, and signals `entered` as it starts. Returns the dispatcher
    /// with one sample already enqueued and its callback confirmed running.
    fn d1_parked_dispatcher(
        entered: &Arc<Latch>,
        release: &Arc<Latch>,
        calls: &Arc<AtomicUsize>,
    ) -> CallbackDispatcher {
        let (e, r, c) = (entered.clone(), release.clone(), calls.clone());
        let handler = Arc::new(move |_sample: Sample| {
            c.fetch_add(1, Ordering::SeqCst);
            e.set();
            assert!(
                r.wait_for(D1_LONG),
                "the parked callback was never released; the test leaked a thread"
            );
        });
        let dispatcher = CallbackDispatcher::spawn("/d1", handler, 8).unwrap();
        let shim = dispatcher.always_shim();
        shim(d1_sample());
        assert!(
            entered.wait_for(D1_SHORT),
            "the callback never started, so the teardown assertion would be vacuous"
        );
        dispatcher
    }

    #[test]
    fn drop_returns_while_a_callback_is_still_running() {
        let entered = Latch::new();
        let release = Latch::new();
        let dropped = Latch::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let dispatcher = d1_parked_dispatcher(&entered, &release, &calls);

        // Drop from another thread, so this thread can time the drop while the
        // callback is still parked.
        let dropped_signal = dropped.clone();
        let dropper = std::thread::spawn(move || {
            drop(dispatcher);
            dropped_signal.set();
        });

        let returned = dropped.wait_for(D1_SHORT);
        // Release before asserting: a failing run must still terminate.
        release.set();
        dropper.join().unwrap();

        assert!(
            returned,
            "drop(dispatcher) did not return within {D1_SHORT:?} while a callback was \
             still running. Dropping a subscriber must stop new callbacks, not wait \
             for the in-flight one -- see #296 (tag G2)."
        );
    }

    #[test]
    fn close_waits_for_an_in_flight_callback() {
        let entered = Latch::new();
        let release = Latch::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let dispatcher = d1_parked_dispatcher(&entered, &release, &calls);

        let releaser = {
            let release = release.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                release.set();
            })
        };

        let outcome = dispatcher.close(D1_SHORT);
        releaser.join().unwrap();

        assert_eq!(
            outcome,
            CloseOutcome::Joined,
            "close() must report that it joined when the callback finishes in time"
        );
    }

    #[test]
    fn close_reports_a_timeout_rather_than_blocking_forever() {
        let entered = Latch::new();
        let release = Latch::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let dispatcher = d1_parked_dispatcher(&entered, &release, &calls);

        let start = std::time::Instant::now();
        let outcome = dispatcher.close(Duration::from_millis(300));
        let waited = start.elapsed();
        release.set();

        assert_eq!(
            outcome,
            CloseOutcome::TimedOut,
            "a callback that outlives the deadline must be reported, not waited on"
        );
        assert!(
            waited < D1_SHORT,
            "close() waited {waited:?}, far past its 300ms deadline"
        );
    }

    #[test]
    fn close_discards_the_backlog_rather_than_flushing_it() {
        let entered = Latch::new();
        let release = Latch::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let dispatcher = d1_parked_dispatcher(&entered, &release, &calls);

        // Three more samples queue up behind the parked callback.
        let shim = dispatcher.always_shim();
        for _ in 0..3 {
            shim(d1_sample());
        }

        let releaser = {
            let release = release.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                release.set();
            })
        };
        let outcome = dispatcher.close(D1_SHORT);
        releaser.join().unwrap();

        assert_eq!(outcome, CloseOutcome::Joined);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "close() is a teardown barrier, not a flush: the three queued samples \
             must be discarded, exactly as drop discards them"
        );
    }

    #[test]
    fn volatile_does_not_need_an_advanced_subscriber() {
        let qos = hiroz_protocol::qos::QosProfile {
            durability: QosDurability::Volatile,
            ..Default::default()
        };
        assert!(
            !qos_needs_advanced(&qos),
            "Volatile is the ROS 2 default. An AdvancedSubscriber adds no protocol \
             behaviour for it and runs the user callback under a non-reentrant mutex"
        );
    }

    #[test]
    fn transient_local_needs_an_advanced_subscriber() {
        let qos = hiroz_protocol::qos::QosProfile {
            durability: QosDurability::TransientLocal,
            ..Default::default()
        };
        assert!(
            qos_needs_advanced(&qos),
            "TransientLocal needs history replay and miss recovery"
        );
    }

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
