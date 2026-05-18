use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};
use zenoh::{Result, Wait, query::Query};

use crate::{
    Builder, ServiceTypeInfo,
    context::ZContext,
    lifecycle::{
        msgs::{
            ChangeState, ChangeStateRequest, ChangeStateResponse, GetAvailableStates,
            GetAvailableStatesRequest, GetAvailableStatesResponse, GetAvailableTransitions,
            GetAvailableTransitionsRequest, GetAvailableTransitionsResponse, GetState,
            GetStateRequest, GetStateResponse, LcState, LcTime, LcTransition,
            LcTransitionDescription, LcTransitionEvent,
        },
        publisher::{ManagedEntity, ZLifecyclePublisher},
        state_machine::{CallbackReturn, State, StateMachine, TransitionId},
    },
    msg::{NativeCdrSerdes, ZDeserializer, ZMessage, ZSerializer},
    node::ZNode,
    service::ZServer,
};

/// A ROS 2 lifecycle-aware node.
///
/// Wraps a [`ZNode`] and adds the full ROS 2 lifecycle state machine:
/// - 5 lifecycle management services (`~/change_state`, `~/get_state`,
///   `~/get_available_states`, `~/get_available_transitions`,
///   `~/get_transition_graph`)
/// - `~/transition_event` publisher
/// - Lifecycle publisher factory ([`create_publisher`]) whose publish calls are
///   gated on the node's activation state
///
/// # Setting lifecycle callbacks
///
/// Override the callback fields after building the node:
///
/// ```no_run
/// use hiroz::lifecycle::{ZLifecycleNode, CallbackReturn, LifecycleState};
/// use hiroz::prelude::*;
///
/// # fn main() -> zenoh::Result<()> {
/// let ctx = ZContextBuilder::default().build()?;
/// let mut node = ctx.create_lifecycle_node("talker").build()?;
/// node.on_configure = Box::new(|_prev| {
///     println!("configuring!");
///     CallbackReturn::Success
/// });
/// node.configure()?;
/// # Ok(())
/// # }
/// ```
pub struct ZLifecycleNode {
    pub inner: ZNode,
    state_machine: Arc<Mutex<StateMachine>>,
    managed_entities: Mutex<Vec<Arc<dyn ManagedEntity>>>,

    /// Called when the node transitions to Inactive (from Unconfigured).
    pub on_configure: Box<dyn Fn(State) -> CallbackReturn + Send + Sync>,
    /// Called when the node transitions to Active.
    pub on_activate: Box<dyn Fn(State) -> CallbackReturn + Send + Sync>,
    /// Called when the node transitions from Active to Inactive.
    pub on_deactivate: Box<dyn Fn(State) -> CallbackReturn + Send + Sync>,
    /// Called when the node transitions from Inactive to Unconfigured.
    pub on_cleanup: Box<dyn Fn(State) -> CallbackReturn + Send + Sync>,
    /// Called when the node shuts down from any primary state.
    pub on_shutdown: Box<dyn Fn(State) -> CallbackReturn + Send + Sync>,
    /// Called when a callback returns `Error`. Default returns `Failure`
    /// (→ Finalized). Override to return `Success` (→ Unconfigured).
    pub on_error: Box<dyn Fn(State) -> CallbackReturn + Send + Sync>,

    // Services held alive
    _srv_change_state: ZServer<ChangeState, ()>,
    _srv_get_state: ZServer<GetState, ()>,
    _srv_get_available_states: ZServer<GetAvailableStates, ()>,
    _srv_get_available_transitions: ZServer<GetAvailableTransitions, ()>,
    _srv_get_transition_graph: ZServer<GetAvailableTransitions, ()>,

    // Transition-event publisher (Arc so trigger_transition can publish)
    te_pub: Arc<crate::pubsub::ZPub<LcTransitionEvent, NativeCdrSerdes<LcTransitionEvent>>>,
}

impl std::fmt::Debug for ZLifecycleNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZLifecycleNode")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl ZLifecycleNode {
    /// The current lifecycle state.
    pub fn get_current_state(&self) -> State {
        self.state_machine.lock().unwrap().current_state()
    }

    /// Trigger the `configure` transition.
    pub fn configure(&mut self) -> Result<State> {
        self.trigger_transition(TransitionId::Configure)
    }

    /// Trigger the `activate` transition.
    pub fn activate(&mut self) -> Result<State> {
        self.trigger_transition(TransitionId::Activate)
    }

    /// Trigger the `deactivate` transition.
    pub fn deactivate(&mut self) -> Result<State> {
        self.trigger_transition(TransitionId::Deactivate)
    }

    /// Trigger the `cleanup` transition.
    pub fn cleanup(&mut self) -> Result<State> {
        self.trigger_transition(TransitionId::Cleanup)
    }

    /// Trigger the `shutdown` transition from the current primary state.
    pub fn shutdown(&mut self) -> Result<State> {
        let current = self.get_current_state();
        match TransitionId::shutdown_for(current) {
            Some(t) => self.trigger_transition(t),
            None => Ok(current),
        }
    }

    /// Trigger a specific lifecycle transition.
    pub fn trigger_transition(&mut self, transition: TransitionId) -> Result<State> {
        let start = self.get_current_state();
        debug!(node=%self.inner.entity.name, ?transition, ?start, "triggering lifecycle transition");

        let cb_result = {
            let callback: &dyn Fn(State) -> CallbackReturn = match transition {
                TransitionId::Configure => self.on_configure.as_ref(),
                TransitionId::Activate => self.on_activate.as_ref(),
                TransitionId::Deactivate => self.on_deactivate.as_ref(),
                TransitionId::Cleanup => self.on_cleanup.as_ref(),
                TransitionId::UnconfiguredShutdown
                | TransitionId::InactiveShutdown
                | TransitionId::ActiveShutdown => self.on_shutdown.as_ref(),
            };
            self.state_machine
                .lock()
                .unwrap()
                .trigger(transition, callback)
        };

        let final_state = if cb_result == State::ErrorProcessing {
            self.state_machine
                .lock()
                .unwrap()
                .trigger_error_processing(|prev| (self.on_error)(prev))
        } else {
            cb_result
        };

        // Bulk-activate / bulk-deactivate managed entities
        match (start, final_state) {
            (_, State::Active) if start != State::Active => {
                for e in self.managed_entities.lock().unwrap().iter() {
                    e.on_activate();
                }
            }
            (State::Active, _) if final_state != State::Active => {
                for e in self.managed_entities.lock().unwrap().iter() {
                    e.on_deactivate();
                }
            }
            _ => {}
        }

        // Publish transition event
        let te = make_transition_event(transition, start, final_state);
        if let Err(e) = self.te_pub.publish(&te) {
            warn!("failed to publish transition_event: {e}");
        }

        info!(node=%self.inner.entity.name, ?final_state, "lifecycle transition complete");
        Ok(final_state)
    }

    /// Create a lifecycle-gated publisher registered as a managed entity.
    pub fn create_publisher<T>(&self, topic: &str) -> Result<Arc<ZLifecyclePublisher<T>>>
    where
        T: ZMessage + crate::WithTypeInfo + serde::Serialize,
        <T as ZMessage>::Serdes: Send + Sync,
    {
        let inner = self.inner.create_pub::<T>(topic).build()?;
        let lc_pub = ZLifecyclePublisher::new(inner);
        if self.get_current_state() == State::Active {
            lc_pub.on_activate();
        }
        self.managed_entities
            .lock()
            .unwrap()
            .push(lc_pub.clone() as Arc<dyn ManagedEntity>);
        Ok(lc_pub)
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub struct ZLifecycleNodeBuilder {
    pub(crate) ctx: ZContext,
    pub(crate) name: String,
    pub(crate) namespace: Option<String>,
    pub enable_communication_interface: bool,
}

impl ZLifecycleNodeBuilder {
    pub fn with_namespace<S: AsRef<str>>(mut self, ns: S) -> Self {
        self.namespace = Some(crate::entity::normalize_node_namespace(ns.as_ref()));
        self
    }

    pub fn disable_communication_interface(mut self) -> Self {
        self.enable_communication_interface = false;
        self
    }
}

impl Builder for ZLifecycleNodeBuilder {
    type Output = ZLifecycleNode;

    fn build(self) -> Result<ZLifecycleNode> {
        let mut node_builder = self.ctx.create_node(&self.name);
        if let Some(ns) = self.namespace {
            node_builder = node_builder.with_namespace(ns);
        }
        let inner = node_builder.build()?;

        // Shared state machine for service closures
        let sm = Arc::new(Mutex::new(StateMachine::new()));

        // Transition-event publisher
        let te_pub = Arc::new(
            inner
                .create_pub::<LcTransitionEvent>("~/transition_event")
                .build()?,
        );

        // --- change_state ---
        let sm_cs = sm.clone();
        let te_cs = te_pub.clone();
        let srv_change_state = inner
            .create_service_impl::<ChangeState>(
                "~/change_state",
                Some(ChangeState::service_type_info()),
            )
            .build_with_callback(move |query| {
                let Some(req) = decode_request::<ChangeStateRequest>(&query) else {
                    return;
                };
                let label = req.transition.label.clone();
                let tid = req.transition.id;
                let current = sm_cs.lock().unwrap().current_state();
                let transition = if !label.is_empty() {
                    TransitionId::from_label_and_state(&label, current)
                } else {
                    TransitionId::from_id_and_state(tid, current)
                };
                let success = if let Some(t) = transition {
                    let start = current;
                    let goal = sm_cs
                        .lock()
                        .unwrap()
                        .trigger(t, |_| CallbackReturn::Success);
                    let _ = te_cs.publish(&make_transition_event(t, start, goal));
                    true
                } else {
                    warn!("change_state: invalid id={tid} label='{label}' from {current:?}");
                    false
                };
                encode_reply(&query, &ChangeStateResponse { success });
            })?;

        // --- get_state ---
        let sm_gs = sm.clone();
        let srv_get_state = inner
            .create_service_impl::<GetState>("~/get_state", Some(GetState::service_type_info()))
            .build_with_callback(move |query| {
                let Some(_): Option<GetStateRequest> = decode_request(&query) else {
                    return;
                };
                let s = sm_gs.lock().unwrap().current_state();
                encode_reply(
                    &query,
                    &GetStateResponse {
                        current_state: to_lc_state(s),
                    },
                );
            })?;

        // --- get_available_states ---
        let srv_get_available_states = inner
            .create_service_impl::<GetAvailableStates>(
                "~/get_available_states",
                Some(GetAvailableStates::service_type_info()),
            )
            .build_with_callback(move |query| {
                let Some(_): Option<GetAvailableStatesRequest> = decode_request(&query) else {
                    return;
                };
                let available_states = StateMachine::all_states()
                    .iter()
                    .map(|(id, lbl)| LcState {
                        id: *id,
                        label: lbl.to_string(),
                    })
                    .collect();
                encode_reply(&query, &GetAvailableStatesResponse { available_states });
            })?;

        // --- get_available_transitions ---
        let sm_gat = sm.clone();
        let srv_get_available_transitions = inner
            .create_service_impl::<GetAvailableTransitions>(
                "~/get_available_transitions",
                Some(GetAvailableTransitions::service_type_info()),
            )
            .build_with_callback(move |query| {
                let Some(_): Option<GetAvailableTransitionsRequest> = decode_request(&query) else {
                    return;
                };
                let available_transitions = sm_gat
                    .lock()
                    .unwrap()
                    .available_transitions()
                    .into_iter()
                    .map(|(t, s, g)| to_lc_td(t, s, g))
                    .collect();
                encode_reply(
                    &query,
                    &GetAvailableTransitionsResponse {
                        available_transitions,
                    },
                );
            })?;

        // --- get_transition_graph ---
        let srv_get_transition_graph = inner
            .create_service_impl::<GetAvailableTransitions>(
                "~/get_transition_graph",
                Some(GetAvailableTransitions::service_type_info()),
            )
            .build_with_callback(move |query| {
                let Some(_): Option<GetAvailableTransitionsRequest> = decode_request(&query) else {
                    return;
                };
                let available_transitions = StateMachine::all_transitions()
                    .into_iter()
                    .map(|(t, s, g)| to_lc_td(t, s, g))
                    .collect();
                encode_reply(
                    &query,
                    &GetAvailableTransitionsResponse {
                        available_transitions,
                    },
                );
            })?;

        Ok(ZLifecycleNode {
            inner,
            state_machine: sm,
            managed_entities: Mutex::new(Vec::new()),
            on_configure: Box::new(|_| CallbackReturn::Success),
            on_activate: Box::new(|_| CallbackReturn::Success),
            on_deactivate: Box::new(|_| CallbackReturn::Success),
            on_cleanup: Box::new(|_| CallbackReturn::Success),
            on_shutdown: Box::new(|_| CallbackReturn::Success),
            on_error: Box::new(|_| CallbackReturn::Failure),
            _srv_change_state: srv_change_state,
            _srv_get_state: srv_get_state,
            _srv_get_available_states: srv_get_available_states,
            _srv_get_available_transitions: srv_get_available_transitions,
            _srv_get_transition_graph: srv_get_transition_graph,
            te_pub,
        })
    }
}

// ---------------------------------------------------------------------------
// CDR helpers for callback-mode services
// ---------------------------------------------------------------------------

fn decode_request<T>(query: &Query) -> Option<T>
where
    T: hiroz_cdr::CdrDeserialize + Send + Sync + 'static,
{
    match query.payload() {
        Some(payload) => match NativeCdrSerdes::deserialize(payload.to_bytes().as_ref()) {
            Ok(req) => Some(req),
            Err(e) => {
                warn!("failed to deserialize lifecycle request: {e}");
                None
            }
        },
        None => {
            // Empty payload is valid for e.g. GetStateRequest
            NativeCdrSerdes::<T>::deserialize(&[] as &[u8]).ok()
        }
    }
}

fn encode_reply<T: hiroz_cdr::CdrSerialize + hiroz_cdr::CdrSerializedSize>(
    query: &Query,
    resp: &T,
) {
    let bytes = NativeCdrSerdes::serialize(resp);
    let mut reply = query.reply(query.key_expr().clone(), bytes);
    if let Some(att_bytes) = query.attachment()
        && let Ok(att) = crate::attachment::Attachment::try_from(att_bytes)
    {
        reply = reply.attachment(att);
    }
    if let Err(e) = reply.wait() {
        warn!("failed to send lifecycle reply: {e}");
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn to_lc_state(s: State) -> LcState {
    LcState {
        id: s.id(),
        label: s.label().to_string(),
    }
}

fn to_lc_transition(t: TransitionId) -> LcTransition {
    LcTransition {
        id: t.id(),
        label: t.label().to_string(),
    }
}

fn to_lc_td(t: TransitionId, start: State, goal: State) -> LcTransitionDescription {
    LcTransitionDescription {
        transition: to_lc_transition(t),
        start_state: to_lc_state(start),
        goal_state: to_lc_state(goal),
    }
}

fn make_transition_event(t: TransitionId, start: State, goal: State) -> LcTransitionEvent {
    LcTransitionEvent {
        timestamp: LcTime::default(),
        transition: to_lc_transition(t),
        start_state: to_lc_state(start),
        goal_state: to_lc_state(goal),
    }
}
