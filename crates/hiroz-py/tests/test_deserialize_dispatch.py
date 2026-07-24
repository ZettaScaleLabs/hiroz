#!/usr/bin/env python3
"""Guard that the subscriber decode path does not go through the Python REGISTRY.

``serialize_to_zbuf`` dispatches on the type name with a Rust ``match``,
deliberately bypassing ``hiroz_py.hiroz_msgs.REGISTRY``. ``deserialize_from_cdr``
must mirror it: it dispatches through the generated ``deserialize_direct``, so a
subscriber decodes an incoming sample without touching Python state at all.

The detector: empty the REGISTRY, then push a message through a **real
subscriber**. Direct dispatch is unaffected and the message arrives; a registry
walk cannot resolve anything and the message is dropped (the callback path logs
``deserialization error in callback`` and never fires; the ``recv`` path raises).

Note that the exported ``hiroz_msgs.deserialize_message`` pyfunction is
intentionally still registry-backed — it is the public escape hatch for types
resolved at runtime. Asserting on *it* would fail on both a patched and an
unpatched build and prove nothing, so these tests deliberately go through
``node.create_subscriber`` instead, which is the path
``crates/hiroz-py/src/node.rs`` and ``pubsub.rs`` actually use.
"""

import threading
import time

import pytest

from hiroz_py import hiroz_msgs, std_msgs


@pytest.fixture
def empty_registry():
    """Empty ``hiroz_msgs.REGISTRY`` for one test, then restore it.

    Restoration is mandatory: the REGISTRY is module-global, so leaking an empty
    one would break every later test that legitimately resolves through it.
    """
    saved = dict(hiroz_msgs.REGISTRY)
    assert saved, "REGISTRY was already empty - the detector would be vacuous"
    hiroz_msgs.REGISTRY.clear()
    try:
        yield
    finally:
        hiroz_msgs.REGISTRY.update(saved)


def test_callback_subscriber_decodes_without_registry(node, empty_registry):
    """The callback decode path (node.rs) must not consult the REGISTRY."""
    received = []
    arrived = threading.Event()

    def on_msg(msg):
        received.append(msg)
        arrived.set()

    # Build the endpoints before emptying matters: type info comes from the
    # class's __msgtype__, not the REGISTRY, so ordering is not load-bearing -
    # but keeping it explicit documents what the detector is and is not probing.
    sub = node.create_subscriber("/dispatch_cb", std_msgs.String, callback=on_msg)
    assert sub is not None
    pub = node.create_publisher("/dispatch_cb", std_msgs.String)
    time.sleep(0.5)

    payload = "registry-free callback decode"
    pub.publish(std_msgs.String(data=payload))

    assert arrived.wait(timeout=5.0), (
        "no message reached the callback with an empty REGISTRY - the subscriber "
        "decode path is resolving through hiroz_msgs.REGISTRY instead of "
        "dispatching directly"
    )
    assert received[0].data == payload


def test_recv_subscriber_decodes_without_registry(node, empty_registry):
    """The recv decode path (pubsub.rs) must not consult the REGISTRY either."""
    sub = node.create_subscriber("/dispatch_recv", std_msgs.String)
    pub = node.create_publisher("/dispatch_recv", std_msgs.String)
    time.sleep(0.5)

    payload = "registry-free recv decode"
    pub.publish(std_msgs.String(data=payload))

    msg = sub.recv(timeout=5.0)
    assert msg is not None, (
        "recv returned nothing with an empty REGISTRY - the subscriber decode "
        "path is resolving through hiroz_msgs.REGISTRY instead of dispatching "
        "directly"
    )
    assert msg.data == payload


def test_exported_deserialize_message_still_uses_registry():
    """Pin the deliberate asymmetry, so the two paths are not "fixed" together.

    The exported ``serialize_message``/``deserialize_message`` pair is the
    runtime-dispatch escape hatch and *both* halves resolve through the REGISTRY.
    The subscriber path above is the codegen-time one and resolves through
    neither. If this test ever starts passing, the escape hatch has been rewired
    and types registered at runtime will no longer round-trip.

    The message must be serialized *before* the REGISTRY is emptied - the encode
    half needs it too, so this test cannot use the ``empty_registry`` fixture.
    """
    type_name = "std_msgs/msg/String"
    raw = bytes(hiroz_msgs.serialize_message(type_name, std_msgs.String(data="x")))

    saved = dict(hiroz_msgs.REGISTRY)
    hiroz_msgs.REGISTRY.clear()
    try:
        with pytest.raises(Exception, match="Unknown message type"):
            hiroz_msgs.deserialize_message(type_name, raw)
    finally:
        hiroz_msgs.REGISTRY.update(saved)
