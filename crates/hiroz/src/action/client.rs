//! Action client implementation for ROS 2 actions.
//!
//! This module provides the client-side functionality for ROS 2 actions,
//! allowing nodes to send goals to action servers, receive feedback,
//! monitor goal status, and retrieve results.

use std::{marker::PhantomData, sync::Arc};

use dashmap::DashMap;
use tokio::sync::{mpsc, watch};
use zenoh::Result;

use super::{GoalId, GoalInfo, GoalStatus, Time, ZAction, messages::*};
use crate::{
    Builder, entity::TypeInfo, msg::ZMessage, qos::QosProfile, topic_name::qualify_topic_name,
};

/// Type states for goal handles.
pub mod goal_state {
    /// The goal is active and can be monitored or canceled.
    pub struct Active;
    /// The goal has been terminated and cannot be used further.
    pub struct Terminated;
}

/// Builder for creating an action client.
///
/// The `ZActionClientBuilder` allows you to configure QoS settings for different
/// action communication channels before building the client.
///
/// # Examples
///
/// ```no_run
/// # use hiroz::action::*;
/// # use hiroz::qos::QosProfile;
/// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
/// # let node: hiroz::node::ZNode = todo!();
/// let client = node.create_action_client::<Fibonacci>("fibonacci")
///     .with_goal_service_qos(QosProfile::default())
///     .build()?;
/// # Ok::<(), zenoh::Error>(())
/// ```
pub struct ZActionClientBuilder<'a, A: ZAction> {
    /// The name of the action.
    pub action_name: String,
    /// Reference to the node that will own this client.
    pub node: &'a crate::node::ZNode,
    /// QoS profile for the goal service.
    pub goal_service_qos: Option<QosProfile>,
    /// QoS profile for the result service.
    pub result_service_qos: Option<QosProfile>,
    /// QoS profile for the cancel service.
    pub cancel_service_qos: Option<QosProfile>,
    /// QoS profile for the feedback topic.
    pub feedback_topic_qos: Option<QosProfile>,
    /// QoS profile for the status topic.
    pub status_topic_qos: Option<QosProfile>,
    /// Override for goal (send_goal) type info; uses `A::send_goal_type_info()` if None.
    pub goal_type_info: Option<TypeInfo>,
    /// Override for result (get_result) type info; uses `A::get_result_type_info()` if None.
    pub result_type_info: Option<TypeInfo>,
    /// Override for feedback type info; uses `A::feedback_type_info()` if None.
    pub feedback_type_info: Option<TypeInfo>,
    /// Phantom data for the action type and backend.
    pub _phantom: std::marker::PhantomData<A>,
}

impl<'a, A: ZAction> ZActionClientBuilder<'a, A> {
    pub fn with_goal_service_qos(mut self, qos: QosProfile) -> Self {
        self.goal_service_qos = Some(qos);
        self
    }

    pub fn with_result_service_qos(mut self, qos: QosProfile) -> Self {
        self.result_service_qos = Some(qos);
        self
    }

    pub fn with_cancel_service_qos(mut self, qos: QosProfile) -> Self {
        self.cancel_service_qos = Some(qos);
        self
    }

    pub fn with_feedback_topic_qos(mut self, qos: QosProfile) -> Self {
        self.feedback_topic_qos = Some(qos);
        self
    }

    pub fn with_status_topic_qos(mut self, qos: QosProfile) -> Self {
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

impl<'a, A: ZAction> ZActionClientBuilder<'a, A> {
    pub fn new(action_name: &str, node: &'a crate::node::ZNode) -> Self {
        Self {
            action_name: action_name.to_string(),
            node,
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

impl<'a, A: ZAction> Builder for ZActionClientBuilder<'a, A> {
    type Output = ZActionClient<A>;

    fn build(self) -> Result<Self::Output> {
        // Apply remapping to action name
        let action_name = self.node.remap_rules.apply(&self.action_name);

        // Validate action name is not empty
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

        // Create goal client using node API for proper graph registration
        // Use override if provided, otherwise fall back to the action's static type info.
        let goal_type_info = Some(self.goal_type_info.unwrap_or_else(A::send_goal_type_info));
        let mut goal_client_builder = self
            .node
            .create_client_impl::<GoalService<A>>(&goal_service_name, goal_type_info);
        if let Some(qos) = self.goal_service_qos {
            goal_client_builder.entity.qos = qos.to_protocol_qos();
        }
        let goal_client = goal_client_builder.build()?;

        // Create result client using node API for proper graph registration
        let result_type_info = Some(
            self.result_type_info
                .unwrap_or_else(A::get_result_type_info),
        );
        let mut result_client_builder = self
            .node
            .create_client_impl::<ResultService<A>>(&result_service_name, result_type_info)
            .with_querier_timeout(std::time::Duration::MAX);
        if let Some(qos) = self.result_service_qos {
            result_client_builder.entity.qos = qos.to_protocol_qos();
        }
        let result_client = result_client_builder.build()?;
        tracing::debug!("Created result client for: {}", result_service_name);

        // Create cancel client using node API for proper graph registration
        // Use the action's cancel_goal_type_info for proper ROS 2 interop
        let cancel_type_info = Some(A::cancel_goal_type_info());
        let mut cancel_client_builder = self
            .node
            .create_client_impl::<CancelService<A>>(&cancel_service_name, cancel_type_info);
        if let Some(qos) = self.cancel_service_qos {
            cancel_client_builder.entity.qos = qos.to_protocol_qos();
        }
        let cancel_client = cancel_client_builder.build()?;

        let goal_board = Arc::new(GoalBoard {
            active_goals: DashMap::new(),
        });

        // Create feedback subscriber with callback for proper graph registration
        let feedback_type_info = Some(
            self.feedback_type_info
                .unwrap_or_else(A::feedback_type_info),
        );
        let mut feedback_sub_builder = self
            .node
            .create_sub_impl::<FeedbackMessage<A>>(&feedback_topic_name, feedback_type_info);
        if let Some(qos) = self.feedback_topic_qos {
            feedback_sub_builder.entity.qos = qos.to_protocol_qos();
        }
        tracing::debug!(
            "Creating feedback subscriber with callback for {}",
            feedback_topic_name
        );
        let goal_board_feedback = goal_board.clone();
        let feedback_sub =
            feedback_sub_builder.build_with_callback(move |msg: FeedbackMessage<A>| {
                tracing::trace!("Feedback callback received for goal {:?}", msg.goal_id);
                if let Some(channels) = goal_board_feedback.active_goals.get(&msg.goal_id) {
                    tracing::trace!("Routing feedback to goal {:?}", msg.goal_id);
                    let _ = channels.feedback_tx.send(msg.feedback);
                } else {
                    tracing::warn!("No active goal found for feedback {:?}", msg.goal_id);
                }
            })?;
        tracing::debug!("Feedback subscriber created successfully");

        // Create status subscriber with callback for direct message routing
        // Use the action's status_type_info for proper ROS 2 interop
        let status_type_info = Some(A::status_type_info());
        let mut status_sub_builder = self
            .node
            .create_sub_impl::<StatusMessage>(&status_topic_name, status_type_info);
        if let Some(qos) = self.status_topic_qos {
            status_sub_builder.entity.qos = qos.to_protocol_qos();
        }
        let goal_board_status = goal_board.clone();
        let status_sub = status_sub_builder.build_with_callback(move |msg: StatusMessage| {
            tracing::trace!(
                "Status callback received with {} statuses",
                msg.status_list.len()
            );
            for status_info in msg.status_list {
                if let Some(channels) = goal_board_status
                    .active_goals
                    .get(&status_info.goal_info.goal_id)
                {
                    tracing::trace!(
                        "Routing status {:?} to goal {:?}",
                        status_info.status,
                        status_info.goal_info.goal_id
                    );
                    let _ = channels.status_tx.send(status_info.status);
                } else {
                    tracing::trace!(
                        "No active goal found for status {:?}",
                        status_info.goal_info.goal_id
                    );
                }
            }
        })?;

        Ok(ZActionClient {
            action_name: qualified_action_name,
            graph: self.node.graph.clone(),
            goal_client: Arc::new(goal_client),
            result_client: Arc::new(result_client),
            cancel_client: Arc::new(cancel_client),
            feedback_sub: Arc::new(feedback_sub),
            status_sub: Arc::new(status_sub),
            goal_board,
        })
    }
}

/// An action client for sending goals to an action server.
///
/// The `ZActionClient` allows you to send goals, receive feedback,
/// monitor status, and request results from an action server.
///
/// # Simple Goal Send/Receive
///
/// ```no_run
/// # use hiroz::action::*;
/// # use hiroz_msgs::action_tutorials_interfaces::{FibonacciGoal, action::Fibonacci};
/// # #[tokio::main]
/// # async fn main() -> zenoh::Result<()> {
/// # let node: hiroz::node::ZNode = todo!();
/// let client = node.create_action_client::<Fibonacci>("fibonacci").build()?;
/// let goal_handle = client.send_goal(FibonacciGoal { order: 42 }).await?;
/// let result = goal_handle.result().await?;
/// println!("Sequence: {:?}", result.sequence);
/// # Ok(())
/// # }
/// ```
///
/// # Feedback Streaming
///
/// ```no_run
/// # use hiroz::action::*;
/// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
/// # #[tokio::main]
/// # async fn main() -> zenoh::Result<()> {
/// # let mut goal_handle: GoalHandle<Fibonacci, goal_state::Active> = todo!();
/// let mut feedback_rx = goal_handle.feedback().unwrap();
/// tokio::spawn(async move {
///     while let Some(feedback) = feedback_rx.recv().await {
///         println!("Partial sequence: {:?}", feedback.partial_sequence);
///     }
/// });
/// # Ok(())
/// # }
/// ```
///
/// # Cancellation
///
/// ```no_run
/// # use hiroz::action::*;
/// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
/// # #[tokio::main]
/// # async fn main() -> zenoh::Result<()> {
/// # let client: ZActionClient<Fibonacci> = todo!();
/// # let goal_handle: GoalHandle<Fibonacci, goal_state::Active> = todo!();
/// client.cancel_goal(goal_handle.id()).await?;
/// client.cancel_all_goals().await?;
/// # Ok(())
/// # }
/// ```
pub struct ZActionClient<A: ZAction> {
    action_name: String,
    graph: Arc<crate::graph::Graph>,
    goal_client: Arc<crate::service::ZClient<GoalService<A>>>,
    result_client: Arc<crate::service::ZClient<ResultService<A>>>,
    cancel_client: Arc<crate::service::ZClient<CancelService<A>>>,
    feedback_sub:
        Arc<crate::pubsub::ZSub<FeedbackMessage<A>, (), <FeedbackMessage<A> as ZMessage>::Serdes>>,
    status_sub: Arc<crate::pubsub::ZSub<StatusMessage, (), <StatusMessage as ZMessage>::Serdes>>,
    goal_board: Arc<GoalBoard<A>>,
}

impl<A: ZAction> std::fmt::Debug for ZActionClient<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZActionClient")
            .field("goal_client", &self.goal_client)
            .finish_non_exhaustive()
    }
}

// This saves the condition A: Clone if using #[derive(Clone)]
impl<A: ZAction> Clone for ZActionClient<A> {
    fn clone(&self) -> Self {
        Self {
            action_name: self.action_name.clone(),
            graph: self.graph.clone(),
            goal_client: self.goal_client.clone(),
            result_client: self.result_client.clone(),
            cancel_client: self.cancel_client.clone(),
            feedback_sub: self.feedback_sub.clone(),
            status_sub: self.status_sub.clone(),
            goal_board: self.goal_board.clone(),
        }
    }
}

impl<A: ZAction> ZActionClient<A> {
    /// Wait until the action server is fully available.
    pub async fn wait_for_server(&self, timeout: std::time::Duration) -> bool {
        self.graph
            .wait_for_action_server(self.action_name.as_str(), timeout)
            .await
    }

    /// Sends a goal to the action server.
    ///
    /// This method sends a goal to the action server and returns a `GoalHandle`
    /// that can be used to monitor the goal's progress, receive feedback,
    /// and retrieve the result.
    ///
    /// # Arguments
    ///
    /// * `goal` - The goal to send to the server.
    ///
    /// # Returns
    ///
    /// Returns a `GoalHandle` if the goal is accepted, or an error if rejected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hiroz::action::*;
    /// # use hiroz_msgs::action_tutorials_interfaces::{FibonacciGoal, action::Fibonacci};
    /// # #[tokio::main]
    /// # async fn main() -> zenoh::Result<()> {
    /// # let client: ZActionClient<Fibonacci> = todo!();
    /// let goal_handle = client.send_goal(FibonacciGoal { order: 42 }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_goal(&self, goal: A::Goal) -> Result<GoalHandle<A, goal_state::Active>> {
        let goal_id = GoalId::new();

        // 1. Create channels for this goal
        let (feedback_tx, feedback_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(GoalStatus::Unknown);

        // 2. Insert into board (Lock-Free)
        self.goal_board.active_goals.insert(
            goal_id,
            GoalChannels {
                feedback_tx,
                status_tx,
            },
        );

        // 3. Send goal request via service client
        let request = SendGoalRequest { goal_id, goal };
        tracing::debug!("Sending goal request for goal_id: {:?}", goal_id);
        let response = match self.goal_client.call(&request).await {
            Ok(response) => response,
            Err(error) => {
                self.goal_board.active_goals.remove(&goal_id);
                return Err(error);
            }
        };

        // 5. Check if accepted
        if !response.accepted {
            // Cleanup on rejection
            self.goal_board.active_goals.remove(&goal_id);
            return Err(zenoh::Error::from("Goal rejected".to_string()));
        }

        // 6. Seed status watch with Accepted.
        //
        // The RPC response already confirms acceptance; the status topic message
        // travels a separate async pub/sub path and may arrive later. Setting
        // Accepted here ensures the caller sees a consistent initial status
        // regardless of pub/sub delivery timing.  Use send_if_modified so a
        // concurrent status delivery that already advanced beyond Accepted
        // (unlikely but possible) is not overwritten.
        if let Some(channels) = self.goal_board.active_goals.get(&goal_id) {
            channels.status_tx.send_if_modified(|s| {
                if *s == GoalStatus::Unknown {
                    *s = GoalStatus::Accepted;
                    true
                } else {
                    false
                }
            });
        }

        // 7. Return typed handle in Active state
        Ok(GoalHandle {
            id: goal_id,
            client: Arc::new(self.clone()),
            feedback_rx: Some(feedback_rx),
            status_rx: Some(status_rx),
            _state: PhantomData,
        })
    }

    pub async fn cancel_goal(&self, goal_id: GoalId) -> Result<CancelGoalServiceResponse> {
        let goal_info = GoalInfo::new(goal_id);
        let request = CancelGoalServiceRequest { goal_info };

        self.cancel_client.call(&request).await
    }

    pub async fn cancel_all_goals(&self) -> Result<CancelGoalServiceResponse> {
        // NOTE: ROS 2 convention: zero UUID + zero timestamp means "cancel all"
        let zero_goal_id = GoalId([0u8; 16]);
        let goal_info = GoalInfo {
            goal_id: zero_goal_id,
            stamp: Time::zero(),
        };
        let request = CancelGoalServiceRequest { goal_info };

        self.cancel_client.call(&request).await
    }

    pub fn feedback_stream(&self, goal_id: GoalId) -> Option<mpsc::UnboundedReceiver<A::Feedback>> {
        self.goal_board
            .active_goals
            .get_mut(&goal_id)
            .map(|mut channels| {
                // Create new receiver (old one already taken via GoalHandle)
                let (tx, rx) = mpsc::unbounded_channel();
                channels.feedback_tx = tx;
                rx
            })
    }

    pub fn status_watch(&self, goal_id: GoalId) -> Option<watch::Receiver<GoalStatus>> {
        self.goal_board
            .active_goals
            .get(&goal_id)
            .map(|channels| channels.status_tx.subscribe())
    }

    pub async fn get_result(&self, goal_id: GoalId) -> Result<A::Result> {
        let request = GetResultRequest { goal_id };

        let response: GetResultResponse<A> = self.result_client.call(&request).await?;

        Ok(response.result)
    }
}

/// The Goal Board (Lock-Free)
///
/// DashMap handles concurrent access safely and efficiently without blocking.
struct GoalBoard<A: ZAction> {
    active_goals: DashMap<GoalId, GoalChannels<A>>,
}

struct GoalChannels<A: ZAction> {
    feedback_tx: mpsc::UnboundedSender<A::Feedback>,
    status_tx: watch::Sender<GoalStatus>,
}

/// Handle for monitoring and controlling an active goal.
///
/// A `GoalHandle` is returned when a goal is successfully sent to an action server.
/// It provides methods to monitor the goal's status, receive feedback, retrieve results,
/// and cancel the goal.
///
/// The handle uses a type-state pattern to ensure goals cannot be misused:
/// - `GoalHandle<A, goal_state::Active>` - Can be monitored, cancelled, or consumed for result
/// - `GoalHandle<A, goal_state::Terminated>` - Read-only access after completion
///
/// # Examples
///
/// ```no_run
/// # use hiroz::action::*;
/// # use hiroz_msgs::action_tutorials_interfaces::action::Fibonacci;
/// # #[tokio::main]
/// # async fn main() -> zenoh::Result<()> {
/// # let mut goal_handle: GoalHandle<Fibonacci, goal_state::Active> = todo!();
/// let mut status_watch = goal_handle.status_watch().unwrap();
/// while let Ok(()) = status_watch.changed().await {
///     println!("Status: {:?}", *status_watch.borrow());
/// }
/// let result = goal_handle.result().await?;
/// # Ok(())
/// # }
/// ```
pub struct GoalHandle<A: ZAction, State = goal_state::Active> {
    /// Unique identifier for this goal.
    id: GoalId,
    /// Reference to the client that sent this goal.
    client: Arc<ZActionClient<A>>,
    /// Receiver for feedback messages.
    feedback_rx: Option<mpsc::UnboundedReceiver<A::Feedback>>,
    /// Receiver for status updates.
    status_rx: Option<watch::Receiver<GoalStatus>>,
    /// Type-state marker
    _state: PhantomData<State>,
}

// --- Active State Methods ---
impl<A: ZAction> GoalHandle<A, goal_state::Active> {
    /// Returns the unique identifier for this goal.
    ///
    /// # Returns
    ///
    /// The `GoalId` assigned to this goal when it was sent.
    pub fn id(&self) -> GoalId {
        self.id
    }

    /// Takes ownership of the feedback receiver.
    ///
    /// Returns `Some` the first time it's called, `None` afterwards.
    pub fn feedback(&mut self) -> Option<mpsc::UnboundedReceiver<A::Feedback>> {
        self.feedback_rx.take()
    }

    /// Takes ownership of the status watcher.
    ///
    /// Returns `Some` the first time it's called, `None` afterwards.
    pub fn status_watch(&mut self) -> Option<watch::Receiver<GoalStatus>> {
        self.status_rx.take()
    }

    /// Requests cancellation of this goal.
    ///
    /// # Returns
    ///
    /// The cancellation response from the server.
    pub async fn cancel(&self) -> Result<CancelGoalServiceResponse> {
        self.client.cancel_goal(self.id).await
    }

    /// Consumes the Active handle to prevent reuse.
    ///
    /// Waits for the goal to reach a terminal state, fetches the result,
    /// and cleans up the goal from the board. This is crucial for memory safety.
    ///
    /// # Returns
    ///
    /// The result of the action once it completes.
    pub async fn result(self) -> Result<A::Result> {
        // Skip status wait — go directly to get_result. The get_result
        // queryable handles both cases (returns immediately if the goal is
        // already terminated, or blocks until done). Relying on the status
        // subscription to gate get_result breaks when the server is on an
        // older zenoh version where transient_local pub/sub via router is
        // not reliable (e.g. zenoh-c 1.6.2 PEER pub → zenoh 1.9.0 CLIENT sub).

        // Fetch result. The server's get_result handler will either:
        // - Return immediately if the goal is already terminated
        // - Block until termination otherwise
        let res = self.client.get_result(self.id).await;

        // Cleanup Board (Crucial for Memory Safety)
        self.client.goal_board.active_goals.remove(&self.id);

        res
    }
}
