//! Action server implementation for ROS 2 actions.
//!
//! This module provides the server-side functionality for ROS 2 actions,
//! allowing nodes to accept goals from action clients, execute them,
//! provide feedback, and return results.

use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio_util::sync::CancellationToken;
use zenoh::{Result, Wait};

use super::{
    GoalId, GoalInfo, GoalStatus, Time, ZAction,
    messages::*,
    state::{SafeGoalManager, ServerGoalState},
};
use crate::{
    Builder, attachment::Attachment, entity::TypeInfo, msg::ZMessage,
    topic_name::qualify_topic_name,
};

/// Per-goal cancel signal routed by [`CancelDispatcher`].
pub(crate) enum RoutedCancel {
    /// A selected cancellation has already updated the goal and received a reply.
    Accepted,
}

/// Processes shared cancel requests and notifies registered execution handles.
///
/// Selection updates the goal manager and its durable flags before replying.
/// Per-goal channels carry notifications only, so a full channel cannot drop a
/// service request or lose an accepted cancellation.
pub(crate) struct CancelDispatcher {
    routes: parking_lot::Mutex<HashMap<GoalId, flume::Sender<RoutedCancel>>>,
}

impl CancelDispatcher {
    pub(crate) fn new() -> Self {
        Self {
            routes: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Register a goal; returns the per-goal receiver.
    pub(crate) fn register(&self, goal_id: GoalId) -> flume::Receiver<RoutedCancel> {
        let (tx, rx) = flume::bounded(4);
        self.routes.lock().insert(goal_id, tx);
        rx
    }

    /// Deregister a goal (call when goal terminates).
    pub(crate) fn deregister(&self, goal_id: GoalId) {
        self.routes.lock().remove(&goal_id);
    }

    fn reply_cancel(query: zenoh::query::Query, response: CancelGoalServiceResponse) {
        let response_bytes = <CancelGoalServiceResponse as ZMessage>::serialize(&response);
        if let Some(raw_attachment) = query.attachment()
            && let Ok(attachment) = Attachment::try_from(raw_attachment)
        {
            let _ = query
                .reply(query.key_expr().clone(), response_bytes)
                .attachment(attachment)
                .wait();
        }
    }

    /// Process each queued request once, using the same selection as driver mode.
    /// Flags and states are updated before replying, including accepted goals that
    /// have not registered an execution handle yet.
    pub(crate) fn drain<A: ZAction>(&self, server: &ZActionServer<A>) {
        while let Some(query) = server.cancel_server().queue().try_recv() {
            let Some(payload) = query.payload() else {
                tracing::warn!("CancelDispatcher: cancel query has no payload");
                continue;
            };
            let request = match CancelGoalServiceRequest::deserialize(&payload.to_bytes()) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!("CancelDispatcher: failed to parse cancel request: {error}");
                    continue;
                }
            };
            let response = server.cancel_goals(&request.goal_info);
            {
                let routes = self.routes.lock();
                for info in &response.goals_canceling {
                    if let Some(sender) = routes.get(&info.goal_id) {
                        // The durable flag is already set; a full notification
                        // channel cannot lose cancellation or its service reply.
                        let _ = sender.try_send(RoutedCancel::Accepted);
                    }
                }
            }
            Self::reply_cancel(query, response);
        }
    }
}

/// Private implementation holding the actual server state.
/// This is wrapped by the public `ZActionServer` handle.
pub(crate) struct InnerServer<A: ZAction> {
    pub(crate) goal_server: Arc<crate::service::ZServer<GoalService<A>>>,
    pub(crate) result_server: Arc<crate::service::ZServer<ResultService<A>>>,
    pub(crate) cancel_server: Arc<crate::service::ZServer<CancelService<A>>>,
    pub(crate) feedback_pub:
        Arc<crate::pubsub::ZPub<FeedbackMessage<A>, <FeedbackMessage<A> as ZMessage>::Serdes>>,
    pub(crate) status_pub:
        Arc<crate::pubsub::ZPub<StatusMessage, <StatusMessage as ZMessage>::Serdes>>,
    pub(crate) goal_manager: Arc<SafeGoalManager<A>>,
    /// Token to cancel the default result handler when switching to full driver mode
    pub(crate) result_handler_token: CancellationToken,
    pub(crate) cancel_dispatcher: Arc<CancelDispatcher>,
}

/// Drop guard that triggers shutdown when the last server handle is dropped.
pub(crate) struct ShutdownGuard {
    pub(crate) token: CancellationToken,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        tracing::debug!("ZActionServer handle dropped, triggering shutdown");
        self.token.cancel();
    }
}

/// Builder for creating an action server.
///
/// The `ZActionServerBuilder` allows you to configure timeouts and QoS settings
/// for different action communication channels before building the server.
///
/// # Examples
///
/// ```no_run
/// # use hiroz::action::*;
/// # use std::time::Duration;
/// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
/// # let node: hiroz::node::ZNode = todo!();
/// let server = node.create_action_server::<Fibonacci>("fibonacci")
///     .with_result_timeout(Duration::from_secs(30))
///     .build()?;
/// # Ok::<(), zenoh::Error>(())
/// ```
pub struct ZActionServerBuilder<'a, A: ZAction> {
    /// The name of the action.
    pub action_name: String,
    /// Reference to the node that will own this server.
    pub node: &'a crate::node::ZNode,
    /// Timeout for result requests.
    pub result_timeout: Duration,
    /// Optional timeout for goal execution.
    pub goal_timeout: Option<Duration>,
    /// QoS profile for the goal service.
    pub goal_service_qos: Option<crate::qos::QosProfile>,
    /// QoS profile for the result service.
    pub result_service_qos: Option<crate::qos::QosProfile>,
    /// QoS profile for the cancel service.
    pub cancel_service_qos: Option<crate::qos::QosProfile>,
    /// QoS profile for the feedback topic.
    pub feedback_topic_qos: Option<crate::qos::QosProfile>,
    /// QoS profile for the status topic.
    pub status_topic_qos: Option<crate::qos::QosProfile>,
    /// Override for goal (send_goal) type info; uses `A::send_goal_type_info()` if None.
    pub goal_type_info: Option<TypeInfo>,
    /// Override for result (get_result) type info; uses `A::get_result_type_info()` if None.
    pub result_type_info: Option<TypeInfo>,
    /// Override for feedback type info; uses `A::feedback_type_info()` if None.
    pub feedback_type_info: Option<TypeInfo>,
    pub _phantom: std::marker::PhantomData<A>,
}

impl<'a, A: ZAction> ZActionServerBuilder<'a, A> {
    pub fn with_result_timeout(mut self, timeout: Duration) -> Self {
        self.result_timeout = timeout;
        self
    }

    pub fn with_goal_timeout(mut self, timeout: Duration) -> Self {
        self.goal_timeout = Some(timeout);
        self
    }

    pub fn with_goal_service_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.goal_service_qos = Some(qos);
        self
    }

    pub fn with_result_service_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.result_service_qos = Some(qos);
        self
    }

    pub fn with_cancel_service_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.cancel_service_qos = Some(qos);
        self
    }

    pub fn with_feedback_topic_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.feedback_topic_qos = Some(qos);
        self
    }

    pub fn with_status_topic_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.status_topic_qos = Some(qos);
        self
    }

    /// Override the goal type info used for graph registration.
    ///
    /// By default `A::send_goal_type_info()` is used. Set this to supply a
    /// runtime-determined type hash (e.g. from Python message classes).
    pub fn with_goal_type_info(mut self, info: TypeInfo) -> Self {
        self.goal_type_info = Some(info);
        self
    }

    /// Override the result type info used for graph registration.
    pub fn with_result_type_info(mut self, info: TypeInfo) -> Self {
        self.result_type_info = Some(info);
        self
    }

    /// Override the feedback type info used for graph registration.
    pub fn with_feedback_type_info(mut self, info: TypeInfo) -> Self {
        self.feedback_type_info = Some(info);
        self
    }
}

impl<'a, A: ZAction> ZActionServerBuilder<'a, A> {
    pub fn new(action_name: &str, node: &'a crate::node::ZNode) -> Self {
        Self {
            action_name: action_name.to_string(),
            node,
            result_timeout: Duration::from_secs(10),
            goal_timeout: None,
            goal_service_qos: None,
            result_service_qos: None,
            cancel_service_qos: None,
            feedback_topic_qos: None,
            status_topic_qos: None,
            goal_type_info: None,
            result_type_info: None,
            feedback_type_info: None,
            _phantom: std::marker::PhantomData,
        }
    }
}

// Legacy result handler to preserve original behavior (using InnerServer)
fn reply_result<A: ZAction>(query: zenoh::query::Query, result: A::Result, status: GoalStatus) {
    let response = GetResultResponse::<A> {
        status: status as i8,
        result,
    };
    let response_bytes = <GetResultResponse<A> as ZMessage>::serialize(&response);
    let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
    let _ = query
        .reply(query.key_expr().clone(), response_bytes)
        .attachment(attachment)
        .wait();
    tracing::debug!("Sent result response");
}

async fn handle_result_requests_legacy_inner<A: ZAction>(
    inner: &InnerServer<A>,
    query: zenoh::query::Query,
) {
    tracing::debug!("Received result request");
    let payload = query.payload().unwrap().to_bytes();
    let request = match <GetResultRequest as ZMessage>::deserialize(&payload) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to deserialize result request: {}", e);
            return;
        }
    };

    let goal_id = request.goal_id;

    // Either extract the result immediately (goal already terminated / unknown) or
    // register a oneshot channel so `ExecutingGoal::terminate` can notify us later.
    let (result_data, maybe_rx) = inner.goal_manager.modify(|manager| {
        if let Some(ServerGoalState::Terminated { result, status, .. }) =
            manager.goals.get(&goal_id)
        {
            (Some((result.clone(), *status)), None)
        } else if manager.goals.contains_key(&goal_id) {
            let (tx, rx) = tokio::sync::oneshot::channel();
            manager.result_futures.entry(goal_id).or_default().push(tx);
            (None, Some(rx))
        } else {
            // Unknown goal — reply STATUS_UNKNOWN so the client does not hang.
            (Some((A::Result::default(), GoalStatus::Unknown)), None)
        }
    });

    if let Some((result, status)) = result_data {
        tracing::debug!("Goal {:?} already terminated ({:?})", goal_id, status);
        reply_result::<A>(query, result, status);
    } else if let Some(rx) = maybe_rx {
        // Goal not yet terminal — spawn a task so the result loop stays responsive.
        tokio::spawn(async move {
            match rx.await {
                Ok((result, status)) => {
                    tracing::debug!(
                        "Goal {:?} terminated ({:?}), sending result",
                        goal_id,
                        status
                    );
                    reply_result::<A>(query, result, status);
                }
                Err(_) => {
                    tracing::warn!(
                        "Result future dropped for goal {:?}; replying Aborted",
                        goal_id
                    );
                    reply_result::<A>(query, A::Result::default(), GoalStatus::Aborted);
                }
            }
        });
    }
}

impl<'a, A: ZAction> Builder for ZActionServerBuilder<'a, A> {
    type Output = ZActionServer<A>;

    fn build(self) -> Result<Self::Output> {
        // Apply remapping to action name
        let action_name = self.node.remap_rules.apply(&self.action_name);

        // Validate action name
        if action_name.is_empty() {
            return Err(zenoh::Error::from("Action name cannot be empty"));
        }

        // Qualify action name like a topic name
        let qualified_action_name = qualify_topic_name(
            &action_name,
            &self.node.entity.namespace,
            &self.node.entity.name,
        )?;

        tracing::debug!(
            "Action name: '{}', namespace: '{}', qualified: '{}'",
            action_name,
            self.node.entity.namespace,
            qualified_action_name
        );

        // ROS 2 action naming conventions
        let goal_service_name = format!("{}/_action/send_goal", qualified_action_name);
        let result_service_name = format!("{}/_action/get_result", qualified_action_name);
        let cancel_service_name = format!("{}/_action/cancel_goal", qualified_action_name);
        let feedback_topic_name = format!("{}/_action/feedback", qualified_action_name);
        let status_topic_name = format!("{}/_action/status", qualified_action_name);

        // Create goal server using node API for proper graph registration
        // Use override if provided, otherwise fall back to the action's static type info.
        let goal_type_info = Some(self.goal_type_info.unwrap_or_else(A::send_goal_type_info));
        let mut goal_server_builder = self
            .node
            .create_service_impl::<GoalService<A>>(&goal_service_name, goal_type_info);
        if let Some(qos) = self.goal_service_qos {
            goal_server_builder.entity.qos = qos.to_protocol_qos();
        }
        let goal_server = goal_server_builder.build()?;

        // Create result server using node API for proper graph registration
        let result_type_info = Some(
            self.result_type_info
                .unwrap_or_else(A::get_result_type_info),
        );
        let mut result_server_builder = self
            .node
            .create_service_impl::<ResultService<A>>(&result_service_name, result_type_info);
        if let Some(qos) = self.result_service_qos {
            result_server_builder.entity.qos = qos.to_protocol_qos();
        }
        let result_server = result_server_builder.build()?;
        tracing::debug!("Created result server for: {}", result_service_name);

        // Create cancel server using node API for proper graph registration
        // Use the action's cancel_goal_type_info for proper ROS 2 interop
        let cancel_type_info = Some(A::cancel_goal_type_info());
        let mut cancel_server_builder = self
            .node
            .create_service_impl::<CancelService<A>>(&cancel_service_name, cancel_type_info);
        if let Some(qos) = self.cancel_service_qos {
            cancel_server_builder.entity.qos = qos.to_protocol_qos();
        }
        let cancel_server = cancel_server_builder.build()?;

        // Create feedback publisher using node API for proper graph registration
        let feedback_type_info = Some(
            self.feedback_type_info
                .unwrap_or_else(A::feedback_type_info),
        );
        let mut feedback_pub_builder = self
            .node
            .create_pub_impl::<FeedbackMessage<A>>(&feedback_topic_name, feedback_type_info);
        if let Some(qos) = self.feedback_topic_qos {
            feedback_pub_builder.entity.qos = qos.to_protocol_qos();
        }
        // Keep attachments enabled for RMW-Zenoh compatibility
        let feedback_pub = feedback_pub_builder.build()?;

        // Create status publisher using node API for proper graph registration
        // Use the action's status_type_info for proper ROS 2 interop
        let status_type_info = Some(A::status_type_info());
        let mut status_pub_builder = self
            .node
            .create_pub_impl::<StatusMessage>(&status_topic_name, status_type_info);
        if let Some(qos) = self.status_topic_qos {
            status_pub_builder.entity.qos = qos.to_protocol_qos();
        }
        // Keep attachments enabled for RMW-Zenoh compatibility
        let status_pub = status_pub_builder.build()?;

        let goal_manager = Arc::new(SafeGoalManager::new(self.result_timeout, self.goal_timeout));

        let cancellation_token = CancellationToken::new();
        let result_handler_token = CancellationToken::new();

        // Create the inner server
        let inner = Arc::new(InnerServer {
            goal_server: Arc::new(goal_server),
            result_server: Arc::new(result_server),
            cancel_server: Arc::new(cancel_server),
            feedback_pub: Arc::new(feedback_pub),
            status_pub: Arc::new(status_pub),
            goal_manager,
            result_handler_token: result_handler_token.clone(),
            cancel_dispatcher: Arc::new(CancelDispatcher::new()),
        });

        // Spawn background task to handle result requests (default mode for manual goal handling)
        // This task will be cancelled if with_handler() is called
        let weak_inner = Arc::downgrade(&inner);
        let global_shutdown = cancellation_token.clone();
        let handler_token = result_handler_token.clone();

        tokio::spawn(async move {
            // Run until EITHER global shutdown OR handler-specific cancellation
            tokio::select! {
                _ = global_shutdown.cancelled() => {
                    tracing::debug!("Result handler stopping due to global shutdown");
                },
                _ = handler_token.cancelled() => {
                    tracing::debug!("Result handler stopping - switching to full driver mode");
                },
                _ = async {
                    while let Some(inner) = weak_inner.upgrade() {
                        let query = inner.result_server.queue().recv_async().await;
                        handle_result_requests_legacy_inner(&inner, query).await;
                    }
                } => {},
            }
        });

        // Note: cancel requests are NOT handled by a background task in polling mode.
        // In polling mode (Python), cancel requests are processed on-demand via
        // GoalHandle::try_process_cancel(), called from the is_cancel_requested getter.
        // This avoids competing with explicit recv_cancel() calls in Rust code.
        // In driver mode (with_handler), the driver loop handles cancel requests.

        Ok(ZActionServer {
            inner,
            _shutdown: Arc::new(ShutdownGuard {
                token: cancellation_token,
            }),
        })
    }
}

/// Action server handle using the Handle Pattern.
///
/// This is a lightweight, cloneable handle that wraps the actual server implementation.
/// When all handles are dropped, the server automatically shuts down.
pub struct ZActionServer<A: ZAction> {
    inner: Arc<InnerServer<A>>,
    /// Drop guard that triggers shutdown when the last handle is dropped
    _shutdown: Arc<ShutdownGuard>,
}

impl<A: ZAction> std::fmt::Debug for ZActionServer<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZActionServer")
            .field("goal_server", &self.inner.goal_server)
            .finish_non_exhaustive()
    }
}

impl<A: ZAction> Clone for ZActionServer<A> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _shutdown: self._shutdown.clone(),
        }
    }
}

// Internal helper for driver to create server handles and access inner fields
impl<A: ZAction> ZActionServer<A> {
    pub(crate) fn from_inner(inner: Arc<InnerServer<A>>) -> Self {
        // Create a dummy shutdown guard that doesn't do anything
        // The driver doesn't control the server lifetime
        let dummy_token = CancellationToken::new();
        Self {
            inner,
            _shutdown: Arc::new(ShutdownGuard { token: dummy_token }),
        }
    }
}

// Provide convenient access to inner fields via getter methods
impl<A: ZAction> ZActionServer<A> {
    fn goal_server(&self) -> &Arc<crate::service::ZServer<GoalService<A>>> {
        &self.inner.goal_server
    }

    fn result_server(&self) -> &Arc<crate::service::ZServer<ResultService<A>>> {
        &self.inner.result_server
    }

    pub(crate) fn cancel_server(&self) -> &Arc<crate::service::ZServer<CancelService<A>>> {
        &self.inner.cancel_server
    }

    pub(crate) fn cancel_dispatcher(&self) -> &Arc<CancelDispatcher> {
        &self.inner.cancel_dispatcher
    }

    fn feedback_pub(
        &self,
    ) -> &Arc<crate::pubsub::ZPub<FeedbackMessage<A>, <FeedbackMessage<A> as ZMessage>::Serdes>>
    {
        &self.inner.feedback_pub
    }

    fn status_pub(
        &self,
    ) -> &Arc<crate::pubsub::ZPub<StatusMessage, <StatusMessage as ZMessage>::Serdes>> {
        &self.inner.status_pub
    }

    /// Access the goal manager for advanced use cases and testing.
    ///
    /// # Warning
    ///
    /// This is a low-level API that gives direct access to the goal state.
    /// Use with caution as it bypasses the normal goal handle abstractions.
    pub fn goal_manager(&self) -> &Arc<SafeGoalManager<A>> {
        &self.inner.goal_manager
    }

    fn result_handler_token(&self) -> &CancellationToken {
        &self.inner.result_handler_token
    }
}

impl<A: ZAction> ZActionServer<A> {
    fn publish_status(&self) {
        // Build status list while holding lock, then release before publishing
        let status_list: Vec<GoalStatusInfo> = self.goal_manager().read(|manager| {
            manager
                .goals
                .values()
                .map(|state| {
                    let status = match state {
                        ServerGoalState::Accepted { .. } => GoalStatus::Accepted,
                        ServerGoalState::Executing { .. } => GoalStatus::Executing,
                        ServerGoalState::Canceling { .. } => GoalStatus::Canceling,
                        ServerGoalState::Terminated { status, .. } => *status,
                    };
                    GoalStatusInfo {
                        goal_info: state.info().clone(),
                        status,
                    }
                })
                .collect()
        }); // Lock released here

        // Publish without holding lock
        let msg = StatusMessage { status_list };
        // FIXME: address the result
        let _ = self.status_pub().publish(&msg);
    }

    pub async fn recv_goal(&self) -> Result<GoalHandle<A, Requested>> {
        let query = self.goal_server().queue().recv_async().await;
        let payload = query.payload().unwrap().to_bytes();
        let request = <SendGoalRequest<A> as ZMessage>::deserialize(&payload)
            .map_err(|e| zenoh::Error::from(e.to_string()))?;

        Ok(GoalHandle {
            goal: request.goal,
            info: GoalInfo::new(request.goal_id),
            server: self.clone(),
            cancel_flag: None,
            cancel_rx: None,
            cleanup: GoalCleanup::new(request.goal_id, self.clone(), Some(query)),
            _state: PhantomData,
        })
    }

    pub async fn recv_cancel(&self) -> Result<(CancelGoalServiceRequest, zenoh::query::Query)> {
        let query = self.cancel_server().queue().recv_async().await;
        let payload = query.payload().unwrap().to_bytes();
        let request = <CancelGoalServiceRequest as ZMessage>::deserialize(&payload)
            .map_err(|e| zenoh::Error::from(e.to_string()))?;
        Ok((request, query))
    }

    pub fn is_cancel_request_ready(&self) -> bool {
        !self.cancel_server().queue().is_empty()
    }

    /// Marks a goal as canceling by setting its atomic cancel flag and transitioning
    /// to [`ServerGoalState::Canceling`] so status subscribers see `CANCELING`.
    ///
    /// Accepted goals (pre-`execute`) are cancelable — the same flag is reused when
    /// execution starts so a cancel that races accept→execute is not lost.
    pub fn request_cancel(&self, goal_id: GoalId) -> bool {
        if !goal_id.is_valid() {
            return false;
        }
        !self
            .cancel_goals(&GoalInfo {
                goal_id,
                stamp: Time::zero(),
            })
            .goals_canceling
            .is_empty()
    }

    /// Apply all four ROS UUID/time selection modes atomically. Both the
    /// automatic driver and manual polling path use this operation.
    pub(crate) fn cancel_goals(&self, request: &GoalInfo) -> CancelGoalServiceResponse {
        let response = self.goal_manager().modify(|manager| {
            let all_ids = !request.goal_id.is_valid();
            let zero_stamp = request.stamp == Time::zero();
            let ids: Vec<_> = manager
                .goals
                .iter()
                .filter(|(_, state)| !matches!(state, ServerGoalState::Terminated { .. }))
                .filter(|(id, state)| {
                    (all_ids && zero_stamp)
                        || (!all_ids && **id == request.goal_id)
                        || (!zero_stamp
                            && (state.info().stamp.sec, state.info().stamp.nanosec)
                                <= (request.stamp.sec, request.stamp.nanosec))
                })
                .map(|(id, _)| *id)
                .collect();
            let mut goals_canceling = Vec::with_capacity(ids.len());
            for id in ids {
                let state = manager.goals.remove(&id).expect("selected goal exists");
                match state {
                    ServerGoalState::Accepted {
                        info,
                        goal,
                        cancel_flag,
                        expires_at,
                        ..
                    }
                    | ServerGoalState::Executing {
                        info,
                        goal,
                        cancel_flag,
                        expires_at,
                    }
                    | ServerGoalState::Canceling {
                        info,
                        goal,
                        cancel_flag,
                        expires_at,
                    } => {
                        cancel_flag.store(true, Ordering::Relaxed);
                        goals_canceling.push(info.clone());
                        manager.goals.insert(
                            id,
                            ServerGoalState::Canceling {
                                info,
                                goal,
                                cancel_flag,
                                expires_at,
                            },
                        );
                    }
                    ServerGoalState::Terminated { .. } => {
                        unreachable!("only active goals selected")
                    }
                }
            }
            // Match rcl_action_process_cancel_request: only UUID-only requests
            // distinguish unknown/terminal IDs. Time-bearing and all-goal
            // requests succeed even if they select no eligible goals.
            let return_code = if !goals_canceling.is_empty() || all_ids || !zero_stamp {
                0
            } else if manager.goals.contains_key(&request.goal_id) {
                3
            } else {
                2
            };
            CancelGoalServiceResponse {
                return_code,
                goals_canceling,
            }
        });
        if !response.goals_canceling.is_empty() {
            self.publish_status();
        }
        response
    }

    pub async fn recv_result_request(&self) -> Result<(GoalId, zenoh::query::Query)> {
        let query = self.result_server().queue().recv_async().await;
        let payload = query.payload().unwrap().to_bytes();
        let request = <ResultRequest as ZMessage>::deserialize(&payload)
            .map_err(|e| zenoh::Error::from(e.to_string()))?;
        Ok((request.goal_id, query))
    }

    // FIXME: check the necessity
    pub fn send_goal_response_low(
        &self,
        query: &zenoh::query::Query,
        response: &GoalResponse,
    ) -> Result<()> {
        let response_bytes = <GoalResponse as ZMessage>::serialize(response);
        let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
        let _ = query
            .reply(query.key_expr().clone(), response_bytes)
            .attachment(attachment)
            .wait();
        Ok(())
    }

    // FIXME: check the necessity
    pub async fn recv_cancel_request_low(
        &self,
    ) -> Result<(CancelGoalServiceRequest, zenoh::query::Query)> {
        let query = self.cancel_server().queue().recv_async().await;
        let payload = query.payload().unwrap().to_bytes();
        let request = <CancelGoalServiceRequest as ZMessage>::deserialize(&payload)
            .map_err(|e| zenoh::Error::from(e.to_string()))?;
        Ok((request, query))
    }

    pub fn send_cancel_response_low(
        &self,
        query: &zenoh::query::Query,
        response: &CancelGoalServiceResponse,
    ) -> Result<()> {
        let response_bytes = <CancelGoalServiceResponse as ZMessage>::serialize(response);
        let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
        let _ = query
            .reply(query.key_expr().clone(), response_bytes)
            .attachment(attachment)
            .wait();
        Ok(())
    }

    // FIXME: check the necessity
    pub fn send_result_response_low(
        &self,
        query: &zenoh::query::Query,
        response: &GetResultResponse<A>,
    ) -> Result<()> {
        let response_bytes = <GetResultResponse<A> as ZMessage>::serialize(response);
        let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
        let _ = query
            .reply(query.key_expr().clone(), response_bytes)
            .attachment(attachment)
            .wait();
        Ok(())
    }

    /// Attaches an automatic goal handler to the server.
    ///
    /// This method transitions the server from "manual mode" (where you call `recv_goal()`)
    /// to "automatic mode" (where goals are handled by the provided callback).
    ///
    /// **Important**: This method cancels the default result-only handler and starts a full
    /// driver loop that handles all protocol events (goals, cancels, results) automatically.
    ///
    /// # Arguments
    ///
    /// * `handler` - Callback function that will be invoked for each accepted goal
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hiroz::action::*;
    /// # use hiroz_msgs::action_tutorials_interfaces::{FibonacciResult, action::Fibonacci};
    /// # let server: hiroz::action::server::ZActionServer<Fibonacci> = todo!();
    /// let server = server.with_handler(|executing: hiroz::action::server::ExecutingGoal<Fibonacci>| async move {
    ///     executing.succeed(FibonacciResult { sequence: vec![1, 1, 2, 3] }).unwrap();
    /// });
    /// ```
    pub fn with_handler<F, Fut>(self, handler: F) -> Self
    where
        F: Fn(GoalHandle<A, Executing>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        // 1. Stop the default result-only handler to avoid competing for result_server.rx
        tracing::debug!("Cancelling default result handler to switch to full driver mode");
        self.result_handler_token().cancel();

        // 2. Start the full driver loop that handles all protocol events
        let weak_inner = Arc::downgrade(&self.inner);
        let shutdown_token = self._shutdown.token.clone();
        tokio::spawn(async move {
            crate::action::driver::run_driver_loop(weak_inner, shutdown_token, handler).await;
        });

        self
    }

    /// Expires goals that have passed their expiration time.
    ///
    /// This method checks all goals with `expires_at` timestamps and removes:
    /// - Accepted/Executing goals that have timed out (goal timeout)
    /// - Terminated goals whose results have expired (result timeout)
    ///
    /// Goals without expiration times (when timeouts are not configured) are never expired.
    ///
    /// # Returns
    ///
    /// Returns a vector of `GoalId`s that were expired.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hiroz::action::*;
    /// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
    /// # let server: hiroz::action::server::ZActionServer<Fibonacci> = todo!();
    /// let expired = server.expire_goals();
    /// println!("Expired {} goals", expired.len());
    /// ```
    pub fn expire_goals(&self) -> Vec<GoalId> {
        let (expired, waiters) = self.goal_manager().modify(|manager| {
            let now = Instant::now();
            let mut expired = Vec::new();
            let mut waiters = Vec::new();

            let to_expire: Vec<GoalId> = manager
                .goals
                .iter()
                .filter_map(|(goal_id, state)| {
                    let should_expire = match state {
                        ServerGoalState::Accepted { expires_at, .. }
                        | ServerGoalState::Executing { expires_at, .. }
                        | ServerGoalState::Canceling { expires_at, .. }
                        | ServerGoalState::Terminated { expires_at, .. } => {
                            expires_at.is_some_and(|exp| now >= exp)
                        }
                    };
                    should_expire.then_some(*goal_id)
                })
                .collect();

            for goal_id in to_expire {
                manager.goals.remove(&goal_id);
                if let Some(txs) = manager.result_futures.remove(&goal_id) {
                    waiters.extend(txs);
                }
                expired.push(goal_id);
            }

            (expired, waiters)
        }); // Lock released here

        for goal_id in &expired {
            self.cancel_dispatcher().deregister(*goal_id);
        }

        // Wake any get_result waiters so clients do not hang after expiration.
        for tx in waiters {
            let _ = tx.send((A::Result::default(), GoalStatus::Aborted));
        }

        // Publish updated status if any goals were expired
        if !expired.is_empty() {
            self.publish_status();
        }

        expired
    }

    /// Sets the result timeout for this server.
    ///
    /// This configures how long the server will keep terminated goals
    /// before they can be expired. Note: This does not automatically
    /// expire goals - you must call `expire_goals()` periodically.
    ///
    /// # Arguments
    ///
    /// * `timeout` - The result timeout duration
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hiroz::action::*;
    /// # use std::time::Duration;
    /// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
    /// # let server: hiroz::action::server::ZActionServer<Fibonacci> = todo!();
    /// server.set_result_timeout(Duration::from_secs(30));
    /// ```
    pub fn set_result_timeout(&self, timeout: Duration) {
        self.goal_manager().modify(|manager| {
            manager.result_timeout = timeout;
        });
    }

    /// Gets the current result timeout for this server.
    ///
    /// # Returns
    ///
    /// The result timeout duration
    pub fn result_timeout(&self) -> Duration {
        self.goal_manager().read(|manager| manager.result_timeout)
    }
}

// --- State Markers for Type-State Pattern ---
/// Marker type representing a goal that has been requested but not yet accepted or rejected.
pub struct Requested;

/// Marker type representing a goal that has been accepted but not yet executing.
pub struct Accepted;

/// Marker type representing a goal that is currently executing.
pub struct Executing;

// Type aliases for convenience
/// A goal handle in the "Requested" state.
pub type RequestedGoal<A> = GoalHandle<A, Requested>;

/// A goal handle in the "Accepted" state.
pub type AcceptedGoal<A> = GoalHandle<A, Accepted>;

/// A goal handle in the "Executing" state.
pub type ExecutingGoal<A> = GoalHandle<A, Executing>;

// Type-state pattern for goal lifecycle with PhantomData markers
/// A type-safe goal handle that uses compile-time state tracking.
///
/// The `GoalHandle` is generic over the action type `A` and the state `State`.
/// Different methods are available depending on the current state, enforced at compile time.
///
/// # Type States
///
/// - `GoalHandle<A, Requested>`: Can be accepted or rejected
/// - `GoalHandle<A, Accepted>`: Can be executed
/// - `GoalHandle<A, Executing>`: Can publish feedback and be terminated
///
/// # Examples
///
/// ```no_run
/// # use hiroz::action::*;
/// # use hiroz_msgs::action_tutorials_interfaces::{FibonacciResult, action::Fibonacci};
/// # let server: std::sync::Arc<server::ZActionServer<Fibonacci>> = todo!();
/// # async {
/// let requested = server.recv_goal().await?;
/// let accepted = requested.accept();
/// let executing = accepted.execute();
/// executing.succeed(FibonacciResult { sequence: vec![] })?;
/// # Ok::<(), zenoh::Error>(())
/// # };
/// ```
pub struct GoalHandle<A: ZAction, State> {
    /// The goal data.
    pub goal: A::Goal,
    /// The goal metadata.
    pub info: GoalInfo,
    pub(crate) server: ZActionServer<A>,
    pub(crate) cancel_flag: Option<Arc<AtomicBool>>,
    /// Per-goal cancel channel registered with the CancelDispatcher (Some only in Executing state).
    pub(crate) cancel_rx: Option<flume::Receiver<RoutedCancel>>,
    /// Drop cleanup; disarmed on `accept`/`execute` so type-state moves do not abort.
    pub(crate) cleanup: GoalCleanup<A>,
    pub(crate) _state: PhantomData<State>,
}

/// Aborts non-terminal goals (and rejects unanswered send_goal) if the handle is
/// dropped without an explicit terminal transition. Disarmed when ownership is
/// transferred via `accept` / `execute`.
pub(crate) struct GoalCleanup<A: ZAction> {
    pub(crate) goal_id: GoalId,
    pub(crate) server: ZActionServer<A>,
    pub(crate) query: Option<zenoh::query::Query>,
    pub(crate) armed: bool,
}

impl<A: ZAction> GoalCleanup<A> {
    pub(crate) fn new(
        goal_id: GoalId,
        server: ZActionServer<A>,
        query: Option<zenoh::query::Query>,
    ) -> Self {
        Self {
            goal_id,
            server,
            query,
            armed: true,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
        self.query = None;
    }
}

impl<A: ZAction> Drop for GoalCleanup<A> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        if let Some(query) = self.query.take() {
            let response = GoalResponse {
                accepted: false,
                stamp_sec: 0,
                stamp_nanosec: 0,
            };
            let response_bytes = <GoalResponse as ZMessage>::serialize(&response);
            if let Some(raw) = query.attachment()
                && let Ok(attachment) = Attachment::try_from(raw)
            {
                let _ = query
                    .reply(query.key_expr().clone(), response_bytes)
                    .attachment(attachment)
                    .wait();
            }
        }

        self.server.cancel_dispatcher().deregister(self.goal_id);

        let aborted =
            self.server
                .goal_manager()
                .modify(|manager| match manager.goals.get(&self.goal_id) {
                    Some(
                        ServerGoalState::Accepted { .. }
                        | ServerGoalState::Executing { .. }
                        | ServerGoalState::Canceling { .. },
                    ) => {
                        let info = manager.goals[&self.goal_id].info().clone();
                        let now = Instant::now();
                        let expires_at = Some(now + manager.result_timeout);
                        manager.goals.insert(
                            self.goal_id,
                            ServerGoalState::Terminated {
                                info,
                                result: A::Result::default(),
                                status: GoalStatus::Aborted,
                                timestamp: now,
                                expires_at,
                            },
                        );
                        Some(
                            manager
                                .result_futures
                                .remove(&self.goal_id)
                                .unwrap_or_default(),
                        )
                    }
                    _ => None,
                });

        if let Some(waiters) = aborted {
            tracing::warn!(
                goal_id = %self.goal_id,
                "GoalHandle dropped without succeed/abort/canceled; aborting"
            );
            for tx in waiters {
                let _ = tx.send((A::Result::default(), GoalStatus::Aborted));
            }
            self.server.publish_status();
        }
    }
}

// --- State-specific implementations ---

/// Methods available only for goals in the "Requested" state.
impl<A: ZAction> GoalHandle<A, Requested> {
    /// Access the goal data.
    pub fn goal(&self) -> &A::Goal {
        &self.goal
    }

    /// Access the goal info.
    pub fn info(&self) -> &GoalInfo {
        &self.info
    }

    /// Accept this goal and transition to the "Accepted" state.
    ///
    /// This sends an acceptance response to the client and updates the server state.
    pub fn accept(mut self) -> GoalHandle<A, Accepted> {
        self.info.stamp = Time::now();
        // Insert before replying — client may fire get_result before we'd register the goal.
        // Create cancel_flag early so cancel can race accept→execute without being lost.
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.server.goal_manager().modify(|manager| {
            let expires_at = manager.goal_timeout.map(|timeout| Instant::now() + timeout);
            manager.goals.insert(
                self.info.goal_id,
                ServerGoalState::Accepted {
                    info: self.info.clone(),
                    goal: self.goal.clone(),
                    cancel_flag: cancel_flag.clone(),
                    timestamp: Instant::now(),
                    expires_at,
                },
            );
        });

        // Send acceptance response
        // Use timestamp from GoalInfo which is already in sec/nanosec format
        let response = SendGoalResponse {
            accepted: true,
            stamp_sec: self.info.stamp.sec,
            stamp_nanosec: self.info.stamp.nanosec,
        };
        let response_bytes = <SendGoalResponse as ZMessage>::serialize(&response);

        if let Some(query) = self.cleanup.query.take() {
            let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
            // FIXME: address the result
            let _ = query
                .reply(query.key_expr().clone(), response_bytes)
                .attachment(attachment)
                .wait();
        }

        // Publish status update
        self.server.publish_status();

        // Disarm so Drop of this Requested handle does not abort the Accepted goal.
        let goal_id = self.info.goal_id;
        let server = self.server.clone();
        self.cleanup.disarm();

        GoalHandle {
            goal: self.goal,
            info: self.info,
            server: server.clone(),
            cancel_flag: Some(cancel_flag),
            cancel_rx: None,
            cleanup: GoalCleanup::new(goal_id, server, None),
            _state: PhantomData,
        }
    }

    /// Reject this goal.
    ///
    /// This sends a rejection response to the client. The goal will not be executed.
    pub fn reject(mut self) -> Result<()> {
        // Send rejection response
        let response = GoalResponse {
            accepted: false,
            stamp_sec: 0,
            stamp_nanosec: 0,
        };
        let response_bytes = <GoalResponse as ZMessage>::serialize(&response);

        if let Some(query) = self.cleanup.query.take() {
            // FIXME: Address the unwrap usage
            let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
            let _ = query
                .reply(query.key_expr().clone(), response_bytes)
                .attachment(attachment)
                .wait();
        }
        self.cleanup.disarm();
        Ok(())
    }
}

/// Methods available only for goals in the "Accepted" state.
impl<A: ZAction> GoalHandle<A, Accepted> {
    /// Access the goal data.
    pub fn goal(&self) -> &A::Goal {
        &self.goal
    }

    /// Access the goal info.
    pub fn info(&self) -> &GoalInfo {
        &self.info
    }

    /// Begin executing this goal and transition to the "Executing" state.
    ///
    /// This updates the server state to executing and publishes a status update.
    pub fn execute(mut self) -> GoalHandle<A, Executing> {
        // Register before changing the state, without holding the goal-manager lock.
        // Result and cancel requests must still find the accepted goal while this waits.
        let cancel_rx = self.server.cancel_dispatcher().register(self.info.goal_id);

        let cancel_flag = self.server.goal_manager().modify(|manager| {
            // Extract and replace under one lock so no request observes a missing goal.
            let (cancel_flag, expires_at) = match manager.goals.remove(&self.info.goal_id) {
                Some(ServerGoalState::Accepted {
                    cancel_flag,
                    expires_at,
                    ..
                })
                | Some(ServerGoalState::Canceling {
                    cancel_flag,
                    expires_at,
                    ..
                }) => (cancel_flag, expires_at),
                other => {
                    if let Some(state) = other {
                        manager.goals.insert(self.info.goal_id, state);
                    }
                    (
                        Arc::new(AtomicBool::new(false)),
                        manager.goal_timeout.map(|timeout| Instant::now() + timeout),
                    )
                }
            };

            // A cancel received while Accepted must survive the transition.
            if cancel_flag.load(Ordering::Relaxed) {
                manager.goals.insert(
                    self.info.goal_id,
                    ServerGoalState::Canceling {
                        info: self.info.clone(),
                        goal: self.goal.clone(),
                        cancel_flag: cancel_flag.clone(),
                        expires_at,
                    },
                );
            } else {
                manager.goals.insert(
                    self.info.goal_id,
                    ServerGoalState::Executing {
                        info: self.info.clone(),
                        goal: self.goal.clone(),
                        cancel_flag: cancel_flag.clone(),
                        expires_at,
                    },
                );
            }
            cancel_flag
        });

        self.server.publish_status();

        let goal_id = self.info.goal_id;
        let server = self.server.clone();
        self.cleanup.disarm();

        GoalHandle {
            goal: self.goal,
            info: self.info,
            server: server.clone(),
            cancel_flag: Some(cancel_flag),
            cancel_rx: Some(cancel_rx),
            cleanup: GoalCleanup::new(goal_id, server, None),
            _state: PhantomData,
        }
    }
}

/// Methods available only for goals in the "Executing" state.
impl<A: ZAction> GoalHandle<A, Executing> {
    /// Access the goal data.
    pub fn goal(&self) -> &A::Goal {
        &self.goal
    }

    /// Access the goal info.
    pub fn info(&self) -> &GoalInfo {
        &self.info
    }

    /// Wait until at least `count` feedback subscribers are active, or `timeout` elapses.
    ///
    /// Call this before the first `publish_feedback` to ensure the client's feedback
    /// subscriber is registered before publishing starts.  Returns `true` if the
    /// required number of subscribers became active within the timeout.
    pub async fn wait_for_feedback_subscriber(
        &self,
        count: usize,
        timeout: std::time::Duration,
    ) -> bool {
        self.server
            .feedback_pub()
            .wait_for_subscription(count, timeout)
            .await
    }

    /// Publish feedback for this goal.
    ///
    /// Feedback can be published multiple times during goal execution to inform
    /// the client of progress.
    pub fn publish_feedback(&self, feedback: A::Feedback) -> Result<()> {
        let msg = FeedbackMessage {
            goal_id: self.info.goal_id,
            feedback,
        };
        self.server.feedback_pub().publish(&msg)
    }

    /// Check if cancellation has been requested for this goal.
    ///
    /// This is a lock-free operation that can be called frequently from the
    /// goal execution loop.
    ///
    /// # Returns
    ///
    /// `true` if a cancel request has been received, `false` otherwise.
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_flag
            .as_ref()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Check for and process any pending cancel request for this goal (polling mode).
    ///
    /// Drains queued requests without waiting for a new one, applies their ROS
    /// UUID/time selection to every matching goal, and sends each reply. Reply
    /// publication itself may block. Returns whether this goal's cancellation
    /// flag is set; polling one goal also processes requests for other goals.
    /// Accepted goals are selectable before an execution handle is registered.
    pub fn try_process_cancel(&self) -> bool {
        // Even an already-canceling handle must process subsequent requests so
        // duplicate cancellation and cancellation of other goals receive replies.
        self.server.cancel_dispatcher().drain(&self.server);
        if let Some(receiver) = &self.cancel_rx {
            while receiver.try_recv().is_ok() {}
        }
        self.is_cancel_requested()
    }

    /// Mark this goal as succeeded with the given result.
    ///
    /// This transitions the goal to a terminal state and consumes the handle.
    pub fn succeed(self, result: A::Result) -> Result<()> {
        self.terminate(result, GoalStatus::Succeeded)
    }

    /// Mark this goal as aborted with the given result.
    ///
    /// This transitions the goal to a terminal state and consumes the handle.
    pub fn abort(self, result: A::Result) -> Result<()> {
        self.terminate(result, GoalStatus::Aborted)
    }

    /// Mark this goal as canceled with the given result.
    ///
    /// This transitions the goal to a terminal state and consumes the handle.
    pub fn canceled(self, result: A::Result) -> Result<()> {
        self.terminate(result, GoalStatus::Canceled)
    }

    fn terminate(mut self, result: A::Result, status: GoalStatus) -> Result<()> {
        // Ownership of termination; disarm so Drop does not double-abort.
        self.cleanup.disarm();

        // Deregister from the cancel dispatcher so no more cancel messages are routed here
        self.server
            .cancel_dispatcher()
            .deregister(self.info.goal_id);

        // Notify any waiting result futures
        let futures_to_notify = self.server.goal_manager().modify(|manager| {
            let now = Instant::now();
            let expires_at = Some(now + manager.result_timeout);
            manager.goals.insert(
                self.info.goal_id,
                ServerGoalState::Terminated {
                    info: self.info.clone(),
                    result: result.clone(),
                    status,
                    timestamp: now,
                    expires_at,
                },
            );

            // Take all waiting result futures for this goal
            manager
                .result_futures
                .remove(&self.info.goal_id)
                .unwrap_or_default()
        }); // Drop the lock before notifying futures and publishing status

        // Notify all waiting result futures
        for tx in futures_to_notify {
            let _ = tx.send((result.clone(), status));
        }

        self.server.publish_status();
        Ok(())
    }
}

#[cfg(test)]
mod transition_tests {
    use std::{sync::mpsc, time::Duration};

    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::context::ZContextBuilder;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TransitionGoal {
        value: i32,
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    struct TransitionResult {
        value: i32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TransitionFeedback;

    struct TransitionAction;
    crate::define_action! {
        TransitionAction,
        action_name: "execute_transition",
        Goal: TransitionGoal,
        Result: TransitionResult,
        Feedback: TransitionFeedback,
    }

    async fn result_during_execute(
        cancel_requested: bool,
        cancel_during_register: bool,
    ) -> Result<()> {
        let context = ZContextBuilder::default()
            .with_mode("peer")
            .disable_multicast_scouting()
            .with_json("connect/endpoints", Vec::<String>::new())
            .with_json("listen/endpoints", Vec::<String>::new())
            .with_json("scouting/gossip/enabled", false)
            .with_json("scouting/delay", 0)
            .build()?;
        let node = context.create_node("execute_transition").build()?;
        let server = node
            .create_action_server::<TransitionAction>("/execute_transition")
            .with_goal_timeout(Duration::from_secs(30))
            .build()?;
        let client = node
            .create_action_client::<TransitionAction>("/execute_transition")
            .build()?;
        let results = node
            .create_client::<ResultService<TransitionAction>>(
                "/execute_transition/_action/get_result",
            )
            .build()?;
        let (sent, accepted) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(client.send_goal(TransitionGoal { value: 42 }), async {
                server.recv_goal().await.map(GoalHandle::accept)
            })
        })
        .await
        .expect("local goal acceptance");
        let sent = sent?;
        let accepted = accepted?;
        let id = sent.id();
        if cancel_requested {
            assert!(server.request_cancel(id));
        }

        let original_expiry = server
            .goal_manager()
            .read(|manager| match manager.goals.get(&id) {
                Some(
                    ServerGoalState::Accepted { expires_at, .. }
                    | ServerGoalState::Canceling { expires_at, .. },
                ) => *expires_at,
                _ => panic!("goal must be accepted before execution"),
            });

        // Park execute at dispatcher registration. Before the fix it has
        // already removed the goal from the manager at this point, exposing
        // an UNKNOWN result to a concurrent caller. Keep the lock on its own
        // thread so no synchronous mutex guard is held across an await.
        let (locked_tx, locked_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let locked_server = server.clone();
        let locker = std::thread::spawn(move || {
            let _guard = locked_server.cancel_dispatcher().routes.lock();
            locked_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        locked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            accepted.execute()
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        // Observe the old missing-state window if present. The corrected
        // implementation leaves Accepted/Canceling visible while blocked.
        let missing = tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if server
                    .goal_manager()
                    .read(|manager| !manager.goals.contains_key(&id))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let cancellation_accepted = !cancel_during_register || server.request_cancel(id);
        let request = GetResultRequest { goal_id: id };
        let result = results.call_with_timeout(&request, Duration::from_secs(2));
        tokio::pin!(result);
        let early = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                tokio::select! {
                    response = &mut result => break Some(response),
                    _ = tokio::task::yield_now() => {
                        if server.goal_manager().read(|manager| {
                            manager.result_futures.contains_key(&id)
                        }) {
                            break None;
                        }
                    }
                }
            }
        })
        .await;

        // Always release and join the blocked threads before checking the
        // response, including when the unfixed server answered UNKNOWN.
        release_tx.send(()).unwrap();
        locker.join().unwrap();
        let executing = worker.join().unwrap();
        let flag_preserved = executing.is_cancel_requested();
        let expiry_preserved = server
            .goal_manager()
            .read(|manager| match manager.goals.get(&id) {
                Some(
                    ServerGoalState::Executing { expires_at, .. }
                    | ServerGoalState::Canceling { expires_at, .. },
                ) => *expires_at == original_expiry,
                _ => false,
            });
        let expected_cancellation = cancel_requested || cancel_during_register;
        let expected_status = if expected_cancellation {
            executing.canceled(TransitionResult { value: 42 })?;
            GoalStatus::Canceled
        } else {
            executing.abort(TransitionResult { value: 42 })?;
            GoalStatus::Aborted
        };
        let response = match early.expect("result request must reach the server") {
            Some(response) => response?,
            None => result.await?,
        };
        assert_eq!(
            (response.status, response.result.value),
            (expected_status as i8, 42),
            "GetResult observed a temporary missing goal during execute (missing={missing})"
        );
        assert!(!missing, "execute temporarily removed an accepted goal");
        assert!(
            cancellation_accepted,
            "cancel lost during execute registration"
        );
        assert_eq!(flag_preserved, expected_cancellation);
        assert!(expiry_preserved, "execute changed the original goal expiry");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_goal_stays_visible_while_execute_registers() -> Result<()> {
        result_during_execute(false, false).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_cancel_stays_visible_while_execute_registers() -> Result<()> {
        result_during_execute(true, false).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_during_execute_registration_is_preserved() -> Result<()> {
        result_during_execute(false, true).await
    }
}
