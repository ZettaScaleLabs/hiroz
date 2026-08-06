use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::reentrancy::TrackedMutex;
use zenoh::Result;

use crate::GidArray;

// Event types matching the RMW specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZenohEventType {
    RequestedQosIncompatible = 0,
    OfferedQosIncompatible = 1,
    MessageLost = 2,
    SubscriptionMatched = 3,
    PublicationMatched = 4,
    SubscriptionIncompatibleType = 5,
    PublisherIncompatibleType = 6,
    OfferedDeadlineMissed = 7,
    RequestedDeadlineMissed = 8,
    LivelinessLost = 9,
    LivelinessChanged = 10,
}

pub const ZENOH_EVENT_ID_MAX: usize = 11;

// Event status structure
#[derive(Debug, Clone, Default)]
pub struct ZenohEventStatus {
    pub total_count: i32,
    pub total_count_change: i32,
    pub current_count: i32,
    pub current_count_change: i32,
    pub data: String,
    pub changed: bool,
    pub last_policy_kind: u32, // RMW QoS policy kind that caused incompatibility
}

// Event callback type.
//
// `Arc` rather than `Box` so that a callback can be *collected* while the
// registry lock is held and *invoked* after it has been released. Event
// callbacks are user code (the rmw layer hands them straight to an rclcpp
// executor), and they routinely re-enter hiroz — querying the graph,
// publishing, unregistering an entity. Invoking them under the registry guard
// makes any such re-entry a self-deadlock on a non-reentrant `Mutex`.
pub type EventCallback = Arc<dyn Fn(i32) + Send + Sync>;

/// Event state for a single publisher/subscription.
///
/// # The `&mut self` rule
///
/// This type is shared as `Arc<Mutex<EventsManager>>`, so **`&mut self` proves
/// the caller holds that outer mutex**. Any `&mut self` method that invoked a
/// callback would therefore run user code under a lock the same thread already
/// holds — and these callbacks re-enter (`rmw_take_event` →
/// [`RmEventHandle::take_event`], or `set_callback` to re-arm). Non-reentrant
/// mutex, one thread, no race: a guaranteed deadlock.
///
/// So the `&mut self` methods here *return* the callback instead, and
/// [`update_shared_event_status`] is the entry point holders actually use.
pub struct EventsManager {
    event_statuses: Vec<ZenohEventStatus>,
    event_callbacks: Vec<Option<EventCallback>>,
    event_mutex: Mutex<()>,
    entity_gid: GidArray,
}

impl EventsManager {
    pub fn new(entity_gid: GidArray) -> Self {
        let mut event_callbacks = Vec::with_capacity(ZENOH_EVENT_ID_MAX);
        for _ in 0..ZENOH_EVENT_ID_MAX {
            event_callbacks.push(None);
        }
        Self {
            event_statuses: vec![ZenohEventStatus::default(); ZENOH_EVENT_ID_MAX],
            event_callbacks,
            event_mutex: Mutex::new(()),
            entity_gid,
        }
    }

    /// Install a callback, delivering any backlog immediately.
    ///
    /// **Only for a caller that owns this manager outright.** It fires the
    /// backlog while the caller's outer guard is still live — the lock this
    /// releases is only the *inner* `event_mutex` — so a re-entering callback
    /// deadlocks. See the `&mut self` note on [`EventsManager`]; holders of an
    /// `Arc<Mutex<..>>` want [`RmEventHandle::set_callback`] instead.
    pub fn set_callback<F>(&mut self, event_type: ZenohEventType, callback: F)
    where
        F: Fn(i32) + Send + Sync + 'static,
    {
        let callback: EventCallback = Arc::new(callback);
        let unread_count = self.install_callback(event_type, callback.clone());
        // Outside the inner `event_mutex` only — see the note above.
        if unread_count != 0 {
            callback(unread_count);
        }
    }

    /// Install `callback` and take (clearing) the unread-event backlog.
    ///
    /// Deliberately does *not* invoke the callback: the caller fires it after
    /// releasing every lock it holds, including the outer `Mutex<EventsManager>`
    /// that [`RmEventHandle`] uses. Returns the backlog count, or 0 if none.
    pub fn install_callback(&mut self, event_type: ZenohEventType, callback: EventCallback) -> i32 {
        let event_id = event_type as usize;
        let _lock = self.event_mutex.lock().unwrap();

        let unread_count = self.event_statuses[event_id].total_count_change;
        if unread_count != 0 {
            self.event_statuses[event_id].total_count_change = 0;
        }
        self.event_callbacks[event_id] = Some(callback);

        unread_count
    }

    /// Record a status change and invoke the registered callback, if any.
    ///
    /// Only safe when the caller does not hold the outer `Mutex<EventsManager>`
    /// — see [`update_shared_event_status`], which is what every holder of an
    /// `Arc<Mutex<EventsManager>>` must use instead.
    pub fn update_event_status(&mut self, event_type: ZenohEventType, change: i32) {
        self.update_event_status_with_policy(event_type, change, 0);
    }

    /// See [`EventsManager::update_event_status`] for the locking caveat.
    pub fn update_event_status_with_policy(
        &mut self,
        event_type: ZenohEventType,
        change: i32,
        policy_kind: u32,
    ) {
        if let Some(callback) =
            self.record_event_status_with_policy(event_type, change, policy_kind)
        {
            callback(change);
        }
    }

    /// Record a status change and hand back the callback that owes a
    /// notification, *without* invoking it.
    ///
    /// Not invoking is the point: see the `&mut self` note on
    /// [`EventsManager`]. The caller invokes once every guard is dropped.
    #[must_use = "the returned callback must be invoked after every guard is dropped"]
    pub fn record_event_status_with_policy(
        &mut self,
        event_type: ZenohEventType,
        change: i32,
        policy_kind: u32,
    ) -> Option<EventCallback> {
        let event_id = event_type as usize;

        {
            let _lock = self.event_mutex.lock().unwrap();
            let status = &mut self.event_statuses[event_id];

            status.total_count += change.max(0);
            status.total_count_change += change.max(0);
            status.current_count = (status.current_count + change).max(0);
            status.current_count_change += change;
            status.changed = true;
            // Update policy kind if provided (non-zero for QoS incompatibility events)
            if policy_kind != 0 {
                status.last_policy_kind = policy_kind;
            }
        }

        self.event_callbacks[event_id].clone()
    }

    pub fn take_event_status(&mut self, event_type: ZenohEventType) -> ZenohEventStatus {
        let event_id = event_type as usize;
        let _lock = self.event_mutex.lock().unwrap();

        let status = self.event_statuses[event_id].clone();
        // Reset change counters
        self.event_statuses[event_id].current_count_change = 0;
        self.event_statuses[event_id].total_count_change = 0;
        self.event_statuses[event_id].changed = false;

        status
    }

    pub fn entity_gid(&self) -> &GidArray {
        &self.entity_gid
    }
}

/// A graph guard condition this manager may trigger on a graph change.
///
/// Registrations are **owned**, not raw pointers, and that is the whole point.
/// Triggering happens with the registry lock released — it must, since the
/// trigger re-enters hiroz — and a raw pointer cloned out of the lock is only
/// valid if something keeps the target alive across the call. Nothing did:
/// `rmw_destroy_node` unregisters and immediately frees, so a destroy landing
/// mid-call dereferenced freed memory.
///
/// **Implementors must keep [`trigger`] valid after the owning C object is
/// gone.** The natural shape is state behind its own `Arc`, one reference held
/// by the C handle and one by this registry.
///
/// [`trigger`]: GraphGuardCondition::trigger
pub trait GraphGuardCondition: Send + Sync {
    /// Wake whatever is waiting on this guard condition.
    ///
    /// Called with no hiroz lock held, possibly concurrently, and possibly
    /// after the corresponding C handle has been destroyed.
    fn trigger(&self);
}

// GraphCache event integration
pub struct GraphEventManager {
    event_callbacks: TrackedMutex<HashMap<GidArray, HashMap<ZenohEventType, EventCallback>>>,
    entity_topics: TrackedMutex<HashMap<GidArray, String>>, // Topic name per registered entity
    graph_guard_conditions: TrackedMutex<Vec<Arc<dyn GraphGuardCondition>>>,
}

impl Default for GraphEventManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphEventManager {
    pub fn new() -> Self {
        Self {
            event_callbacks: TrackedMutex::new(HashMap::new()),
            entity_topics: TrackedMutex::new(HashMap::new()),
            graph_guard_conditions: TrackedMutex::new(Vec::new()),
        }
    }

    pub fn register_event_callback<F>(
        &self,
        entity_gid: GidArray,
        topic: String,
        event_type: ZenohEventType,
        callback: F,
    ) -> Result<()>
    where
        F: Fn(i32) + Send + Sync + 'static,
    {
        {
            let mut callbacks = self.event_callbacks.lock().unwrap();
            let entity_callbacks = callbacks.entry(entity_gid).or_default();
            entity_callbacks.insert(event_type, Arc::new(callback));
        }

        let mut topics = self.entity_topics.lock().unwrap();
        topics.insert(entity_gid, topic);

        Ok(())
    }

    pub fn unregister_entity(&self, entity_gid: &GidArray) {
        // Scoped so the two registries are never held at the same time.
        {
            let mut callbacks = self.event_callbacks.lock().unwrap();
            callbacks.remove(entity_gid);
        }
        let mut topics = self.entity_topics.lock().unwrap();
        topics.remove(entity_gid);
    }

    /// Register a guard condition to be triggered on every graph change.
    ///
    /// The manager keeps the `Arc` alive for as long as it is registered, and
    /// for the duration of any trigger already in flight — see
    /// [`GraphGuardCondition`] for why that ownership is load-bearing.
    pub fn register_graph_guard_condition(&self, guard_condition: Arc<dyn GraphGuardCondition>) {
        let mut conditions = self.graph_guard_conditions.lock().unwrap();
        conditions.push(guard_condition);
    }

    /// Stop triggering `guard_condition`.
    ///
    /// Identity is `Arc::ptr_eq`, so the caller must pass the same allocation it
    /// registered. Returning does **not** mean no trigger is in flight: a
    /// concurrent [`Self::trigger_graph_change`] may already hold its own clone
    /// and be calling into it. That is exactly why the registration is owned —
    /// the in-flight call keeps the target alive, so a caller that frees its own
    /// handle immediately after this returns is still safe.
    pub fn unregister_graph_guard_condition(&self, guard_condition: &Arc<dyn GraphGuardCondition>) {
        let mut conditions = self.graph_guard_conditions.lock().unwrap();
        conditions.retain(|gc| !Arc::ptr_eq(gc, guard_condition));
    }

    pub fn trigger_event(&self, entity_gid: &GidArray, event_type: ZenohEventType, change: i32) {
        self.trigger_event_with_policy(entity_gid, event_type, change, 0);
    }

    pub fn trigger_event_with_policy(
        &self,
        entity_gid: &GidArray,
        event_type: ZenohEventType,
        change: i32,
        policy_kind: u32,
    ) {
        // For QoS incompatibility events, we need to pass policy_kind through a different mechanism
        // since callbacks only take i32. We'll encode it in the change parameter's upper bits for now.
        // This is a workaround - ideally we'd change the callback signature.
        let encoded_change = if policy_kind != 0
            && (matches!(
                event_type,
                ZenohEventType::RequestedQosIncompatible | ZenohEventType::OfferedQosIncompatible
            )) {
            // Encode policy_kind in upper 16 bits, change in lower 16 bits
            // This works because change is always small (number of incompatible entities)
            ((policy_kind as i32) << 16) | (change & 0xFFFF)
        } else {
            change
        };

        // Collect under the lock, invoke after it is released — see [`EventCallback`].
        let callback = {
            let callbacks = self.event_callbacks.lock().unwrap();
            callbacks
                .get(entity_gid)
                .and_then(|entity_callbacks| entity_callbacks.get(&event_type))
                .cloned()
        };

        if let Some(callback) = callback {
            crate::invoke_user_callback!(
                "GraphEventManager::trigger_event_with_policy",
                callback(encoded_change)
            );
        }
    }

    pub fn trigger_graph_change(
        &self,
        entity: &crate::entity::Entity,
        appeared: bool,
        _local_zid: zenoh::session::ZenohId,
    ) {
        use crate::entity::EndpointKind;

        let change = if appeared { 1 } else { -1 };

        // Trigger graph guard conditions for ALL graph changes (local and remote).
        //
        // Snapshot, release the lock, then call — the trigger is rmw-side code
        // and may re-enter this manager, so it must not run under the guard.
        // The snapshot clones `Arc`s rather than raw pointers, which is what
        // makes releasing the lock safe: a concurrent `rmw_destroy_node` can
        // unregister and free its C handle here, and each in-flight trigger
        // still holds the target alive until it returns.
        let guard_conditions = self.graph_guard_conditions.lock().unwrap().clone();
        for gc in guard_conditions {
            gc.trigger();
        }

        // Determine which event type based on entity kind
        // When a publisher appears/disappears, subscriptions get SubscriptionMatched events
        // When a subscription appears/disappears, publishers get PublicationMatched events
        let event_type = match entity {
            crate::entity::Entity::Endpoint(endpoint) => match endpoint.kind {
                EndpointKind::Publisher => ZenohEventType::SubscriptionMatched,
                EndpointKind::Subscription => ZenohEventType::PublicationMatched,
                EndpointKind::Service | EndpointKind::Client => return, // TODO: Add service matched events
            },
            crate::entity::Entity::Node(_) => return, // Node changes don't trigger matched events
        };

        // Find all entities on the same topic that should be notified
        let changed_topic = match entity {
            crate::entity::Entity::Endpoint(endpoint) => &endpoint.topic,
            _ => return,
        };

        // Collect the callbacks to notify, then drop both registry guards before
        // invoking any of them — see [`EventCallback`]. Locks are taken in the
        // same order as `register_event_callback` (callbacks, then topics).
        let to_notify: Vec<EventCallback> = {
            let callbacks = self.event_callbacks.lock().unwrap();
            let entity_topics = self.entity_topics.lock().unwrap();
            callbacks
                .iter()
                .filter(|(entity_gid, _)| {
                    // Only notify entities on the same topic
                    entity_topics
                        .get(*entity_gid)
                        .is_some_and(|registered_topic| registered_topic == changed_topic)
                })
                .filter_map(|(_, entity_callbacks)| entity_callbacks.get(&event_type).cloned())
                .collect()
        };

        for callback in to_notify {
            crate::invoke_user_callback!(
                "GraphEventManager::trigger_graph_change",
                callback(change)
            );
        }
    }
}

// Wait set integration (simplified)
pub struct EventWaitData {
    pub triggered: AtomicBool,
    // TODO: Add condition variable for proper waiting
}

impl Default for EventWaitData {
    fn default() -> Self {
        Self::new()
    }
}

impl EventWaitData {
    pub fn new() -> Self {
        Self {
            triggered: AtomicBool::new(false),
        }
    }

    pub fn is_triggered(&self) -> bool {
        self.triggered.load(Ordering::Acquire)
    }

    pub fn set_triggered(&self, triggered: bool) {
        self.triggered.store(triggered, Ordering::Release);
    }
}

/// Record an event-status change on a *shared* [`EventsManager`] and fire its
/// callback with the manager lock released.
///
/// Holders of an `Arc<Mutex<EventsManager>>` must use this rather than locking
/// and calling [`EventsManager::update_event_status`] directly: that keeps the
/// outer guard alive across the callback, and the callback is user code handed
/// to an rclcpp executor which routinely calls straight back into the same
/// manager (`rmw_take_event` → [`RmEventHandle::take_event`]).
pub fn update_shared_event_status(
    events_mgr: &Mutex<EventsManager>,
    event_type: ZenohEventType,
    change: i32,
) {
    update_shared_event_status_with_policy(events_mgr, event_type, change, 0)
}

/// Detects samples lost **in transit** and raises [`ZenohEventType::MessageLost`].
///
/// Every sample carries an [`Attachment`] with the publisher's GID and a
/// per-publisher sequence number. Holding the last sequence seen from each
/// publisher makes a gap detectable: receiving `n` when `n - 2` was the last
/// means one sample never arrived.
///
/// [`Attachment`]: crate::attachment::Attachment
///
/// # What this does *not* count
///
/// A subscriber dropping its own oldest queued sample because the queue is at
/// its history depth. That sample **arrived** — it updated the last-seen
/// sequence on the way in — so it produces no gap, and the ROS event does not
/// claim it. `rmw_zenoh_cpp` draws the line in the same place: its depth-drops
/// are a debug log, and only sequence gaps raise `MESSAGE_LOST`.
pub struct MessageLossTracker {
    events_mgr: Arc<Mutex<EventsManager>>,
    /// Last sequence number seen per publisher GID.
    ///
    /// Its own lock, and never held across the callout below — raising the
    /// event runs user code, which may re-enter this subscriber.
    last_seen: Mutex<HashMap<GidArray, i64>>,
}

impl MessageLossTracker {
    pub fn new(events_mgr: Arc<Mutex<EventsManager>>) -> Self {
        Self {
            events_mgr,
            last_seen: Mutex::new(HashMap::new()),
        }
    }

    /// Record an arrival, raising the event if it skipped past anything.
    pub fn observe(&self, source_gid: GidArray, sequence_number: i64) {
        let lost = {
            let Ok(mut seen) = self.last_seen.lock() else {
                return;
            };
            match seen.insert(source_gid, sequence_number) {
                // Not the first from this publisher: anything strictly between
                // the two never arrived. A non-positive difference means a
                // retransmit, a reorder, or a publisher that restarted its
                // numbering — none of which is loss, so it reports nothing.
                Some(previous) => sequence_number.saturating_sub(previous).saturating_sub(1),
                // First sample from this publisher. There is no baseline to
                // measure against, and a subscriber that joined late has not
                // "lost" the history it was never sent.
                None => 0,
            }
        };

        if lost > 0 {
            // Clamped rather than truncated: the rmw status field is i32, and a
            // publisher restart can produce an arbitrarily large apparent jump.
            let lost = lost.min(i64::from(i32::MAX)) as i32;
            update_shared_event_status(&self.events_mgr, ZenohEventType::MessageLost, lost);
        }
    }
}

/// [`update_shared_event_status`] with a QoS policy kind.
///
/// # Known hazard
///
/// Releasing the guard also releases the mutual exclusion that used to
/// serialise this against `RmEventHandle::set_callback`, so a concurrent detach
/// can return — and rclcpp can free `user_data` — between the clone below and
/// the call. The `Arc` keeps the *closure* alive; nothing owns the raw C
/// pointer it captured.
///
/// Do not "fix" this by re-locking (that is the deadlock this removes) or by a
/// flag checked in the closure (the free can land between check and call).
/// Tracked with the design in hiroz#287.
pub fn update_shared_event_status_with_policy(
    events_mgr: &Mutex<EventsManager>,
    event_type: ZenohEventType,
    change: i32,
    policy_kind: u32,
) {
    // The guard is bound to its own `let` inside a block on purpose. Written as
    // a `match`/`if let` scrutinee it would stay alive across the invocation
    // below and silently reinstate the deadlock this function exists to remove.
    let callback = {
        let Ok(mut mgr) = events_mgr.lock() else {
            return;
        };
        mgr.record_event_status_with_policy(event_type, change, policy_kind)
    };

    if let Some(callback) = callback {
        callback(change);
    }
}

// RMW-style event handle
pub struct RmEventHandle {
    pub events_mgr: Arc<Mutex<EventsManager>>,
    pub event_type: ZenohEventType,
}

impl std::fmt::Debug for RmEventHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RmEventHandle")
            .field("event_type", &self.event_type)
            .finish()
    }
}

// RmEventHandle is Send because the Arc<Mutex<>> provides thread safety
unsafe impl Send for RmEventHandle {}

impl RmEventHandle {
    pub fn new(events_mgr: Arc<Mutex<EventsManager>>, event_type: ZenohEventType) -> Self {
        Self {
            events_mgr,
            event_type,
        }
    }

    pub fn take_event(&self) -> ZenohEventStatus {
        let mut mgr = self.events_mgr.lock().unwrap();
        mgr.take_event_status(self.event_type)
    }

    pub fn is_ready(&self) -> bool {
        let mgr = self.events_mgr.lock().unwrap();
        mgr.event_statuses[self.event_type as usize].changed
    }

    pub fn set_callback<F>(&self, callback: F)
    where
        F: Fn(i32) + Send + Sync + 'static,
    {
        let callback: EventCallback = Arc::new(callback);
        // Install under the manager lock; fire the backlog notification after it
        // is released so the callback may re-enter this handle.
        let unread_count = {
            let mut mgr = self.events_mgr.lock().unwrap();
            mgr.install_callback(self.event_type, callback.clone())
        };
        if unread_count != 0 {
            callback(unread_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gid(n: u8) -> GidArray {
        let mut g = [0u8; 16];
        g[0] = n;
        g
    }

    // ── EventsManager ────────────────────────────────────────────────────────

    #[test]
    fn test_events_manager_initial_state() {
        let mgr = EventsManager::new(gid(1));
        // All callbacks are None; take_event_status returns zeroed status
        let status = {
            let mut m = mgr;
            m.take_event_status(ZenohEventType::PublicationMatched)
        };
        assert!(!status.changed);
        assert_eq!(status.total_count, 0);
        assert_eq!(status.current_count, 0);
    }

    /// Drive a tracker through a sequence of arrivals and return the total
    /// `MessageLost` count it reported.
    fn losses_for(arrivals: &[(u8, i64)]) -> i32 {
        let mgr = Arc::new(Mutex::new(EventsManager::new(gid(1))));
        let tracker = MessageLossTracker::new(mgr.clone());
        for &(publisher, sn) in arrivals {
            tracker.observe(gid(publisher), sn);
        }
        mgr.lock()
            .unwrap()
            .take_event_status(ZenohEventType::MessageLost)
            .total_count
    }

    #[test]
    fn message_loss_is_counted_from_sequence_gaps() {
        // 0,1,2 contiguous → nothing lost. Then 5 skips 3 and 4.
        assert_eq!(losses_for(&[(1, 0), (1, 1), (1, 2), (1, 5)]), 2);
    }

    #[test]
    fn message_loss_ignores_the_first_sample_from_a_publisher() {
        // A late joiner's first sample has no baseline. Reporting `sn` as the
        // loss count would make every subscriber that starts late look lossy.
        assert_eq!(losses_for(&[(1, 9_000)]), 0);
    }

    #[test]
    fn message_loss_is_tracked_per_publisher() {
        // Interleaved publishers each keep their own baseline; without that,
        // alternating 0,0,1,1 reads as a gap on every other sample.
        assert_eq!(losses_for(&[(1, 0), (2, 0), (1, 1), (2, 1)]), 0);
    }

    #[test]
    fn message_loss_ignores_reorder_and_republish() {
        // A non-positive difference is a retransmit, a reorder, or a publisher
        // that restarted its numbering — none of which is loss.
        assert_eq!(losses_for(&[(1, 5), (1, 3), (1, 5), (1, 0)]), 0);
    }

    #[test]
    fn message_loss_clamps_an_implausible_jump() {
        // A restarted publisher can present an arbitrarily large apparent gap;
        // the rmw status field is i32, so it must saturate rather than wrap.
        assert_eq!(losses_for(&[(1, 0), (1, i64::MAX)]), i32::MAX);
    }

    #[test]
    fn test_update_event_status_fires_callback() {
        let called = Arc::new(Mutex::new(0i32));
        let called_clone = called.clone();

        let mut mgr = EventsManager::new(gid(2));
        mgr.set_callback(ZenohEventType::SubscriptionMatched, move |change| {
            *called_clone.lock().unwrap() += change;
        });

        mgr.update_event_status(ZenohEventType::SubscriptionMatched, 1);
        assert_eq!(*called.lock().unwrap(), 1);

        mgr.update_event_status(ZenohEventType::SubscriptionMatched, 1);
        assert_eq!(*called.lock().unwrap(), 2);
    }

    #[test]
    fn test_update_without_callback_no_panic() {
        let mut mgr = EventsManager::new(gid(3));
        // No callback registered — must not panic
        mgr.update_event_status(ZenohEventType::MessageLost, 1);
        let status = mgr.take_event_status(ZenohEventType::MessageLost);
        assert!(status.changed);
        assert_eq!(status.total_count, 1);
    }

    #[test]
    fn test_set_callback_fires_immediately_for_unread_events() {
        let mut mgr = EventsManager::new(gid(4));
        // Accumulate events before any callback is registered
        mgr.update_event_status(ZenohEventType::PublicationMatched, 3);

        let fired = Arc::new(Mutex::new(0i32));
        let fired_clone = fired.clone();
        // Registering the callback now should fire immediately with the backlog
        mgr.set_callback(ZenohEventType::PublicationMatched, move |change| {
            *fired_clone.lock().unwrap() += change;
        });

        assert_eq!(*fired.lock().unwrap(), 3);
    }

    #[test]
    fn test_set_callback_replaces_existing() {
        let old_fired = Arc::new(Mutex::new(false));
        let new_fired = Arc::new(Mutex::new(false));

        let old_clone = old_fired.clone();
        let new_clone = new_fired.clone();

        let mut mgr = EventsManager::new(gid(5));
        mgr.set_callback(ZenohEventType::LivelinessLost, move |_| {
            *old_clone.lock().unwrap() = true;
        });
        mgr.set_callback(ZenohEventType::LivelinessLost, move |_| {
            *new_clone.lock().unwrap() = true;
        });

        mgr.update_event_status(ZenohEventType::LivelinessLost, 1);
        assert!(!*old_fired.lock().unwrap(), "old callback must not fire");
        assert!(*new_fired.lock().unwrap(), "new callback must fire");
    }

    #[test]
    fn test_take_event_status_resets_change_counters() {
        let mut mgr = EventsManager::new(gid(6));
        mgr.update_event_status(ZenohEventType::RequestedQosIncompatible, 2);

        let first = mgr.take_event_status(ZenohEventType::RequestedQosIncompatible);
        assert!(first.changed);
        assert_eq!(first.total_count_change, 2);

        // Second take: change counters must be reset, total count persists
        let second = mgr.take_event_status(ZenohEventType::RequestedQosIncompatible);
        assert!(!second.changed);
        assert_eq!(second.total_count_change, 0);
        assert_eq!(second.total_count, 2); // cumulative count unchanged
    }

    #[test]
    fn test_update_with_policy_sets_last_policy_kind() {
        let mut mgr = EventsManager::new(gid(7));
        mgr.update_event_status_with_policy(ZenohEventType::OfferedQosIncompatible, 1, 42);
        let status = mgr.take_event_status(ZenohEventType::OfferedQosIncompatible);
        assert_eq!(status.last_policy_kind, 42);
    }

    // ── GraphEventManager ────────────────────────────────────────────────────

    #[test]
    fn test_graph_event_manager_register_and_trigger() {
        let mgr = GraphEventManager::new();
        let fired = Arc::new(Mutex::new(0i32));
        let fired_clone = fired.clone();

        mgr.register_event_callback(
            gid(1),
            "/test".to_string(),
            ZenohEventType::SubscriptionMatched,
            move |v| {
                *fired_clone.lock().unwrap() += v;
            },
        )
        .unwrap();

        mgr.trigger_event(&gid(1), ZenohEventType::SubscriptionMatched, 5);
        assert_eq!(*fired.lock().unwrap(), 5);
    }

    #[test]
    fn test_graph_event_manager_unregister_stops_firing() {
        let mgr = GraphEventManager::new();
        let fired = Arc::new(Mutex::new(0i32));
        let fired_clone = fired.clone();

        mgr.register_event_callback(
            gid(2),
            "/test".to_string(),
            ZenohEventType::PublicationMatched,
            move |v| {
                *fired_clone.lock().unwrap() += v;
            },
        )
        .unwrap();

        mgr.trigger_event(&gid(2), ZenohEventType::PublicationMatched, 1);
        assert_eq!(*fired.lock().unwrap(), 1);

        mgr.unregister_entity(&gid(2));
        mgr.trigger_event(&gid(2), ZenohEventType::PublicationMatched, 1);
        assert_eq!(*fired.lock().unwrap(), 1); // unchanged
    }

    #[test]
    fn test_graph_event_manager_no_callback_no_panic() {
        let mgr = GraphEventManager::new();
        // Trigger on an unregistered GID — must not panic
        mgr.trigger_event(&gid(99), ZenohEventType::LivelinessChanged, 1);
    }

    // ── EventWaitData ────────────────────────────────────────────────────────

    #[test]
    fn test_event_wait_data_set_and_check() {
        let w = EventWaitData::new();
        assert!(!w.is_triggered());
        w.set_triggered(true);
        assert!(w.is_triggered());
        w.set_triggered(false);
        assert!(!w.is_triggered());
    }

    // ── RmEventHandle ────────────────────────────────────────────────────────

    #[test]
    fn test_rmevent_handle_is_ready_and_take() {
        let mgr = Arc::new(Mutex::new(EventsManager::new(gid(8))));
        let handle = RmEventHandle::new(mgr.clone(), ZenohEventType::MessageLost);

        assert!(!handle.is_ready());

        mgr.lock()
            .unwrap()
            .update_event_status(ZenohEventType::MessageLost, 2);

        assert!(handle.is_ready());
        let status = handle.take_event();
        assert_eq!(status.total_count, 2);
        assert!(!handle.is_ready()); // reset after take
    }

    #[test]
    fn test_rmevent_handle_set_callback() {
        let mgr = Arc::new(Mutex::new(EventsManager::new(gid(9))));
        let handle = RmEventHandle::new(mgr.clone(), ZenohEventType::LivelinessChanged);
        let fired = Arc::new(Mutex::new(0i32));
        let fired_clone = fired.clone();

        handle.set_callback(move |v| {
            *fired_clone.lock().unwrap() += v;
        });

        mgr.lock()
            .unwrap()
            .update_event_status(ZenohEventType::LivelinessChanged, 3);
        assert_eq!(*fired.lock().unwrap(), 3);
    }

    /// The registry must **own** what it registers — the invariant that makes
    /// triggering outside the lock safe. See [`GraphGuardCondition`].
    ///
    /// Pins the deterministic half: the registry keeps the value alive after
    /// the registrant drops its handle, and releases it on unregister. It does
    /// not try to schedule the destroy-during-trigger race, which would be a
    /// timing test — ownership is what makes that race harmless.
    #[test]
    fn graph_guard_condition_registration_is_owned_by_the_manager() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingGc(Arc<AtomicUsize>);
        impl GraphGuardCondition for CountingGc {
            fn trigger(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let mgr = GraphEventManager::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let gc: Arc<dyn GraphGuardCondition> = Arc::new(CountingGc(hits.clone()));
        let weak = Arc::downgrade(&gc);

        mgr.register_graph_guard_condition(gc.clone());
        drop(gc);
        let held = weak.upgrade().expect(
            "the manager must keep the registration alive after the registrant drops its handle; \
             otherwise a trigger issued outside the lock dereferences freed memory",
        );

        // Still reachable and callable through the registry's own reference.
        held.trigger();
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        mgr.unregister_graph_guard_condition(&held);
        drop(held);
        assert!(
            weak.upgrade().is_none(),
            "unregister must release the registry's reference, or registrations leak for the \
             life of the process",
        );
    }
}
