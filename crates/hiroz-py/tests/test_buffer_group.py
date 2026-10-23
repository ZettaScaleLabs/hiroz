"""Tests for the buffer_group argument of create_subscriber."""

import ast
import importlib.util
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


@pytest.mark.parametrize("group", ["AUTO", "Off", "on", "", "cpu "])
def test_create_subscriber_rejects_unknown_group(node, group):
    with pytest.raises(ValueError, match="buffer_group"):
        node.create_subscriber("/group_bad", std_msgs.String, buffer_group=group)


def test_create_subscriber_rejects_a_non_string_group(node):
    with pytest.raises(TypeError, match="buffer_group"):
        node.create_subscriber("/group_bad", std_msgs.String, buffer_group=1)


def test_stub_declares_buffer_group_with_default_auto():
    fn = _create_subscriber_signature()
    names = [a.arg for a in fn.args.args]
    assert "buffer_group" in names, "the stub lacks buffer_group"
    offset = len(names) - len(fn.args.defaults)
    default = fn.args.defaults[names.index("buffer_group") - offset]
    assert isinstance(default, ast.Constant) and default.value == "auto"
