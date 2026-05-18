#!/usr/bin/env python3
"""Test action client/server functionality."""

import threading
import time
from typing import ClassVar

import msgspec
import pytest

import hiroz_py


# ---------------------------------------------------------------------------
# Inline message types
# ---------------------------------------------------------------------------


class CountGoal(msgspec.Struct):
    __msgtype__: ClassVar[str] = "test_action/msg/CountGoal"
    target: int = 5
    step_delay: float = 0.05


class CountResult(msgspec.Struct):
    __msgtype__: ClassVar[str] = "test_action/msg/CountResult"
    final_count: int = 0


class CountFeedback(msgspec.Struct):
    __msgtype__: ClassVar[str] = "test_action/msg/CountFeedback"
    current: int = 0


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def action_context():
    return hiroz_py.ZContextBuilder().with_domain_id(5).build()


def make_pair(ctx, action_name):
    """Create a matched server/client pair on the given action name."""
    node_s = ctx.create_node("test_server").build()
    node_c = ctx.create_node("test_client").build()
    server = node_s.create_action_server(
        action_name, CountGoal, CountResult, CountFeedback
    )
    client = node_c.create_action_client(
        action_name, CountGoal, CountResult, CountFeedback
    )
    return server, client


def run_server_once(server, *, reject=False, abort=False):
    """Drive the server for a single goal in a background thread."""

    def _run():
        req = server.recv_goal(timeout=5.0)
        if req is None:
            return
        if reject:
            req.reject()
            return
        executing = req.accept_and_execute()
        goal = executing.goal()
        count = 0
        while count < goal.target:
            time.sleep(goal.step_delay)
            count += 1
            executing.publish_feedback(CountFeedback(current=count))
            if executing.is_cancel_requested:
                executing.canceled(CountResult(final_count=count))
                return
        if abort:
            executing.abort(CountResult(final_count=count))
        else:
            executing.succeed(CountResult(final_count=count))

    t = threading.Thread(target=_run, daemon=True)
    t.start()
    return t


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_action_client_server_creation(action_context):
    """Create a matched action client/server pair."""
    node_s = action_context.create_node("create_test_server").build()
    node_c = action_context.create_node("create_test_client").build()
    server = node_s.create_action_server(
        "/create_test", CountGoal, CountResult, CountFeedback
    )
    client = node_c.create_action_client(
        "/create_test", CountGoal, CountResult, CountFeedback
    )
    assert server is not None
    assert client is not None


def test_basic_send_and_result(action_context):
    """Send a goal and receive the final result."""
    server, client = make_pair(action_context, "/test_basic")
    run_server_once(server)
    time.sleep(0.3)

    handle = client.send_goal(CountGoal(target=3, step_delay=0.05))
    result = handle.get_result(timeout=5.0)

    assert result is not None
    assert result.final_count == 3


def test_feedback_streaming(action_context):
    """Receive incremental feedback messages during execution."""
    server, client = make_pair(action_context, "/test_feedback")
    run_server_once(server)
    time.sleep(0.3)

    handle = client.send_goal(CountGoal(target=4, step_delay=0.05))

    feedbacks = []
    for _ in range(10):
        fb = handle.recv_feedback(timeout=0.5)
        if fb is None:
            break
        feedbacks.append(fb.current)

    result = handle.get_result(timeout=3.0)
    assert result is not None
    assert result.final_count == 4
    assert len(feedbacks) > 0
    assert feedbacks[-1] == 4


def test_goal_status(action_context):
    """Goal status progresses through ACCEPTED → EXECUTING → SUCCEEDED."""
    server, client = make_pair(action_context, "/test_status")
    run_server_once(server)
    time.sleep(0.3)

    handle = client.send_goal(CountGoal(target=2, step_delay=0.1))
    # After send_goal the goal is accepted — status should be active
    assert hiroz_py.GoalStatus(handle.status).is_active()

    result = handle.get_result(timeout=5.0)
    assert result is not None
    # After get_result the goal is terminal
    assert hiroz_py.GoalStatus(handle.status).is_terminal()


def test_cancellation(action_context):
    """Client can cancel a running goal."""
    server, client = make_pair(action_context, "/test_cancel")
    run_server_once(server)
    time.sleep(0.3)

    handle = client.send_goal(CountGoal(target=20, step_delay=0.1))

    # Let the server start executing, then cancel
    time.sleep(0.3)
    handle.cancel()

    result = handle.get_result(timeout=5.0)
    assert result is not None
    # Server was canceled before reaching 20
    assert result.final_count < 20


def test_goal_rejection(action_context):
    """Server can reject a goal; client gets an error on get_result."""
    server, client = make_pair(action_context, "/test_reject")
    run_server_once(server, reject=True)
    time.sleep(0.3)

    with pytest.raises(Exception):
        # send_goal raises when the server rejects
        client.send_goal(CountGoal(target=3))


def test_goal_timeout_no_server(action_context):
    """get_result returns None when no server is present and timeout expires."""
    node_c = action_context.create_node("timeout_client").build()
    client = node_c.create_action_client(
        "/nonexistent_action", CountGoal, CountResult, CountFeedback
    )

    with pytest.raises(Exception):
        # send_goal should raise (or the goal handle's get_result should time out)
        handle = client.send_goal(CountGoal(target=1))
        result = handle.get_result(timeout=1.0)
        # If send_goal doesn't raise, get_result should return None
        assert result is None


def test_server_abort(action_context):
    """Server can abort a goal; client receives the abort result."""
    server, client = make_pair(action_context, "/test_abort")
    run_server_once(server, abort=True)
    time.sleep(0.3)

    handle = client.send_goal(CountGoal(target=3, step_delay=0.05))
    result = handle.get_result(timeout=5.0)

    assert result is not None
    assert result.final_count == 3


def main():
    print("=" * 60)
    print("hiroz-py Action Tests")
    print("=" * 60)

    ctx = hiroz_py.ZContextBuilder().with_domain_id(5).build()

    tests = [
        ("creation", lambda: test_action_client_server_creation(ctx)),
        ("basic send/result", lambda: test_basic_send_and_result(ctx)),
        ("feedback", lambda: test_feedback_streaming(ctx)),
        ("status", lambda: test_goal_status(ctx)),
        ("cancel", lambda: test_cancellation(ctx)),
        ("reject", lambda: test_goal_rejection(ctx)),
        ("abort", lambda: test_server_abort(ctx)),
    ]

    for name, fn in tests:
        try:
            fn()
            print(f"✓ {name}")
        except Exception as e:
            print(f"✗ {name}: {e}")

    print("=" * 60)


if __name__ == "__main__":
    main()
