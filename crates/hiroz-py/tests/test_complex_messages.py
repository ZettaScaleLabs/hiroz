#!/usr/bin/env python3
"""Test complex message types with nested structures."""

import hiroz_py
from hiroz_py import geometry_msgs
import time


def test_vector3(node):
    """Test geometry_msgs/Vector3 serialization."""
    print("Testing Vector3...")

    # Create publisher and subscriber
    pub = node.create_publisher("/vector", geometry_msgs.Vector3)
    sub = node.create_subscriber("/vector", geometry_msgs.Vector3)

    time.sleep(0.5)

    # Test data
    test_msg = geometry_msgs.Vector3(x=1.0, y=2.0, z=3.0)
    pub.publish(test_msg)
    print(f"  Published: Vector3(x={test_msg.x}, y={test_msg.y}, z={test_msg.z})")

    # Receive and validate
    msg = sub.recv(timeout=2.0)
    assert msg is not None, "No message received"
    assert msg.x == 1.0
    assert msg.y == 2.0
    assert msg.z == 3.0
    print(f"  Received: Vector3(x={msg.x}, y={msg.y}, z={msg.z})")
    print("✓ Vector3 works")


def test_twist(node):
    """Test geometry_msgs/Twist serialization (nested message)."""
    print("Testing Twist (nested message)...")

    # Create publisher and subscriber
    pub = node.create_publisher("/cmd_vel", geometry_msgs.Twist)
    sub = node.create_subscriber("/cmd_vel", geometry_msgs.Twist)

    time.sleep(0.5)

    # Test data with nested Vector3 messages
    test_msg = geometry_msgs.Twist(
        linear=geometry_msgs.Vector3(x=1.0, y=0.0, z=0.0),
        angular=geometry_msgs.Vector3(x=0.0, y=0.0, z=0.5),
    )
    pub.publish(test_msg)
    print(
        f"  Published: Twist(linear=({test_msg.linear.x}, {test_msg.linear.y}, {test_msg.linear.z}), "
        f"angular=({test_msg.angular.x}, {test_msg.angular.y}, {test_msg.angular.z}))"
    )

    # Receive and validate
    msg = sub.recv(timeout=2.0)
    assert msg is not None, "No message received"
    assert msg.linear.x == 1.0
    assert msg.linear.y == 0.0
    assert msg.linear.z == 0.0
    assert msg.angular.x == 0.0
    assert msg.angular.y == 0.0
    assert msg.angular.z == 0.5
    print(
        f"  Received: Twist(linear=({msg.linear.x}, {msg.linear.y}, {msg.linear.z}), "
        f"angular=({msg.angular.x}, {msg.angular.y}, {msg.angular.z}))"
    )
    print("✓ Twist works")


def main():
    print("=" * 60)
    print("hiroz-py Complex Messages Test")
    print("=" * 60)

    # Setup
    context = hiroz_py.ZContextBuilder().with_domain_id(0).build()
    node = context.create_node("test_complex_node").with_namespace("/test").build()

    # Run tests
    test_vector3(node)
    test_twist(node)

    print("=" * 60)
    print("All tests passed! ✓")
    print("=" * 60)


if __name__ == "__main__":
    main()
