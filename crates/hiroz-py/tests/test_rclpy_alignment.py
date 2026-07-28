#!/usr/bin/env python3
"""Tests for the rclpy-alignment features (P1-P8)."""

import threading
import time
from typing import ClassVar

import msgspec
import pytest

import hiroz_py
from hiroz_py import example_interfaces, std_msgs


# --- shared inline action types (mirrors test_action.py) ---


class CountGoal(msgspec.Struct):
    __msgtype__: ClassVar[str] = "rclpy_alignment/msg/CountGoal"
    target: int = 3
    step_delay: float = 0.05


class CountResult(msgspec.Struct):
    __msgtype__: ClassVar[str] = "rclpy_alignment/msg/CountResult"
    final_count: int = 0


class CountFeedback(msgspec.Struct):
    __msgtype__: ClassVar[str] = "rclpy_alignment/msg/CountFeedback"
    current: int = 0


class CountTo:
    __actiontype__: ClassVar[str] = "rclpy_alignment/action/CountTo"
    Goal = CountGoal
    Result = CountResult
    Feedback = CountFeedback


def run_action_server_once(server):
    """Drive the action server for a single goal in a background thread."""

    def _run():
        req = server.recv_goal(timeout=5.0)
        if req is None:
            return
        executing = req.accept_and_execute()
        goal = executing.goal()
        count = 0
        while count < goal.target:
            time.sleep(goal.step_delay)
            count += 1
            executing.publish_feedback(CountFeedback(current=count))
        executing.succeed(CountResult(final_count=count))

    t = threading.Thread(target=_run, daemon=True)
    t.start()
    return t


@pytest.fixture(scope="module")
def ctx():
    c = hiroz_py.ZContextBuilder().with_domain_id(0).build()
    yield c


# --- P8: QoS enum constants + int depth shorthand ---


def test_p8_policy_constants():
    assert hiroz_py.ReliabilityPolicy.RELIABLE == "reliable"
    assert hiroz_py.ReliabilityPolicy.BEST_EFFORT == "best_effort"
    assert hiroz_py.DurabilityPolicy.VOLATILE == "volatile"
    assert hiroz_py.DurabilityPolicy.TRANSIENT_LOCAL == "transient_local"
    assert hiroz_py.HistoryPolicy.KEEP_LAST == "keep_last"
    assert hiroz_py.HistoryPolicy.KEEP_ALL == "keep_all"
    assert hiroz_py.LivelinessPolicy.AUTOMATIC == "automatic"


def test_p8_int_depth_shorthand(ctx):
    node = ctx.create_node("p8_int").build()
    # qos=10 should be accepted as a depth shorthand.
    pub = node.create_publisher("/p8_topic", std_msgs.String, qos=10)
    assert pub is not None


def test_p8_policy_constants_in_qos(ctx):
    node = ctx.create_node("p8_policy").build()
    qos = hiroz_py.QosProfile(
        reliability=hiroz_py.ReliabilityPolicy.BEST_EFFORT,
        history=hiroz_py.HistoryPolicy.KEEP_LAST,
        depth=5,
    )
    assert qos.reliability == "best_effort"
    pub = node.create_publisher("/p8_policy_topic", std_msgs.String, qos=qos)
    assert pub is not None


# --- P2: swapped-argument smart error ---


def test_p2_swapped_args_publisher(ctx):
    node = ctx.create_node("p2_pub").build()
    with pytest.raises(TypeError, match="swapped"):
        # rclpy order: (msg_type, topic) -> should be rejected with a clear error.
        node.create_publisher(std_msgs.String, "/chatter")


def test_p2_swapped_args_subscriber(ctx):
    node = ctx.create_node("p2_sub").build()
    with pytest.raises(TypeError, match="swapped"):
        node.create_subscriber(std_msgs.String, "/chatter")


def test_p2_keyword_args_work(ctx):
    node = ctx.create_node("p2_kw").build()
    # Keyword args work regardless of historical order.
    pub = node.create_publisher(msg_type=std_msgs.String, topic="/p2_kw_topic")
    assert pub is not None


# --- P3: method aliases ---


def test_p3_create_subscription_alias():
    assert hiroz_py.ZNode.create_subscription is hiroz_py.ZNode.create_subscriber


def test_p3_create_service_alias():
    assert hiroz_py.ZNode.create_service is hiroz_py.ZNode.create_server


def test_p3_create_subscription_alias_end_to_end(ctx):
    node = ctx.create_node("p3_sub_alias").build()
    pub = node.create_publisher("/p3_sub_topic", std_msgs.String)
    received = []
    node.create_subscription("/p3_sub_topic", std_msgs.String, callback=received.append)

    assert pub.wait_for_subscription(count=1, timeout=5.0)
    pub.publish(std_msgs.String(data="via-alias"))
    deadline = time.time() + 3.0
    while not received and time.time() < deadline:
        time.sleep(0.05)
    assert received and received[0].data == "via-alias"


def test_p3_create_service_alias_end_to_end(ctx):
    node = ctx.create_node("p3_srv_alias").build()

    def handle(req):
        return example_interfaces.AddTwoInts.Response(sum=req.a + req.b)

    server = node.create_service(
        "/p3_add", example_interfaces.AddTwoInts, callback=handle
    )
    client = node.create_client("/p3_add", example_interfaces.AddTwoInts)
    assert client.wait_for_service(timeout=5.0)

    resp = client.call(example_interfaces.AddTwoInts.Request(a=10, b=32), timeout=5.0)
    assert resp.sum == 42
    assert server is not None


# --- P4: service grouping class ---


def test_p4_grouping_class_attributes():
    assert (
        example_interfaces.AddTwoInts.__srvtype__ == "example_interfaces/srv/AddTwoInts"
    )
    assert example_interfaces.AddTwoInts.Request is example_interfaces.AddTwoIntsRequest
    assert (
        example_interfaces.AddTwoInts.Response is example_interfaces.AddTwoIntsResponse
    )


def test_p4_client_accepts_grouping_class(ctx):
    node = ctx.create_node("p4_client").build()
    client = node.create_client("/p4_add", example_interfaces.AddTwoInts)
    assert client is not None


def test_p4_client_accepts_bare_request(ctx):
    # Back-compat: bare Request class still works.
    node = ctx.create_node("p4_client_bc").build()
    client = node.create_client("/p4_add_bc", example_interfaces.AddTwoIntsRequest)
    assert client is not None


# --- P5: custom exception types ---


def test_p5_exception_hierarchy():
    assert issubclass(hiroz_py.TimeoutError, hiroz_py.HirozError)
    assert issubclass(hiroz_py.SerializationError, hiroz_py.HirozError)
    assert issubclass(hiroz_py.TypeMismatchError, hiroz_py.HirozError)


def test_p5_hiroz_error_is_runtime_error():
    """Ported rclpy code catching RuntimeError must keep working.

    The blocking call paths raised a bare RuntimeError before P5; anchoring
    HirozError under it keeps every existing `except RuntimeError:` live.
    """
    assert issubclass(hiroz_py.HirozError, RuntimeError)


def test_p5_timeout_error_is_builtin_timeout_error():
    """rclpy's Client.call raises the *builtin* TimeoutError, not a ROS type.

    Without this base a ported `except TimeoutError:` silently stops catching
    -- it still compiles and runs, so the failure is invisible.
    """
    assert issubclass(hiroz_py.TimeoutError, TimeoutError)
    # builtins.TimeoutError derives from OSError, so instances are OSErrors
    # too. That is inherited from the builtin and matches rclpy.
    assert issubclass(hiroz_py.TimeoutError, OSError)


def test_p5_timeout_caught_by_every_documented_except_clause(ctx):
    """One raised timeout must satisfy all four documented catch styles."""
    node = ctx.create_node("p5_multi_catch").build()
    server = node.create_server("/p5_multi_catch_svc", example_interfaces.AddTwoInts)
    assert server is not None

    threading.Thread(target=server.take_request, daemon=True).start()

    client = node.create_client("/p5_multi_catch_svc", example_interfaces.AddTwoInts)
    assert client.wait_for_service(timeout=5.0)

    req = example_interfaces.AddTwoInts.Request(a=1, b=2)
    try:
        client.call(req, timeout=0.5)
        raise AssertionError("expected a timeout")
    except Exception as exc:
        assert isinstance(exc, hiroz_py.TimeoutError)
        assert isinstance(exc, hiroz_py.HirozError)
        assert isinstance(exc, TimeoutError)  # builtin -- the rclpy contract
        assert isinstance(exc, RuntimeError)  # pre-P5 contract
        assert "timed out" in str(exc)


def test_p5_call_failure_is_hiroz_error(ctx):
    node = ctx.create_node("p5_client").build()
    client = node.create_client("/p5_nonexistent", example_interfaces.AddTwoInts)
    req = example_interfaces.AddTwoInts.Request(a=1, b=2)
    with pytest.raises(hiroz_py.HirozError):
        client.call(req, timeout=1.0)


def test_p5_call_timeout_raises_timeout_error(ctx):
    # A matched-but-unresponsive server (as opposed to no server at all) is
    # required to exercise the actual "timed out" path -- with zero matching
    # queryables the call fails immediately with a different (non-timeout)
    # message (see test_p5_call_failure_is_hiroz_error above).
    node = ctx.create_node("p5_timeout_server").build()
    server = node.create_server("/p5_never_answers", example_interfaces.AddTwoInts)
    assert server is not None

    def _never_respond():
        server.take_request()  # receive but never send_response

    threading.Thread(target=_never_respond, daemon=True).start()

    client = node.create_client("/p5_never_answers", example_interfaces.AddTwoInts)
    assert client.wait_for_service(timeout=5.0)

    req = example_interfaces.AddTwoInts.Request(a=1, b=2)
    with pytest.raises(hiroz_py.TimeoutError):
        client.call(req, timeout=0.5)


def test_p5_action_result_timeout_raises_timeout_error(ctx):
    """get_result(timeout=...) now raises hiroz_py.TimeoutError, matching
    ZClient.call's timeout semantics (previously it returned None)."""
    node = ctx.create_node("p5_action_client").build()
    client = node.create_action_client("/p5_never_completes", CountTo)
    server = node.create_action_server("/p5_never_completes", CountTo)

    def _never_finish():
        req = server.recv_goal(timeout=5.0)
        if req is not None:
            req.accept_and_execute()
            # Deliberately never call succeed/abort/canceled.

    threading.Thread(target=_never_finish, daemon=True).start()
    assert client.wait_for_server(timeout=5.0)

    handle = client.send_goal(CountGoal(target=1))
    with pytest.raises(hiroz_py.TimeoutError):
        handle.get_result(timeout=0.5)


# --- P1 + P4 + P6: end-to-end service with wait_for_service and callback mode ---


def test_p1_p6_callback_service_end_to_end(ctx):
    node = ctx.create_node("p6_node").build()

    def handle(req):
        return example_interfaces.AddTwoInts.Response(sum=req.a + req.b)

    # P6: callback-mode server (no take_request loop).
    server = node.create_server(
        "/p6_add", example_interfaces.AddTwoInts, callback=handle
    )
    assert server is not None

    client = node.create_client("/p6_add", example_interfaces.AddTwoInts)
    # P1: wait for the server instead of sleeping.
    assert client.wait_for_service(timeout=5.0), "server should be discoverable"

    resp = client.call(example_interfaces.AddTwoInts.Request(a=4, b=38), timeout=5.0)
    assert resp.sum == 42


def test_p6_last_error_surfaced_on_callback_exception(ctx):
    node = ctx.create_node("p6_err_node").build()

    def bad_handle(req):
        raise ValueError("intentional callback failure")

    server = node.create_server(
        "/p6_err_add", example_interfaces.AddTwoInts, callback=bad_handle
    )
    client = node.create_client("/p6_err_add", example_interfaces.AddTwoInts)
    assert client.wait_for_service(timeout=5.0)

    # The call will fail from the client side (no response sent).
    try:
        client.call(example_interfaces.AddTwoInts.Request(a=1, b=2), timeout=1.0)
    except Exception:
        pass

    # Give the background thread a moment to record the error.
    deadline = time.time() + 2.0
    err = None
    while err is None and time.time() < deadline:
        err = server.last_error
        if err is None:
            time.sleep(0.05)

    assert err is not None, "last_error should surface the callback exception"
    assert "intentional callback failure" in err


def test_p6_last_error_none_when_no_error(ctx):
    node = ctx.create_node("p6_ok_node").build()

    def handle(req):
        return example_interfaces.AddTwoInts.Response(sum=req.a + req.b)

    server = node.create_server(
        "/p6_ok_add", example_interfaces.AddTwoInts, callback=handle
    )
    assert server.last_error is None


def test_p1_wait_for_service_timeout_returns_false(ctx):
    node = ctx.create_node("p1_wait_to").build()
    client = node.create_client("/p1_never", example_interfaces.AddTwoInts)
    t0 = time.time()
    assert client.wait_for_service(timeout=0.5) is False
    assert time.time() - t0 >= 0.4


# --- P1: wait_for_subscription end-to-end ---


def test_p1_wait_for_subscription(ctx):
    node = ctx.create_node("p1_pubsub").build()
    pub = node.create_publisher("/p1_chatter", std_msgs.String)
    received = []

    def cb(msg):
        received.append(msg.data)

    node.create_subscriber("/p1_chatter", std_msgs.String, callback=cb)

    assert pub.wait_for_subscription(count=1, timeout=5.0), "subscription should match"

    pub.publish(std_msgs.String(data="hello"))
    deadline = time.time() + 3.0
    while not received and time.time() < deadline:
        time.sleep(0.05)
    assert received == ["hello"]


# --- P1: wait_for_server (action) ---


def test_p1_wait_for_server(ctx):
    node = ctx.create_node("p1_wait_for_server").build()
    server = node.create_action_server(
        "/p1_action_wfs", CountGoal, CountResult, CountFeedback
    )
    client = node.create_action_client(
        "/p1_action_wfs", CountGoal, CountResult, CountFeedback
    )
    assert client.wait_for_server(timeout=5.0), "action server should be discoverable"
    assert server is not None


def test_p1_wait_for_server_timeout_returns_false(ctx):
    node = ctx.create_node("p1_wait_for_server_to").build()
    client = node.create_action_client(
        "/p1_action_never", CountGoal, CountResult, CountFeedback
    )
    t0 = time.time()
    assert client.wait_for_server(timeout=0.5) is False
    assert time.time() - t0 >= 0.4


# --- P7: action grouping class, exercised through an actual goal send ---


def test_p7_action_grouping_class_construction(ctx):
    node = ctx.create_node("p7_action").build()
    # Single grouping class instead of three positional types.
    client = node.create_action_client("/p7_count", CountTo)
    assert client is not None
    server = node.create_action_server("/p7_count", CountTo)
    assert server is not None


def test_p7_action_grouping_class_end_to_end(ctx):
    node = ctx.create_node("p7_e2e").build()
    server = node.create_action_server("/p7_e2e_count", CountTo)
    client = node.create_action_client("/p7_e2e_count", CountTo)

    assert client.wait_for_server(timeout=5.0)
    run_action_server_once(server)

    handle = client.send_goal(CountTo.Goal(target=3, step_delay=0.05))
    result = handle.get_result(timeout=5.0)
    assert result is not None
    assert result.final_count == 3


# --- Review follow-ups: input validation and error contracts ---


@pytest.mark.parametrize("bad", [-1.0, float("nan"), float("inf")])
def test_timeout_rejects_non_finite_and_negative(ctx, bad):
    """Timeout args take an unrestricted float, so these reach Rust.

    Duration::from_secs_f64 panics on all three; they must surface as an
    ordinary ValueError rather than a panic leaking through PyO3.
    """
    node = ctx.create_node("timeout_validation").build()
    client = node.create_client("/tv_svc", example_interfaces.AddTwoInts)
    sub = node.create_subscriber("/tv_topic", std_msgs.String)
    pub = node.create_publisher("/tv_topic", std_msgs.String)

    with pytest.raises(ValueError):
        client.wait_for_service(timeout=bad)
    with pytest.raises(ValueError):
        sub.recv(timeout=bad)
    with pytest.raises(ValueError):
        pub.wait_for_subscription(timeout=bad)


@pytest.mark.parametrize(
    "bad_name",
    [
        "//bad//name",  # empty chunks: ROS validation skips these, Zenoh rejects them
        "/bad name",  # space is not a valid topic component
        "",  # empty
    ],
)
def test_invalid_service_name_raises_value_error(ctx, bad_name):
    """Malformed names must fail at construction with ValueError.

    Two things are being pinned. First, validation has to run *before* build,
    or the builder rejects the name first and surfaces HirozError instead.
    Second, the check has to be stricter than ROS name validation alone --
    that skips empty path components, so `//bad//name` would otherwise reach
    Zenoh's key-expression parser and fail with an opaque error citing a cargo
    registry path.
    """
    node = ctx.create_node("name_validation").build()
    with pytest.raises(ValueError):
        node.create_client(bad_name, example_interfaces.AddTwoInts)
    with pytest.raises(ValueError):
        node.create_server(bad_name, example_interfaces.AddTwoInts)


def test_callback_server_survives_repeated_failures(ctx):
    """A repeatedly-raising callback must not retain a pending reply each time.

    Every request registers a reply handle that only send_response removes;
    the error paths have to discard it explicitly. After the failures, a
    working server on a fresh name must still answer normally.
    """
    node = ctx.create_node("pending_leak").build()

    def always_raises(_req):
        raise ValueError("boom")

    server = node.create_service(
        "/leak_svc", example_interfaces.AddTwoInts, callback=always_raises
    )
    client = node.create_client("/leak_svc", example_interfaces.AddTwoInts)
    assert client.wait_for_service(timeout=5.0)

    for _ in range(5):
        with pytest.raises(hiroz_py.HirozError):
            client.call(example_interfaces.AddTwoInts.Request(a=1, b=2), timeout=0.4)

    # The callback thread recorded the failures rather than dying.
    assert server.last_error is not None


# --- P7 grouping classes come from codegen, not just hand-written types ---


def test_p7_generated_action_grouping_class_exists():
    """P7 must be wired into codegen, not only satisfiable by hand-written types.

    hiroz_msgs vendors action_msgs; if the generator emitted any action
    grouping class it carries __actiontype__ plus a Goal member.
    """
    from hiroz_msgs_py import types as msg_types

    found = []
    for pkg_name in getattr(msg_types, "__all__", []):
        pkg = getattr(msg_types, pkg_name)
        for attr in dir(pkg):
            obj = getattr(pkg, attr)
            if isinstance(obj, type) and hasattr(obj, "__actiontype__"):
                found.append((pkg_name, attr, obj))

    if not found:
        pytest.skip("no .action files in the vendored packages for this distro")

    for pkg_name, attr, obj in found:
        assert obj.__actiontype__ == f"{pkg_name}/action/{attr}"
        assert hasattr(obj, "Goal"), f"{attr} grouping class must expose Goal"


# --- Callback-server shutdown must not block on the GIL ---


def test_callback_server_close_is_explicit_and_idempotent(ctx):
    """close() joins the worker with the GIL released; Drop only signals.

    Joining from Drop would deadlock against a callback waiting on the GIL,
    so the blocking path has to be reachable only from close()/__exit__.
    """
    node = ctx.create_node("server_close").build()

    def handle(req):
        return example_interfaces.AddTwoInts.Response(sum=req.a + req.b)

    server = node.create_service(
        "/close_svc", example_interfaces.AddTwoInts, callback=handle
    )
    client = node.create_client("/close_svc", example_interfaces.AddTwoInts)
    assert client.wait_for_service(timeout=5.0)
    assert (
        client.call(example_interfaces.AddTwoInts.Request(a=1, b=2), timeout=5.0).sum
        == 3
    )

    server.close()
    server.close()  # idempotent


def test_callback_server_context_manager(ctx):
    node = ctx.create_node("server_ctx").build()

    def handle(req):
        return example_interfaces.AddTwoInts.Response(sum=req.a + req.b)

    with node.create_service(
        "/ctx_svc", example_interfaces.AddTwoInts, callback=handle
    ) as server:
        assert server is not None
        client = node.create_client("/ctx_svc", example_interfaces.AddTwoInts)
        assert client.wait_for_service(timeout=5.0)
        resp = client.call(
            example_interfaces.AddTwoInts.Request(a=20, b=22), timeout=5.0
        )
        assert resp.sum == 42


def test_pull_mode_close_is_a_noop(ctx):
    node = ctx.create_node("pull_close").build()
    server = node.create_server("/pull_close_svc", example_interfaces.AddTwoInts)
    server.close()
