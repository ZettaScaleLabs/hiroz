#!/usr/bin/env python3
"""Tests for the rclpy-alignment features (P1-P8)."""

import time

import pytest

import hiroz_py
from hiroz_py import example_interfaces, std_msgs


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


def test_p5_call_failure_is_hiroz_error(ctx):
    node = ctx.create_node("p5_client").build()
    client = node.create_client("/p5_nonexistent", example_interfaces.AddTwoInts)
    req = example_interfaces.AddTwoInts.Request(a=1, b=2)
    with pytest.raises(hiroz_py.HirozError):
        client.call(req, timeout=1.0)


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


# --- P7: action grouping class detection (inline, Python-to-Python) ---


def test_p7_action_grouping_class(ctx):
    import msgspec
    from typing import ClassVar

    class CountToGoal(msgspec.Struct):
        __msgtype__: ClassVar[str] = "p7_demo/msg/CountToGoal"
        target: int = 0

    class CountToResult(msgspec.Struct):
        __msgtype__: ClassVar[str] = "p7_demo/msg/CountToResult"
        final_count: int = 0

    class CountToFeedback(msgspec.Struct):
        __msgtype__: ClassVar[str] = "p7_demo/msg/CountToFeedback"
        current: int = 0

    class CountTo:
        __actiontype__: ClassVar[str] = "p7_demo/action/CountTo"
        Goal = CountToGoal
        Result = CountToResult
        Feedback = CountToFeedback

    node = ctx.create_node("p7_action").build()
    # Single grouping class instead of three positional types.
    client = node.create_action_client("/p7_count", CountTo)
    assert client is not None
    server = node.create_action_server("/p7_count", CountTo)
    assert server is not None
