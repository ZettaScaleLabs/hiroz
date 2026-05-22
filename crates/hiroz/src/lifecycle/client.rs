//! Client for driving remote lifecycle nodes through their state transitions.
//!
//! `ZLifecycleClient` connects to a lifecycle node's management services and
//! allows you to orchestrate its state machine from Rust code — the building
//! block for bringup managers and system orchestrators.
//!
//! # Example
//!
//! ```rust,ignore
//! let ctx = ZContextBuilder::default().build()?;
//! let mgr = ctx.create_node("lifecycle_manager").build()?;
//!
//! let client = ZLifecycleClient::new(&mgr, "/camera_node")?;
//!
//! client.configure(Duration::from_secs(5)).await?;
//! client.activate(Duration::from_secs(5)).await?;
//!
//! // … node is now Active and publishing …
//!
//! client.deactivate(Duration::from_secs(5)).await?;
//! client.shutdown(Duration::from_secs(5)).await?;
//! ```

use std::time::Duration;

use crate::{
    Builder, Result,
    lifecycle::{
        LifecycleState, TransitionId,
        msgs::{
            ChangeState, ChangeStateRequest, GetAvailableStates, GetAvailableStatesRequest,
            GetAvailableTransitions, GetAvailableTransitionsRequest, GetState, GetStateRequest,
            LcState, LcTransitionDescription,
        },
    },
    node::ZNode,
    service::ZClient,
};

/// Client for managing a remote lifecycle node's state machine.
///
/// Each method maps to one of the five standard lifecycle management services.
/// The node name passed to [`ZLifecycleClient::new`] must match the name the
/// lifecycle node was created with (including namespace).
pub struct ZLifecycleClient {
    change_state: ZClient<ChangeState>,
    get_state: ZClient<GetState>,
    get_available_states: ZClient<GetAvailableStates>,
    get_available_transitions: ZClient<GetAvailableTransitions>,
}

impl ZLifecycleClient {
    /// Create a new lifecycle client targeting `node_name`.
    ///
    /// Services are resolved relative to the node name:
    /// `{node_name}/change_state`, `{node_name}/get_state`, etc.
    pub fn new(node: &ZNode, node_name: &str) -> Result<Self> {
        let change_state = node
            .create_client::<ChangeState>(&format!("{node_name}/change_state"))
            .build()?;
        let get_state = node
            .create_client::<GetState>(&format!("{node_name}/get_state"))
            .build()?;
        let get_available_states = node
            .create_client::<GetAvailableStates>(&format!("{node_name}/get_available_states"))
            .build()?;
        let get_available_transitions = node
            .create_client::<GetAvailableTransitions>(&format!(
                "{node_name}/get_available_transitions"
            ))
            .build()?;
        Ok(Self {
            change_state,
            get_state,
            get_available_states,
            get_available_transitions,
        })
    }

    // -----------------------------------------------------------------------
    // High-level transition helpers
    // -----------------------------------------------------------------------

    /// Trigger the `configure` transition (Unconfigured → Inactive).
    pub async fn configure(&self, timeout: Duration) -> Result<bool> {
        self.trigger(TransitionId::Configure, timeout).await
    }

    /// Trigger the `activate` transition (Inactive → Active).
    pub async fn activate(&self, timeout: Duration) -> Result<bool> {
        self.trigger(TransitionId::Activate, timeout).await
    }

    /// Trigger the `deactivate` transition (Active → Inactive).
    pub async fn deactivate(&self, timeout: Duration) -> Result<bool> {
        self.trigger(TransitionId::Deactivate, timeout).await
    }

    /// Trigger the `cleanup` transition (Inactive → Unconfigured).
    pub async fn cleanup(&self, timeout: Duration) -> Result<bool> {
        self.trigger(TransitionId::Cleanup, timeout).await
    }

    /// Trigger `shutdown` from any primary state (→ Finalized).
    ///
    /// Sends the appropriate shutdown transition ID for the node's current
    /// state. Returns `Ok(true)` if the node acknowledged the transition.
    pub async fn shutdown(&self, timeout: Duration) -> Result<bool> {
        let transition_id = match self.get_state(timeout).await? {
            LifecycleState::Unconfigured => TransitionId::UnconfiguredShutdown,
            LifecycleState::Active => TransitionId::ActiveShutdown,
            _ => TransitionId::InactiveShutdown,
        };
        self.trigger(transition_id, timeout).await
    }

    // -----------------------------------------------------------------------
    // Low-level transition trigger
    // -----------------------------------------------------------------------

    /// Trigger an arbitrary transition by [`TransitionId`].
    ///
    /// Returns `Ok(true)` when the lifecycle node accepted the transition.
    pub async fn trigger(&self, transition: TransitionId, timeout: Duration) -> Result<bool> {
        let req = ChangeStateRequest {
            transition: crate::lifecycle::msgs::LcTransition {
                id: transition as u8,
                label: String::new(),
            },
        };
        let resp = self.change_state.call_with_timeout(&req, timeout).await?;
        Ok(resp.success)
    }

    // -----------------------------------------------------------------------
    // Query services
    // -----------------------------------------------------------------------

    /// Query the current state of the remote lifecycle node.
    pub async fn get_state(&self, timeout: Duration) -> Result<LifecycleState> {
        let resp = self
            .get_state
            .call_with_timeout(&GetStateRequest {}, timeout)
            .await?;
        Ok(state_from_lc(&resp.current_state))
    }

    /// List all states in the lifecycle state machine.
    pub async fn get_available_states(&self, timeout: Duration) -> Result<Vec<LcState>> {
        let resp = self
            .get_available_states
            .call_with_timeout(&GetAvailableStatesRequest {}, timeout)
            .await?;
        Ok(resp.available_states)
    }

    /// List transitions valid from the node's current state.
    pub async fn get_available_transitions(
        &self,
        timeout: Duration,
    ) -> Result<Vec<LcTransitionDescription>> {
        let resp = self
            .get_available_transitions
            .call_with_timeout(&GetAvailableTransitionsRequest {}, timeout)
            .await?;
        Ok(resp.available_transitions)
    }
}

fn state_from_lc(s: &LcState) -> LifecycleState {
    match s.id {
        1 => LifecycleState::Unconfigured,
        2 => LifecycleState::Inactive,
        3 => LifecycleState::Active,
        4 => LifecycleState::Finalized,
        _ => LifecycleState::Unconfigured,
    }
}
