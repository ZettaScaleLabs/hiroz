//! Wire-level cancellation selection shared by automatic and manual Rust servers.

use std::time::Duration;

use hiroz::{
    Builder, Result,
    action::{
        GoalId, GoalInfo, Time,
        messages::{
            CancelGoalServiceRequest, CancelGoalServiceResponse, CancelService, GoalService,
            SendGoalRequest,
        },
        server::{ExecutingGoal, ZActionServer},
    },
    define_action,
    service::ZClient,
};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SelectionGoal {
    value: i32,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SelectionResult {
    value: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SelectionFeedback {
    value: i32,
}
struct SelectionAction;
define_action! {
    SelectionAction,
    action_name: "cancel_selection",
    Goal: SelectionGoal,
    Result: SelectionResult,
    Feedback: SelectionFeedback,
}

async fn cancel(
    client: &ZClient<CancelService<SelectionAction>>,
    server: &ZActionServer<SelectionAction>,
    poller: &ExecutingGoal<SelectionAction>,
    request: GoalInfo,
    automatic: bool,
) -> Result<CancelGoalServiceResponse> {
    let request = CancelGoalServiceRequest { goal_info: request };
    let call = client.call_with_timeout(&request, Duration::from_secs(2));
    if automatic {
        return call.await;
    }
    // Wait for a real wire request before polling. No arbitrary discovery sleep
    // or direct call into cancellation internals can mask dispatch mistakes.
    timeout(Duration::from_secs(3), async {
        let (response, ()) = tokio::join!(call, async {
            while !server.is_cancel_request_ready() {
                tokio::task::yield_now().await;
            }
            poller.try_process_cancel();
        });
        response
    })
    .await
    .expect("manual cancellation did not complete")
}

async fn selection(automatic: bool, selection: u8) -> Result<()> {
    // Fresh goals for each selection prevent a prior cancellation from hiding
    // an incorrectly selected goal. Goal C is explicitly selected by UUID in
    // the union case, leaving intermediate goal B as a negative control.
    {
        let context = crate::action::context_builder().build()?;
        let node = context
            .create_node("cancel_selection")
            .without_parameters()
            .build()?;
        let server = node
            .create_action_server::<SelectionAction>("/cancel_selection")
            .build()?;
        let (accepted_tx, mut accepted_rx) = tokio::sync::mpsc::unbounded_channel();
        let server = if automatic {
            server.with_handler(move |goal| {
                assert!(
                    accepted_tx.send(goal).is_ok(),
                    "fixture retains accepted receiver"
                );
                async {}
            })
        } else {
            server
        };
        let goals = node
            .create_client::<GoalService<SelectionAction>>("/cancel_selection/_action/send_goal")
            .build()?;
        let cancels = node
            .create_client::<CancelService<SelectionAction>>(
                "/cancel_selection/_action/cancel_goal",
            )
            .build()?;
        let mut handles = Vec::new();
        let mut infos: Vec<GoalInfo> = Vec::new();
        for index in 0..3 {
            if let Some(previous) = infos.last() {
                timeout(Duration::from_secs(1), async {
                    loop {
                        let now = Time::now();
                        if (now.sec, now.nanosec) > (previous.stamp.sec, previous.stamp.nanosec) {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("wall clock did not advance between acceptances");
            }
            let id = GoalId::from_bytes([index + 1; 16]);
            let request = SendGoalRequest {
                goal_id: id,
                goal: SelectionGoal {
                    value: i32::from(index),
                },
            };
            let call = goals.call_with_timeout(&request, Duration::from_secs(2));
            let (response, handle) = if automatic {
                let response = call.await?;
                let handle = timeout(Duration::from_secs(2), accepted_rx.recv())
                    .await
                    .expect("automatic handler did not run")
                    .expect("handler channel closed");
                (response, handle)
            } else {
                let (response, requested) = timeout(Duration::from_secs(2), async {
                    tokio::join!(call, async {
                        server
                            .recv_goal()
                            .await
                            .map(|requested| requested.accept().execute())
                    })
                })
                .await
                .expect("manual goal request did not arrive");
                (response?, requested?)
            };
            assert!(response.accepted);
            let info = GoalInfo {
                goal_id: id,
                stamp: Time {
                    sec: response.stamp_sec,
                    nanosec: response.stamp_nanosec,
                },
            };
            assert_eq!(handle.info().stamp, info.stamp);
            infos.push(info);
            handles.push(handle);
        }
        assert!(!server.request_cancel(GoalId::from_bytes([0; 16])));
        assert!(handles.iter().all(|handle| !handle.is_cancel_requested()));
        let request = GoalInfo {
            goal_id: if selection.is_multiple_of(2) {
                GoalId::from_bytes([0; 16])
            } else {
                infos[2].goal_id
            },
            stamp: if selection < 2 {
                Time::zero()
            } else {
                infos[0].stamp
            },
        };
        let expected: &[usize] = match selection {
            0 => &[0, 1, 2],
            1 => &[2],
            2 => &[0],
            _ => &[0, 2],
        };
        // A repeated request must also receive a response, even if the manual
        // poller's own goal is already canceling.
        for _ in 0..2 {
            let response =
                cancel(&cancels, &server, &handles[0], request.clone(), automatic).await?;
            assert_eq!(response.return_code, 0);
            assert_eq!(
                response.goals_canceling.len(),
                expected.len(),
                "selection {selection}"
            );
            for info in &response.goals_canceling {
                let index = infos
                    .iter()
                    .position(|known| known.goal_id == info.goal_id)
                    .expect("unknown canceled UUID");
                assert!(
                    expected.contains(&index),
                    "selection {selection} canceled unrelated goal {index}"
                );
                assert_eq!(
                    info.stamp, infos[index].stamp,
                    "response changed acceptance time"
                );
            }
            for (index, handle) in handles.iter().enumerate() {
                assert_eq!(
                    handle.is_cancel_requested(),
                    expected.contains(&index),
                    "selection {selection}, goal {index}"
                );
            }
        }
        let finished = handles.remove(0);
        if finished.is_cancel_requested() {
            finished.canceled(SelectionResult::default())?;
        } else {
            finished.succeed(SelectionResult::default())?;
        }
        for (id, code) in [(infos[0].goal_id, 3), (GoalId::from_bytes([99; 16]), 2)] {
            let response = cancel(
                &cancels,
                &server,
                &handles[0],
                GoalInfo {
                    goal_id: id,
                    stamp: Time::zero(),
                },
                automatic,
            )
            .await?;
            assert_eq!(response.return_code, code);
            assert!(response.goals_canceling.is_empty());
        }
        for handle in handles {
            if handle.is_cancel_requested() {
                handle.canceled(SelectionResult::default())?;
            } else {
                handle.succeed(SelectionResult::default())?;
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_cancel_selection_and_metadata() -> Result<()> {
    for mode in 0..4 {
        selection(true, mode).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_cancel_selection_and_metadata() -> Result<()> {
    for mode in 0..4 {
        selection(false, mode).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_cutoff_does_not_cancel_later_goals() -> Result<()> {
    selection(true, 2).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_cutoff_does_not_cancel_later_goals() -> Result<()> {
    selection(false, 2).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_cancel_before_execute_preserves_acceptance() -> Result<()> {
    let context = crate::action::context_builder().build()?;
    let node = context
        .create_node("cancel_accepted")
        .without_parameters()
        .build()?;
    let server = node
        .create_action_server::<SelectionAction>("/cancel_selection")
        .build()?;
    let goals = node
        .create_client::<GoalService<SelectionAction>>("/cancel_selection/_action/send_goal")
        .build()?;
    let cancels = node
        .create_client::<CancelService<SelectionAction>>("/cancel_selection/_action/cancel_goal")
        .build()?;
    let request = SendGoalRequest {
        goal_id: GoalId::from_bytes([1; 16]),
        goal: SelectionGoal { value: 1 },
    };
    let (response, accepted) = timeout(Duration::from_secs(2), async {
        tokio::join!(
            goals.call_with_timeout(&request, Duration::from_secs(2)),
            async { server.recv_goal().await.map(|requested| requested.accept()) }
        )
    })
    .await
    .expect("accepted goal did not arrive");
    let response = response?;
    let accepted = accepted?;
    let info = GoalInfo {
        goal_id: request.goal_id,
        stamp: Time {
            sec: response.stamp_sec,
            nanosec: response.stamp_nanosec,
        },
    };
    let request = SendGoalRequest {
        goal_id: GoalId::from_bytes([2; 16]),
        goal: SelectionGoal { value: 2 },
    };
    let (response, poller) = timeout(Duration::from_secs(2), async {
        tokio::join!(
            goals.call_with_timeout(&request, Duration::from_secs(2)),
            async {
                server
                    .recv_goal()
                    .await
                    .map(|requested| requested.accept().execute())
            }
        )
    })
    .await
    .expect("poller goal did not arrive");
    assert!(response?.accepted);
    let poller = poller?;
    let response = cancel(
        &cancels,
        &server,
        &poller,
        GoalInfo {
            goal_id: info.goal_id,
            stamp: Time::zero(),
        },
        false,
    )
    .await?;
    assert_eq!(response.return_code, 0);
    assert_eq!(response.goals_canceling.len(), 1);
    assert_eq!(response.goals_canceling[0].goal_id, info.goal_id);
    assert_eq!(response.goals_canceling[0].stamp, info.stamp);
    assert!(!poller.is_cancel_requested());
    let executing = accepted.execute();
    assert!(
        executing.is_cancel_requested(),
        "execution lost accepted-state cancellation"
    );
    assert_eq!(executing.info().stamp, info.stamp);
    executing.canceled(SelectionResult::default())?;
    poller.succeed(SelectionResult::default())?;
    Ok(())
}
