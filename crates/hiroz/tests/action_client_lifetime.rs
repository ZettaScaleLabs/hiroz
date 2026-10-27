//! Local goal tracking must follow ownership, including cancellation of futures.
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

use hiroz::{Builder, Result, context::ZContextBuilder, define_action};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Goal {
    value: i32,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Outcome {
    value: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Feedback {
    value: i32,
}
struct Action;
define_action! {
    Action,
    action_name: "client_lifetime",
    Goal: Goal,
    Result: Outcome,
    Feedback: Feedback,
}

fn setup(
    name: &str,
) -> Result<(
    hiroz::context::ZContext,
    hiroz::action::client::ZActionClient<Action>,
    hiroz::action::server::ZActionServer<Action>,
)> {
    let context = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .build()?;
    let node = context.create_node(name).build()?;
    let server = node.create_action_server::<Action>(name).build()?;
    let client = node.create_action_client::<Action>(name).build()?;
    Ok((context, client, server))
}

async fn accepted(
    client: &hiroz::action::client::ZActionClient<Action>,
    server: &hiroz::action::server::ZActionServer<Action>,
) -> (
    hiroz::action::client::GoalHandle<Action>,
    hiroz::action::server::ExecutingGoal<Action>,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (sent, executing) = tokio::join!(client.send_goal(Goal { value: 42 }), async {
            server.recv_goal().await.unwrap().accept().execute()
        },);
        (sent.unwrap(), executing)
    })
    .await
    .expect("local goal acceptance")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_send_goal_releases_registration() -> Result<()> {
    let (_context, client, server) = setup("canceled_send_goal_releases_registration")?;
    for _ in 0..32 {
        let mut send = Box::pin(client.send_goal(Goal { value: 42 }));
        // Receipt proves send_goal has registered the goal and reached its await.
        // Hold the server response so canceling the future is deterministic.
        let requested = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                _ = &mut send => panic!("goal completed before server accepted"),
                goal = server.recv_goal() => goal.unwrap(),
            }
        })
        .await
        .expect("local goal receipt");
        let id = requested.info.goal_id;
        assert!(client.status_watch(id).is_some());
        drop(send);
        assert!(
            client.status_watch(id).is_none(),
            "canceled send retained tracking"
        );
        requested.reject()?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_goal_handle_releases_registration_without_canceling_server() -> Result<()> {
    let (_context, client, server) = setup("dropped_goal_handle_releases_registration")?;
    for _ in 0..32 {
        let (handle, executing) = accepted(&client, &server).await;
        let id = handle.id();
        assert!(client.status_watch(id).is_some());
        drop(handle);
        assert!(
            client.status_watch(id).is_none(),
            "dropped handle retained tracking"
        );
        assert!(!executing.is_cancel_requested());
        executing.succeed(Outcome { value: 42 })?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_result_futures_release_registration() -> Result<()> {
    let (_context, client, server) = setup("canceled_result_futures_release_registration")?;
    for bounded in [false, true] {
        for _ in 0..16 {
            let (handle, executing) = accepted(&client, &server).await;
            let id = handle.id();
            let mut result: std::pin::Pin<Box<dyn Future<Output = Result<Outcome>>>> = if bounded {
                Box::pin(handle.result_with_timeout(Duration::from_secs(60)))
            } else {
                Box::pin(handle.result())
            };
            let polled = std::future::poll_fn(|cx| Poll::Ready(result.as_mut().poll(cx))).await;
            assert!(polled.is_pending(), "server has not produced a result");
            drop(result);
            assert!(
                client.status_watch(id).is_none(),
                "canceled result retained tracking"
            );
            assert!(!executing.is_cancel_requested());
            executing.succeed(Outcome { value: 42 })?;
        }
    }
    // The same persistent client must still deliver an ordinary result and
    // remove that registration after the consuming future completes normally.
    let (handle, executing) = accepted(&client, &server).await;
    let id = handle.id();
    executing.succeed(Outcome { value: 42 })?;
    assert_eq!(
        handle
            .result_with_timeout(Duration::from_secs(2))
            .await?
            .value,
        42
    );
    assert!(client.status_watch(id).is_none());
    Ok(())
}

struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_action_result_rejects_queued_response_after_polling_gap() -> Result<()> {
    let (_context, client, server) = setup("expired_action_result_after_polling_gap")?;
    let (handle, executing) = accepted(&client, &server).await;
    let id = handle.id();
    let budget = Duration::from_millis(20);
    let mut result = Box::pin(handle.result_with_timeout(budget));
    let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(wakes.clone());
    assert!(
        result
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );

    // Observe the real server's pending GetResult and the elapsed timer without
    // polling the caller again. Its custom waker records notifications only.
    tokio::time::timeout(Duration::from_secs(1), async {
        while !server
            .goal_manager()
            .read(|manager| manager.result_futures.contains_key(&id))
            || wakes.0.load(Ordering::SeqCst) == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("GetResult was not waiting when its caller deadline elapsed");
    let timer_wakes = wakes.0.load(Ordering::SeqCst);
    executing.succeed(Outcome { value: 42 })?;
    // The timer has already fired. A new notification proves that the actual
    // result response has reached the still-unpolled client future.
    tokio::time::timeout(Duration::from_secs(1), async {
        while wakes.0.load(Ordering::SeqCst) == timer_wakes {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real action response was not queued");
    let error = result
        .await
        .expect_err("expired action accepted a queued late result");
    assert!(
        hiroz::error::is_timeout(error.as_ref()),
        "wrong error: {error}"
    );
    assert!(
        client.status_watch(id).is_none(),
        "expired result retained local tracking"
    );

    // Reuse the same client with an unrepresentable deadline; successful
    // results and local registration cleanup must retain their normal behavior.
    let (handle, executing) = accepted(&client, &server).await;
    let id = handle.id();
    executing.succeed(Outcome { value: 43 })?;
    assert_eq!(handle.result_with_timeout(Duration::MAX).await?.value, 43);
    assert!(client.status_watch(id).is_none());

    let (handle, executing) = accepted(&client, &server).await;
    let id = handle.id();
    let error = handle
        .result_with_timeout(Duration::ZERO)
        .await
        .unwrap_err();
    assert!(hiroz::error::is_timeout(error.as_ref()));
    assert!(client.status_watch(id).is_none());
    assert!(!executing.is_cancel_requested());
    executing.succeed(Outcome { value: 44 })?;
    Ok(())
}
