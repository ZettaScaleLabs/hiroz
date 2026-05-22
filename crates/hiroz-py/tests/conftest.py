#!/usr/bin/env python3
"""Pytest fixtures for hiroz-py tests."""

import pytest
import hiroz_py
from hiroz_py import std_msgs, example_interfaces


@pytest.fixture(scope="session")
def context():
    """Create a Zenoh context for testing."""
    c = hiroz_py.ZContextBuilder().with_domain_id(0).build()
    yield c
    # Cleanup happens automatically when context goes out of scope


@pytest.fixture(scope="function")
def node(context):
    """Create a test node."""
    n = context.create_node("test_node").with_namespace("/test").build()
    yield n
    # Cleanup happens automatically


@pytest.fixture(scope="function")
def pub(node):
    """Create a test publisher."""
    p = node.create_publisher("/chatter", std_msgs.String)
    yield p


@pytest.fixture(scope="function")
def sub(node):
    """Create a test subscriber."""
    s = node.create_subscriber("/chatter", std_msgs.String)
    yield s


@pytest.fixture(scope="function")
def server(node):
    """Create a test service server."""
    srv = node.create_server("add_two_ints", example_interfaces.AddTwoIntsRequest)
    yield srv


@pytest.fixture(scope="function")
def client(node):
    """Create a test service client."""
    cli = node.create_client("add_two_ints", example_interfaces.AddTwoIntsRequest)
    yield cli
