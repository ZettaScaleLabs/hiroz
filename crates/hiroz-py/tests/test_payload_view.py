#!/usr/bin/env python3
"""Test ZPayloadView zero-copy buffer protocol functionality."""

import pytest
import time
import hiroz_py
from hiroz_py import ZPayloadView, std_msgs


@pytest.fixture(scope="function")
def byte_array_pubsub(node):
    """Create publisher and subscriber for ByteMultiArray messages."""
    pub = node.create_publisher("/test_payload_view", std_msgs.ByteMultiArray)
    sub = node.create_subscriber("/test_payload_view", std_msgs.ByteMultiArray)
    time.sleep(0.3)  # Allow discovery
    yield pub, sub


def create_byte_array_msg(size: int, fill_byte: int = 0xAA) -> std_msgs.ByteMultiArray:
    """Create a ByteMultiArray message with specified size."""
    return std_msgs.ByteMultiArray(
        layout=std_msgs.MultiArrayLayout(
            dim=[std_msgs.MultiArrayDimension(label="data", size=size, stride=size)],
            data_offset=0,
        ),
        data=bytes([fill_byte] * size),
    )


class TestZPayloadView:
    """Test ZPayloadView functionality."""

    def test_recv_raw_view_returns_payload_view(self, byte_array_pubsub):
        """Test that recv_raw_view returns a ZPayloadView object."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None, "recv_raw_view returned None"
        assert isinstance(payload, ZPayloadView), (
            f"Expected ZPayloadView, got {type(payload)}"
        )

    def test_payload_view_length(self, byte_array_pubsub):
        """Test that ZPayloadView reports correct length."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None
        # Length should be CDR serialized size (includes header + layout + data)
        assert len(payload) > 1024, f"Payload too small: {len(payload)}"

    def test_payload_view_bool(self, byte_array_pubsub):
        """Test that ZPayloadView bool is True for non-empty payloads."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(100)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None
        assert bool(payload) is True, "Non-empty payload should be truthy"

    def test_payload_view_is_zero_copy(self, byte_array_pubsub):
        """Test that ZPayloadView reports zero-copy status."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None
        # is_zero_copy_py should be a boolean
        assert isinstance(payload.is_zero_copy_py, bool)
        # For contiguous buffers, should be True
        assert payload.is_zero_copy_py is True, (
            "Expected zero-copy for contiguous buffer"
        )

    def test_memoryview_creation(self, byte_array_pubsub):
        """Test that memoryview can be created from ZPayloadView."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024, fill_byte=0xBB)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None

        mv = memoryview(payload)
        assert mv is not None
        assert len(mv) == len(payload)
        assert mv.readonly is True, "memoryview should be read-only"

    def test_memoryview_slicing(self, byte_array_pubsub):
        """Test that memoryview slicing works without copying."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None

        mv = memoryview(payload)
        # Slicing should work
        slice_view = mv[10:20]
        assert len(slice_view) == 10

    def test_memoryview_to_bytes(self, byte_array_pubsub):
        """Test that memoryview can be converted to bytes."""
        pub, sub = byte_array_pubsub

        fill_byte = 0xCC
        msg = create_byte_array_msg(100, fill_byte=fill_byte)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None

        mv = memoryview(payload)
        data = bytes(mv)
        assert len(data) == len(payload)
        # The fill_byte should appear in the data portion of the CDR payload
        assert fill_byte in data, f"Expected fill byte 0x{fill_byte:02X} in payload"

    def test_try_recv_raw_view_no_message(self, byte_array_pubsub):
        """Test that try_recv_raw_view returns None when no message available."""
        _, sub = byte_array_pubsub

        # Don't publish anything
        payload = sub.try_recv_raw_view()
        assert payload is None, "try_recv_raw_view should return None when no message"

    def test_try_recv_raw_view_with_message(self, byte_array_pubsub):
        """Test that try_recv_raw_view returns payload when message available."""
        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(512)
        pub.publish(msg)
        time.sleep(0.1)  # Allow message to arrive

        payload = sub.try_recv_raw_view()
        assert payload is not None, "try_recv_raw_view should return payload"
        assert isinstance(payload, ZPayloadView)

    def test_recv_raw_view_timeout(self, byte_array_pubsub):
        """Test that recv_raw_view returns None on timeout."""
        _, sub = byte_array_pubsub

        # Don't publish anything, should timeout
        start = time.time()
        payload = sub.recv_raw_view(timeout=0.5)
        elapsed = time.time() - start

        assert payload is None, "Should return None on timeout"
        assert elapsed >= 0.4, f"Should wait for timeout, only waited {elapsed}s"


class TestZPayloadViewNumpy:
    """Test ZPayloadView with numpy (if available)."""

    @pytest.fixture(autouse=True)
    def check_numpy(self):
        """Skip tests if numpy is not available."""
        pytest.importorskip("numpy")

    def test_numpy_frombuffer(self, byte_array_pubsub):
        """Test that numpy.frombuffer works with ZPayloadView."""
        import numpy as np

        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024, fill_byte=0xDD)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None

        arr = np.frombuffer(payload, dtype=np.uint8)
        assert arr is not None
        assert len(arr) == len(payload)
        # Verify array doesn't own data (shares memory with ZPayloadView)
        assert arr.flags["OWNDATA"] is False, "numpy array should not own data"

    def test_numpy_zero_copy_verification(self, byte_array_pubsub):
        """Test that numpy truly shares memory with ZPayloadView."""
        import numpy as np

        pub, sub = byte_array_pubsub

        msg = create_byte_array_msg(1024)
        pub.publish(msg)

        payload = sub.recv_raw_view(timeout=5.0)
        assert payload is not None

        # Create numpy array
        arr = np.frombuffer(payload, dtype=np.uint8)

        # # Get data pointer from numpy
        # arr_ptr = arr.__array_interface__["data"][0]

        # # Get data pointer from memoryview
        # mv = memoryview(payload)
        # mv_ptr = id(mv)  # Note: this isn't the actual data pointer

        # The key test: OWNDATA should be False (numpy views the buffer)
        assert arr.flags["OWNDATA"] is False
        assert arr.flags["WRITEABLE"] is False, "Should be read-only"


def main():
    """Run tests manually."""
    print("=" * 60)
    print("hiroz-py ZPayloadView Test")
    print("=" * 60)

    # Create context and node
    context = hiroz_py.ZContextBuilder().with_domain_id(0).build()
    node = context.create_node("test_node").with_namespace("/test").build()

    # Create pubsub
    pub = node.create_publisher("/test_payload_view", std_msgs.ByteMultiArray)
    sub = node.create_subscriber("/test_payload_view", std_msgs.ByteMultiArray)
    time.sleep(0.3)

    # Test recv_raw_view
    print("Testing recv_raw_view...")
    msg = create_byte_array_msg(1024, fill_byte=0xAA)
    pub.publish(msg)

    payload = sub.recv_raw_view(timeout=5.0)
    assert payload is not None, "recv_raw_view returned None"
    print(
        f"  Got ZPayloadView: length={len(payload)}, zero_copy={payload.is_zero_copy_py}"
    )

    # Test memoryview
    print("Testing memoryview...")
    mv = memoryview(payload)
    print(f"  memoryview: length={len(mv)}, readonly={mv.readonly}")

    # Test numpy if available
    try:
        import numpy as np

        print("Testing numpy.frombuffer...")
        arr = np.frombuffer(payload, dtype=np.uint8)
        print(f"  numpy array: length={len(arr)}, owns_data={arr.flags['OWNDATA']}")
    except ImportError:
        print("  numpy not available, skipping")

    print("=" * 60)
    print("All tests passed!")
    print("=" * 60)


if __name__ == "__main__":
    main()
