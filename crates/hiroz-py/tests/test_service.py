#!/usr/bin/env python3
"""Test service/client functionality with example_interfaces/AddTwoInts."""

import hiroz_py
from hiroz_py import example_interfaces
import time
import threading


def test_context_and_node_creation():
    """Test creating a context and node."""
    print("Testing context and node creation...")
    context = (
        hiroz_py.ZContextBuilder().with_domain_id(0).with_logging_enabled().build()
    )
    assert context is not None
    node = context.create_node("test_service_node").with_namespace("/test").build()
    assert node is not None
    print("✓ Context and node created")
    return context, node


def test_server_creation(node):
    """Test creating a service server."""
    print("Testing server creation...")
    server = node.create_server("add_two_ints", example_interfaces.AddTwoIntsRequest)
    assert server is not None
    print("✓ Server created")
    return server


def test_client_creation(node):
    """Test creating a service client."""
    print("Testing client creation...")
    client = node.create_client("add_two_ints", example_interfaces.AddTwoIntsRequest)
    assert client is not None
    print("✓ Client created")
    return client


def test_request_response(server, client):
    """Test service request/response cycle."""
    print("Testing request/response...")

    server_result = {"received": False, "request_a": 0, "request_b": 0}

    def server_thread():
        try:
            request_id, req = server.take_request()
            server_result["received"] = True
            server_result["request_a"] = req.a
            server_result["request_b"] = req.b
            print(f"  Server received: {req.a} + {req.b}")
            resp = example_interfaces.AddTwoIntsResponse(sum=req.a + req.b)
            server.send_response(resp, request_id)
            print(f"  Server sent response: {resp.sum}")
        except Exception as e:
            print(f"  Server error: {e}")

    thread = threading.Thread(target=server_thread, daemon=True)
    thread.start()

    time.sleep(0.5)

    test_a, test_b = 5, 7
    req = example_interfaces.AddTwoIntsRequest(a=test_a, b=test_b)
    print(f"  Client sending: {test_a} + {test_b}")
    resp = client.call(req, timeout=5.0)

    assert resp is not None, "No response received"
    assert resp.sum == test_a + test_b, f"Expected {test_a + test_b}, got {resp.sum}"
    print(f"  Client received: {resp.sum}")

    thread.join(timeout=2.0)

    assert server_result["received"], "Server did not receive request"
    assert server_result["request_a"] == test_a, "Server received wrong value for a"
    assert server_result["request_b"] == test_b, "Server received wrong value for b"

    print("✓ Request/response cycle works")


def test_timeout_handling(client):
    """Test timeout behavior when no server is available."""
    print("Testing timeout handling...")

    context = hiroz_py.ZContextBuilder().with_domain_id(1).build()
    node = context.create_node("timeout_test_node").build()
    timeout_client = node.create_client(
        "nonexistent_service", example_interfaces.AddTwoIntsRequest
    )

    req = example_interfaces.AddTwoIntsRequest(a=1, b=2)
    try:
        timeout_client.call(req, timeout=1.0)
        assert False, "Expected timeout error, but call succeeded"
    except RuntimeError:
        pass  # expected: call timed out

    print("✓ Timeout handling works")


def test_error_handling(node):
    """Test error handling for unknown service types."""
    print("Testing error handling...")

    # With static types, passing an invalid type is caught by the type system
    # So we test with a class that doesn't have the required attributes
    class FakeService:
        pass

    try:
        _ = node.create_server("test", FakeService)
        assert False, "Should have raised error for invalid service type"
    except (TypeError, AttributeError):
        print("✓ Error handling works (invalid service type rejected)")


def main():
    print("=" * 60)
    print("hiroz-py Service Test")
    print("=" * 60)

    # Run tests
    context, node = test_context_and_node_creation()
    server = test_server_creation(node)
    client = test_client_creation(node)
    test_request_response(server, client)
    test_timeout_handling(client)
    test_error_handling(node)

    print("=" * 60)
    print("All tests passed! ✓")
    print("=" * 60)


if __name__ == "__main__":
    main()
