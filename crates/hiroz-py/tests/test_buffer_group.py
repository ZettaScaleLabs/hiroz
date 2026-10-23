"""Tests for the buffer_group argument of create_subscriber."""

import ast
import importlib.util
import time
from pathlib import Path

import pytest
from hiroz_py import std_msgs


def _create_subscriber_signature():
    spec = importlib.util.find_spec("hiroz_py")
    assert spec is not None
    stub = Path(spec.submodule_search_locations[0]) / "__init__.pyi"
    tree = ast.parse(stub.read_text())
    for cls in ast.walk(tree):
        if isinstance(cls, ast.ClassDef) and cls.name == "ZNode":
            for fn in cls.body:
                if isinstance(fn, ast.FunctionDef) and fn.name == "create_subscriber":
                    return fn
    raise AssertionError("ZNode.create_subscriber is not in the stub")


@pytest.mark.parametrize("group", ["auto", "cpu", "off"])
def test_create_subscriber_accepts_each_group(node, group):
    sub = node.create_subscriber(f"/group_{group}", std_msgs.String, buffer_group=group)
    assert sub is not None


def test_create_subscriber_without_group_still_works(node):
    sub = node.create_subscriber("/group_omitted", std_msgs.String)
    assert sub is not None


@pytest.mark.parametrize("group", ["AUTO", "Off", "on", "", "cpu "])
def test_create_subscriber_rejects_unknown_group(node, group):
    with pytest.raises(ValueError, match="buffer_group"):
        node.create_subscriber("/group_bad", std_msgs.String, buffer_group=group)


def test_create_subscriber_rejects_a_non_string_group(node):
    with pytest.raises(TypeError, match="buffer_group"):
        node.create_subscriber("/group_bad", std_msgs.String, buffer_group=1)


@pytest.mark.parametrize("group", ["auto", "cpu", "off"])
def test_each_group_delivers_a_message(node, group):
    topic = f"/group_delivery_{group}"
    pub = node.create_publisher(topic, std_msgs.String)
    sub = node.create_subscriber(topic, std_msgs.String, buffer_group=group)
    time.sleep(0.5)
    pub.publish(std_msgs.String(data=group))
    msg = sub.recv(timeout=2.0)
    assert msg is not None, f"no message with buffer_group={group!r}"
    assert msg.data == group


def test_callback_subscriber_accepts_a_group(node):
    received = []
    node.create_subscriber(
        "/group_callback",
        std_msgs.String,
        callback=received.append,
        buffer_group="off",
    )
    pub = node.create_publisher("/group_callback", std_msgs.String)
    time.sleep(0.5)
    pub.publish(std_msgs.String(data="cb"))
    time.sleep(0.3)
    assert [m.data for m in received] == ["cb"]


def test_stub_declares_buffer_group_with_default_auto():
    fn = _create_subscriber_signature()
    names = [a.arg for a in fn.args.args]
    assert "buffer_group" in names, "the stub lacks buffer_group"
    offset = len(names) - len(fn.args.defaults)
    default = fn.args.defaults[names.index("buffer_group") - offset]
    assert isinstance(default, ast.Constant) and default.value == "auto"
