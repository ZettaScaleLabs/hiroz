"""Tests that the buffer_group argument of create_subscriber sets the policy that takes effect.

A plain Zenoh session plays the publisher. It reads the liveliness token of the subscriber, and it
writes to the group key `<base>/_buf_cpu`. A subscriber that joined announces `backends:cpu:` in
its token and receives the group key. A subscriber that did not join keeps its token, without the
`backends:cpu:` part, and receives the base key only.

The tests check the token and the group-key delivery of each value, in the queue form and in the
callback form.

Each case has live controls. A second subscriber created with `buffer_group='cpu'` shows that the
token query and the group key work in the same run. A put on the base key shows that the subscriber
under test is alive. The tests cannot tell `auto` from `cpu`, because the two behave the same in
Python.
"""

import socket
import struct
import time

import pytest
import zenoh
from hiroz_py import ZContextBuilder, std_msgs

DOMAIN = 0
LISTEN_WAIT = 5.0
QUIET = 1.5
POLL = 0.1

# The default of the argument: the case passes no buffer_group at all.
OMITTED = None

POLICIES = [
    pytest.param("off", False, id="off"),
    pytest.param("cpu", True, id="cpu"),
    pytest.param("auto", True, id="auto"),
    pytest.param(OMITTED, True, id="omitted"),
]
FORMS = ["queue", "callback"]


def _free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def rig():
    """A plain Zenoh session that listens, and a hiroz node that connects to it."""
    endpoint = f"tcp/127.0.0.1:{_free_port()}"
    config = zenoh.Config()
    config.insert_json5("listen/endpoints", f'["{endpoint}"]')
    config.insert_json5("scouting/multicast/enabled", "false")
    session = zenoh.open(config)
    context = (
        ZContextBuilder()
        .with_domain_id(DOMAIN)
        .with_connect_endpoints([endpoint])
        .disable_multicast_scouting()
        .build()
    )
    node = context.create_node("group_policy_node").build()
    yield session, node
    session.close()


def _cdr_string(text):
    raw = text.encode() + b"\x00"
    return b"\x00\x01\x00\x00" + struct.pack("<I", len(raw)) + raw


def _tokens_of(session, name):
    """The liveliness tokens that name `name` as their topic."""
    needle = f"/%{name}/"
    keys = []
    for reply in session.liveliness().get("@ros2_lv/**", timeout=2.0):
        assert reply.ok is not None, f"liveliness query returned an error: {reply.err}"
        key = str(reply.ok.key_expr)
        if needle in key:
            keys.append(key)
    return keys


def _token_for(session, name):
    """The one token of `name`, once it is visible. No token, or two, fails the case."""
    deadline = time.monotonic() + LISTEN_WAIT
    while True:
        hits = _tokens_of(session, name)
        if len(hits) == 1:
            return hits[0]
        if len(hits) > 1 or time.monotonic() >= deadline:
            raise AssertionError(f"{len(hits)} tokens for {name}: {hits}")
        time.sleep(POLL)


def _base_key(token, name):
    """The key a publisher writes `name` to, read from the segments of the token."""
    parts = token.split("/")
    i = parts.index(f"%{name}")
    return f"{DOMAIN}/{name}/{parts[i + 1]}/{parts[i + 2]}"


class _Probe:
    """A subscriber of either form, and what it has received."""

    def __init__(self, node, topic, form, group):
        self.form = form
        self.seen = []
        kwargs = {} if group is OMITTED else {"buffer_group": group}
        if form == "callback":
            kwargs["callback"] = self.seen.append
        self.sub = node.create_subscriber(topic, std_msgs.String, **kwargs)

    def data(self):
        if self.form == "queue":
            while (msg := self.sub.try_recv()) is not None:
                self.seen.append(msg)
        return [m.data for m in self.seen]


def _put_until_seen(session, key, probe, tag, limit=LISTEN_WAIT):
    """Put `tag` on `key` until `probe` has received it or `limit` seconds pass."""
    end = time.monotonic() + limit
    while time.monotonic() < end:
        session.put(key, _cdr_string(tag))
        time.sleep(0.05)
        if tag in probe.data():
            return True
    return False


def _put_quietly(session, key, probe, tag):
    """Put `tag` on `key` for a fixed window. True when `probe` never received it."""
    end = time.monotonic() + QUIET
    while time.monotonic() < end:
        session.put(key, _cdr_string(tag))
        time.sleep(0.05)
    return tag not in probe.data()


@pytest.mark.parametrize("form", FORMS)
@pytest.mark.parametrize("group,joins", POLICIES)
def test_policy_takes_effect(rig, form, group, joins):
    session, node = rig
    label = "omitted" if group is OMITTED else group
    name = f"policy_{form}_{label}"
    twin_name = f"{name}_twin"

    probe = _Probe(node, f"/{name}", form, group)
    twin = _Probe(node, f"/{twin_name}", form, "cpu")

    # Control: the twin joined, so the token query sees a joined subscriber.
    twin_token = _token_for(session, twin_name)
    assert twin_token.endswith("/backends:cpu:"), (
        f"control: the cpu twin did not announce backends:cpu: {twin_token}"
    )
    # The token of the subscriber under test announces its policy.
    token = _token_for(session, name)
    assert token.endswith("/backends:cpu:") == joins, (
        f"buffer_group={label!r} ({form} form): expected joined={joins}, token is {token}"
    )

    base = _base_key(token, name)
    group_key = f"{base}/_buf_cpu"
    twin_group_key = f"{_base_key(twin_token, twin_name)}/_buf_cpu"

    # Control: a put on the base key reaches the subscriber under test.
    assert _put_until_seen(session, base, probe, "base"), (
        f"control: nothing reached the {form} subscriber from {base}"
    )
    # Control: a put on a joined subscriber's group key arrives in this run.
    assert _put_until_seen(session, twin_group_key, twin, "group"), (
        f"control: nothing reached the cpu twin from {twin_group_key}"
    )

    if joins:
        assert _put_until_seen(session, group_key, probe, "group"), (
            f"buffer_group={label!r} ({form} form) joined but the group key does not deliver"
        )
    else:
        assert _put_quietly(session, group_key, probe, "group"), (
            f"buffer_group={label!r} ({form} form) did not join but received the group key"
        )
