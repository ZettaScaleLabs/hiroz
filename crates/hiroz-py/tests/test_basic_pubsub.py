#!/usr/bin/env python3
"""Test basic pub/sub functionality with std_msgs/String."""

import hiroz_py
from hiroz_py import std_msgs
import time


def test_context_creation():
    """Test creating a context."""
    print("Testing context creation...")
    context = hiroz_py.ZContextBuilder().with_domain_id(0).build()
    assert context is not None
    print("✓ Context created")
    return context


def test_node_creation(context):
    """Test creating a node."""
    print("Testing node creation...")
    node = context.create_node("test_node").with_namespace("/test").build()
    assert node is not None
    print("✓ Node created")
    return node


def test_publisher_creation(node):
    """Test creating a publisher."""
    print("Testing publisher creation...")
    pub = node.create_publisher("/chatter", std_msgs.String)
    assert pub is not None
    print("✓ Publisher created")
    return pub


def test_subscriber_creation(node):
    """Test creating a subscriber."""
    print("Testing subscriber creation...")
    sub = node.create_subscriber("/chatter", std_msgs.String)
    assert sub is not None
    print("✓ Subscriber created")
    return sub


def test_publish_receive(pub, sub):
    """Test publishing and receiving a message."""
    print("Testing publish/receive...")

    # Give subscriber time to set up
    time.sleep(0.5)

    # Publish message
    test_data = "Hello, hiroz-py!"
    msg_to_send = std_msgs.String(data=test_data)
    pub.publish(msg_to_send)
    print(f"  Published: {test_data}")

    # Receive message
    msg = sub.recv(timeout=2.0)
    assert msg is not None, "No message received"
    assert hasattr(msg, "data"), "Message missing 'data' field"
    assert msg.data == test_data, f"Expected '{test_data}', got '{msg.data}'"
    print(f"  Received: {msg.data}")
    print("✓ Publish/receive works")


def test_qos_configuration(node):
    """Test QoS configuration."""
    print("Testing QoS configuration...")

    qos = {
        "reliability": "reliable",
        "durability": "volatile",
        "history": "keep_last",
        "depth": 10,
    }

    pub = node.create_publisher("/qos_topic", std_msgs.String, qos=qos)
    assert pub is not None
    print("✓ QoS configuration accepted")


def test_callback_subscriber_no_assign(node, pub):
    """Regression test: callback subscriber must work even when result is not stored.

    Previously, the ZSub handle was dropped immediately if the caller did not assign
    the return value of create_subscriber(), causing the Zenoh subscription to be
    undeclared before any messages arrived. The node now owns callback subscribers
    internally (matching rmw_zenoh_cpp's NodeData::subs_ pattern).
    """
    received = []
    # Intentionally NOT assigned — this was the bug
    node.create_subscriber("/chatter", std_msgs.String, callback=received.append)
    time.sleep(0.3)
    pub.publish(std_msgs.String(data="background_test"))
    time.sleep(0.3)
    assert len(received) == 1, f"Expected 1 message, got {len(received)}"
    assert received[0].data == "background_test"


def test_destroy_subscriber(node, pub):
    """destroy_subscriber must undeclare the subscription early.

    After destroy_subscriber(), messages published to the topic must not
    reach the callback anymore.
    """
    received = []
    sub = node.create_subscriber("/chatter", std_msgs.String, callback=received.append)
    time.sleep(0.3)
    pub.publish(std_msgs.String(data="before_destroy"))
    time.sleep(0.3)
    assert len(received) == 1, f"Expected 1 message before destroy, got {len(received)}"

    node.destroy_subscriber(sub)
    pub.publish(std_msgs.String(data="after_destroy"))
    time.sleep(0.3)
    assert len(received) == 1, (
        f"Expected no new messages after destroy, got {len(received)}"
    )


def test_error_handling(node):
    """Test error handling for invalid message types."""
    print("Testing error handling...")

    # Passing a non-class type should raise TypeError
    try:
        _ = node.create_publisher("/test", "not_a_class")  # type: ignore
        assert False, "Should have raised error for invalid type"
    except TypeError:
        print("✓ Error handling works (invalid type rejected)")


def main():
    print("=" * 60)
    print("hiroz-py Basic Pub/Sub Test")
    print("=" * 60)

    # Run tests
    context = test_context_creation()
    node = test_node_creation(context)
    pub = test_publisher_creation(node)
    sub = test_subscriber_creation(node)
    test_publish_receive(pub, sub)
    test_qos_configuration(node)
    test_error_handling(node)
    test_callback_subscriber_no_assign(node, pub)
    test_destroy_subscriber(node, pub)

    print("=" * 60)
    print("All tests passed! ✓")
    print("=" * 60)


if __name__ == "__main__":
    main()
