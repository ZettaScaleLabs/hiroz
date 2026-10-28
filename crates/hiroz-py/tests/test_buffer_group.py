"""Tests for the buffer_group argument of create_subscriber."""

import pytest
from hiroz_py import std_msgs


@pytest.mark.parametrize("group", ["AUTO", "Off", "on", "", "cpu "])
def test_create_subscriber_rejects_unknown_group(node, group):
    with pytest.raises(ValueError, match="buffer_group"):
        node.create_subscriber("/group_bad", std_msgs.String, buffer_group=group)


def test_create_subscriber_rejects_a_non_string_group(node):
    with pytest.raises(TypeError, match="buffer_group"):
        node.create_subscriber("/group_bad", std_msgs.String, buffer_group=1)
