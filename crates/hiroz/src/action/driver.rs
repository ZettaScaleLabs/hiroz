//! Unified driver loop for action server event handling.
//!
//! This module provides a single event loop that handles all server-side
//! action protocol events (goal requests, cancel requests, result requests)
//! in a sequential, race-condition-free manner.

use std::{
    future::Future,
    marker::PhantomData,
    sync::{Arc, Weak},
    time::Duration,
};

use tokio::{task::JoinSet, time};
use tokio_util::sync::CancellationToken;
use zenoh::Wait;

use super::{
    GoalInfo, ZAction,
    messages::*,
    server::{Executing, GoalHandle, InnerServer, Requested, ZActionServer},
    state::ServerGoalState,
};
use crate::{attachment::Attachment, msg::ZMessage};

/// Runs the unified driver loop for an action server with automatic goal handling.
///
/// This function consolidates all protocol logic into a single event loop,
/// eliminating race conditions and reducing task overhead.
///
/// # Arguments
///
/// * `weak_inner` - Weak reference to the inner server state
/// * `shutdown` - Cancellation token to stop the driver loop
/// * `handler` - Callback to execute goals automatically
pub(crate) async fn run_driver_loop<A, F, Fut>(
    weak_inner: Weak<InnerServer<A>>,
    shutdown: CancellationToken,
    handler: F,
) where
    A: ZAction,
    F: Fn(GoalHandle<A, Executing>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tracing::debug!("Action Server Driver Loop Started");

    // Try to upgrade the weak reference once at the start
    let Some(inner) = weak_inner.upgrade() else {
        tracing::debug!("Server already dropped, not starting driver loop");
        return;
    };

    let handler = Arc::new(handler);

    // Create a timer for periodic expiration checking (every 1 second)
    let mut expiration_timer = time::interval(Duration::from_secs(1));
    expiration_timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    // STRUCTURED CONCURRENCY: Track all spawned goal tasks here
    let mut goal_tasks = JoinSet::new();

    loop {
        tokio::select! {
            // 1. Priority: Shutdown
            _ = shutdown.cancelled() => {
                tracing::debug!("Shutdown signal received. Aborting all goal tasks.");
                // This sends a cancellation signal to all running futures in the set
                goal_tasks.abort_all();
                break;
            }

            // 2. Reap Finished Tasks (Zombie Prevention)
            // This line is crucial. It removes finished tasks from memory.
            Some(res) = goal_tasks.join_next() => {
                if let Err(e) = res {
                    if e.is_cancelled() {
                        tracing::debug!("Goal task was cancelled");
                    } else if e.is_panic() {
                        tracing::error!("Goal task panicked!");
                    }
                }
            }

            // 3. Goal Expiration Timer
            _ = expiration_timer.tick() => {
                // Check for expired goals and clean them up
                let server = ZActionServer::from_inner(Arc::clone(&inner));
                let expired_goals = server.expire_goals();
                if !expired_goals.is_empty() {
                    tracing::debug!("Expired {} goals: {:?}", expired_goals.len(), expired_goals);
                }
            }

            // 4. New Goal Requests
            query = inner.goal_server.queue().recv_async() => {
                let inner = inner.clone();
                let handler = handler.clone();

                // Spawn into the SET, not globally detached
                goal_tasks.spawn(async move {
                    // This is now safe. If it hangs, abort_all() kills it.
                    handle_goal_request(inner, query, handler).await;
                });
            }

            // 5. Cancel Requests — must stay responsive while get_result waiters run.
            query = inner.cancel_server.queue().recv_async() => {
                let inner = inner.clone();
                goal_tasks.spawn(async move {
                    handle_cancel_request(&inner, query).await;
                });
            }

            // 6. Result Requests — MUST NOT block the driver loop. ros2 clients
            // call get_result immediately after accept and hold it open for the
            // whole goal. Awaiting that here starves cancel_goal + further
            // send_goals (queries arrive at the service layer but sit in queue).
            query = inner.result_server.queue().recv_async() => {
                let inner = inner.clone();
                goal_tasks.spawn(async move {
                    handle_result_request(&inner, query).await;
                });
            }
        }
    }

    // Ensure everything is dead before we exit
    while goal_tasks.join_next().await.is_some() {}
    tracing::debug!("Action Server Driver Loop Stopped");
}

/// Handles incoming goal requests.
async fn handle_goal_request<A, F, Fut>(
    inner: Arc<InnerServer<A>>,
    query: zenoh::query::Query,
    handler: Arc<F>,
) where
    A: ZAction,
    F: Fn(GoalHandle<A, Executing>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tracing::debug!("Received goal request");
    let payload = query.payload().unwrap().to_bytes();
    let request = match <GoalRequest<A> as ZMessage>::deserialize(&payload) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to deserialize goal request: {}", e);
            return;
        }
    };

    // Create a temporary ZActionServer handle for the goal handle
    // This is safe because we're just passing it to the goal handler
    let server = ZActionServer::from_inner(Arc::clone(&inner));

    let requested = GoalHandle {
        goal: request.goal,
        info: GoalInfo::new(request.goal_id),
        server: server.clone(),
        cancel_flag: None,
        cancel_rx: None,
        cleanup: crate::action::server::GoalCleanup::new(request.goal_id, server, Some(query)),
        _state: PhantomData::<Requested>,
    };

    // accept() does a blocking zenoh reply wait. Run it off the async worker
    // so a stuck reply (dead client / uplink blip) cannot starve the driver
    // loop from processing cancel_goal / get_result / further send_goals.
    let accepted = match tokio::task::spawn_blocking(move || requested.accept()).await {
        Ok(accepted) => accepted,
        Err(e) => {
            tracing::error!("send_goal accept task failed: {e}");
            return;
        }
    };
    let executing = accepted.execute();

    // Execute the user's handler
    // No tokio::select! needed anymore. If the driver loop aborts this task,
    // this await simply acts as a cancellation point.
    handler(executing).await;
}

/// Handles incoming cancel requests.
async fn handle_cancel_request<A: ZAction>(
    inner: &Arc<InnerServer<A>>,
    query: zenoh::query::Query,
) {
    tracing::debug!("Received cancel request");
    let payload = query.payload().unwrap().to_bytes();
    let request = match <CancelGoalServiceRequest as ZMessage>::deserialize(&payload) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to deserialize cancel request: {}", e);
            return;
        }
    };

    let server = ZActionServer::from_inner(Arc::clone(inner));

    // Zero UUID = cancel all (ROS 2 / `ros2 action cancel` convention).
    // Specific UUID = cancel that executing/canceling goal only.
    // `request_cancel` also transitions Executing → Canceling for status.
    let goals_canceling = if !request.goal_info.goal_id.is_valid() {
        let ids: Vec<super::GoalId> = inner
            .goal_manager
            .read(|manager| manager.goals.keys().copied().collect());
        let mut out = Vec::new();
        for goal_id in ids {
            if server.request_cancel(goal_id) {
                out.push(GoalInfo::new(goal_id));
            }
        }
        out
    } else if server.request_cancel(request.goal_info.goal_id) {
        vec![request.goal_info.clone()]
    } else {
        vec![]
    };

    tracing::info!(
        count = goals_canceling.len(),
        specific = request.goal_info.goal_id.is_valid(),
        "action cancel request processed"
    );

    // ERROR_NONE (0) if at least one goal is canceling; ERROR_REJECTED (1) otherwise.
    let response = CancelGoalServiceResponse {
        return_code: if goals_canceling.is_empty() { 1 } else { 0 },
        goals_canceling,
    };

    let response_bytes = <CancelGoalServiceResponse as ZMessage>::serialize(&response);
    let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
    // Blocking zenoh wait off the async worker (same rationale as send_goal accept).
    let _ = tokio::task::spawn_blocking(move || {
        let _ = query
            .reply(query.key_expr().clone(), response_bytes)
            .attachment(attachment)
            .wait();
    })
    .await;

    tracing::debug!("Sent cancel response");
}

/// Handles incoming result requests.
async fn handle_result_request<A: ZAction>(
    inner: &Arc<InnerServer<A>>,
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

    // Check if goal is already terminated, or register a waiter
    let (tx, rx) = tokio::sync::oneshot::channel();
    enum ResultState {
        Terminated,
        Waiting,
        NotFound,
    }

    let (result_state, result_data) = inner.goal_manager.modify(|manager| {
        if let Some(ServerGoalState::Terminated { result, status, .. }) =
            manager.goals.get(&request.goal_id)
        {
            // Goal is already terminated - return result immediately
            (ResultState::Terminated, Some((result.clone(), *status)))
        } else if manager.goals.contains_key(&request.goal_id) {
            // Goal exists but not terminated yet - register waiter
            manager
                .result_futures
                .entry(request.goal_id)
                .or_default()
                .push(tx);
            (ResultState::Waiting, None)
        } else {
            // Goal doesn't exist
            (ResultState::NotFound, None)
        }
    }); // Lock released here

    let (result, status) = match result_state {
        ResultState::Terminated => {
            let (r, s) = result_data.unwrap();
            tracing::debug!(
                "Goal {:?} is already terminated with status {:?}",
                request.goal_id,
                s
            );
            (r, s)
        }
        ResultState::Waiting => {
            // Wait for goal to complete
            tracing::debug!(
                "Goal {:?} not terminated yet, waiting for result...",
                request.goal_id
            );
            match rx.await {
                Ok((r, s)) => {
                    tracing::debug!("Goal {:?} completed with status {:?}", request.goal_id, s);
                    (r, s)
                }
                Err(_) => {
                    tracing::warn!(
                        "Result future cancelled for goal {:?}; replying Aborted",
                        request.goal_id
                    );
                    (A::Result::default(), super::GoalStatus::Aborted)
                }
            }
        }
        ResultState::NotFound => {
            tracing::warn!(
                "Goal {:?} not found; replying Unknown",
                request.goal_id
            );
            (A::Result::default(), super::GoalStatus::Unknown)
        }
    };

    // Send result response
    let response = GetResultResponse::<A> {
        status: status as i8,
        result,
    };
    let response_bytes = <GetResultResponse<A> as ZMessage>::serialize(&response);
    let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
    let _ = tokio::task::spawn_blocking(move || {
        let _ = query
            .reply(query.key_expr().clone(), response_bytes)
            .attachment(attachment)
            .wait();
    })
    .await;
    tracing::debug!("Sent result response");
}
