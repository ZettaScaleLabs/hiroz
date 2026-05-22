use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

// Event callback type
pub type EventCallback = Box<dyn Fn(i32) + Send + Sync>;

// EventsManager - manages event state for a single publisher/subscription
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

    pub fn set_callback<F>(&mut self, event_type: ZenohEventType, callback: F)
    where
        F: Fn(i32) + Send + Sync + 'static,
    {
        let event_id = event_type as usize;
        let _lock = self.event_mutex.lock().unwrap();

        // If there are unread events, trigger the callback immediately
        let unread_count = self.event_statuses[event_id].total_count_change;
        if unread_count != 0 {
            callback(unread_count);
            self.event_statuses[event_id].total_count_change = 0;
        }

        self.event_callbacks[event_id] = Some(Box::new(callback));
    }

    pub fn update_event_status(&mut self, event_type: ZenohEventType, change: i32) {
        self.update_event_status_with_policy(event_type, change, 0);
    }

    pub fn update_event_status_with_policy(
        &mut self,
        event_type: ZenohEventType,
        change: i32,
        policy_kind: u32,
    ) {
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

        // Trigger callback if registered
        if let Some(ref callback) = self.event_callbacks[event_id] {
            callback(change);
        }
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

// Callback type for triggering graph guard conditions
pub type GraphGuardConditionTrigger = Box<dyn Fn(*mut std::ffi::c_void) + Send + Sync>;

// GraphCache event integration
pub struct GraphEventManager {
    event_callbacks: Mutex<HashMap<GidArray, HashMap<ZenohEventType, EventCallback>>>,
    entity_topics: Mutex<HashMap<GidArray, String>>, // Topic name per registered entity
    graph_guard_conditions: Mutex<Vec<usize>>,       // Pointers as usize for Send
    trigger_guard_condition: Mutex<Option<GraphGuardConditionTrigger>>,
}

impl Default for GraphEventManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphEventManager {
    pub fn new() -> Self {
        Self {
            event_callbacks: Mutex::new(HashMap::new()),
            entity_topics: Mutex::new(HashMap::new()),
            graph_guard_conditions: Mutex::new(Vec::new()),
            trigger_guard_condition: Mutex::new(None),
        }
    }

    pub fn set_guard_condition_trigger(&self, trigger: GraphGuardConditionTrigger) {
        *self.trigger_guard_condition.lock().unwrap() = Some(trigger);
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
        let mut callbacks = self.event_callbacks.lock().unwrap();
        let entity_callbacks = callbacks.entry(entity_gid).or_default();
        entity_callbacks.insert(event_type, Box::new(callback));

        let mut topics = self.entity_topics.lock().unwrap();
        topics.insert(entity_gid, topic);

        Ok(())
    }

    pub fn unregister_entity(&self, entity_gid: &GidArray) {
        let mut callbacks = self.event_callbacks.lock().unwrap();
        callbacks.remove(entity_gid);
        let mut topics = self.entity_topics.lock().unwrap();
        topics.remove(entity_gid);
    }

    pub fn register_graph_guard_condition(&self, guard_condition: *mut std::ffi::c_void) {
        let mut conditions = self.graph_guard_conditions.lock().unwrap();
        conditions.push(guard_condition as usize);
    }

    pub fn unregister_graph_guard_condition(&self, guard_condition: *mut std::ffi::c_void) {
        let mut conditions = self.graph_guard_conditions.lock().unwrap();
        let gc_usize = guard_condition as usize;
        conditions.retain(|&gc| gc != gc_usize);
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

        let callbacks = self.event_callbacks.lock().unwrap();
        if let Some(entity_callbacks) = callbacks.get(entity_gid)
            && let Some(callback) = entity_callbacks.get(&event_type)
        {
            callback(encoded_change);
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

        // Trigger graph guard conditions for ALL graph changes (local and remote)
        if let Some(ref trigger) = *self.trigger_guard_condition.lock().unwrap() {
            let guard_conditions = self.graph_guard_conditions.lock().unwrap();
            for &gc_usize in guard_conditions.iter() {
                let gc = gc_usize as *mut std::ffi::c_void;
                trigger(gc);
            }
        }

        // Determine which event type based on entity kind
        // When a publisher appears/disappears, subscriptions get SubscriptionMatched events
        // When a subscription appears/disappears, publishers get PublicationMatched events
        let event_type = match entity {
            crate::entity::Entity::Endpoint(endpoint) => match endpoint.kind {
                EndpointKind::Publisher => ZenohEventType::SubscriptionMatched,
                EndpointKind::Subscription => ZenohEventType::PublicationMatched,
                EndpointKind::Service => return, // TODO: Add service matched events
                EndpointKind::Client => return,  // TODO: Add service matched events
            },
            crate::entity::Entity::Node(_) => return, // Node changes don't trigger matched events
        };

        // Find all entities on the same topic that should be notified
        let changed_topic = match entity {
            crate::entity::Entity::Endpoint(endpoint) => &endpoint.topic,
            _ => return,
        };

        let entity_topics = self.entity_topics.lock().unwrap();
        let callbacks = self.event_callbacks.lock().unwrap();
        for (entity_gid, entity_callbacks) in callbacks.iter() {
            // Only notify entities on the same topic
            if let Some(registered_topic) = entity_topics.get(entity_gid)
                && registered_topic == changed_topic
                && let Some(callback) = entity_callbacks.get(&event_type)
            {
                callback(change);
            }
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
        let mut mgr = self.events_mgr.lock().unwrap();
        mgr.set_callback(self.event_type, callback);
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
}
