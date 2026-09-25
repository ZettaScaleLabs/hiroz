//! Action server event handling.

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

/// Runs the driver loop for an action server with automatic goal handling.
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

    // Track request tasks so shutdown can abort and join them.
    let mut goal_tasks = JoinSet::new();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::debug!("Shutdown signal received. Aborting all goal tasks.");
                goal_tasks.abort_all();
                break;
            }

            // Reap finished request tasks.
            Some(res) = goal_tasks.join_next() => {
                if let Err(e) = res {
                    if e.is_cancelled() {
                        tracing::debug!("Goal task was cancelled");
                    } else if e.is_panic() {
                        tracing::error!("Goal task panicked!");
                    }
                }
            }

            _ = expiration_timer.tick() => {
                // Check for expired goals and clean them up
                let server = ZActionServer::from_inner(Arc::clone(&inner));
                let expired_goals = server.expire_goals();
                if !expired_goals.is_empty() {
                    tracing::debug!("Expired {} goals: {:?}", expired_goals.len(), expired_goals);
                }
            }

            query = inner.goal_server.queue().recv_async() => {
                let inner = inner.clone();
                let handler = handler.clone();

                goal_tasks.spawn(async move {
                    handle_goal_request(inner, query, handler).await;
                });
            }

            // Keep receiving while a result request waits for goal completion.
            query = inner.cancel_server.queue().recv_async() => {
                let inner = inner.clone();
                goal_tasks.spawn(async move {
                    handle_cancel_request(&inner, query).await;
                });
            }

            // Result requests may remain open for the lifetime of a goal.
            query = inner.result_server.queue().recv_async() => {
                let inner = inner.clone();
                goal_tasks.spawn(async move {
                    handle_result_request(&inner, query).await;
                });
            }
        }
    }

    // Join canceled request tasks before returning.
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

    let server = ZActionServer::from_inner(Arc::clone(&inner));

    let requested = GoalHandle {
        goal: request.goal,
        info: GoalInfo::new(request.goal_id),
        server: server.clone(),
        cancel_flag: None,
        cleanup: crate::action::server::GoalCleanup::new(request.goal_id, server, Some(query)),
        _state: PhantomData::<Requested>,
    };

    // The reply path blocks, so keep it off the async worker.
    let accepted = match tokio::task::spawn_blocking(move || requested.accept()).await {
        Ok(accepted) => accepted,
        Err(e) => {
            tracing::error!("send_goal accept task failed: {e}");
            return;
        }
    };
    let executing = accepted.execute();

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
    let response = server.process_cancel_request(&request.goal_info);

    let response_bytes = <CancelGoalServiceResponse as ZMessage>::serialize(&response);
    let attachment: Attachment = query.attachment().unwrap().try_into().unwrap();
    // Keep the blocking reply off the async worker.
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
                    ((inner.default_result)(), super::GoalStatus::Aborted)
                }
            }
        }
        ResultState::NotFound => {
            tracing::warn!("Goal {:?} not found; replying Unknown", request.goal_id);
            ((inner.default_result)(), super::GoalStatus::Unknown)
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
