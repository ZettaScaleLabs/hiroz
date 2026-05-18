#!/usr/bin/env python3
"""
External Zenoh subscriber demonstrating interoperability with hiroz.

This Python script subscribes to hiroz topics and decodes the encoding metadata,
showing how non-ROS applications can interact with hiroz using Zenoh.

Requirements:
    pip install zenoh

Usage:
    # Terminal 1: Start hiroz publisher
    cargo run --example protobuf_interop --features protobuf -- pub

    # Terminal 2: Run this script
    python3 examples/zenoh_subscriber.py
"""

import zenoh
import time
import sys


def main():
    print("=== External Zenoh Subscriber ===\n")
    print("Demonstrating interoperability with hiroz Protobuf encoding\n")

    # Open Zenoh session
    conf = zenoh.Config()
    session = zenoh.open(conf)

    print("Zenoh session opened")
    print("\nSubscribing to:")
    print("  - /interop/protobuf (expects Protobuf encoding)")
    print("  - /interop/cdr (expects CDR encoding)")
    print("\nWaiting for messages...\n")

    def protobuf_callback(sample):
        """Handle Protobuf-encoded messages"""
        encoding = sample.encoding
        payload = sample.payload.to_bytes()

        print("[PROTO] Received message")
        print(f"        Encoding: {encoding}")
        print(f"        Payload size: {len(payload)} bytes")
        print(f"        Raw data: {payload[:50]}...")  # Show first 50 bytes

        # In a real application, you would decode the Protobuf here
        # using the schema from encoding.schema
        if "schema=" in str(encoding):
            schema = str(encoding).split("schema=")[1]
            print(f"        Schema: {schema}")
        print()

    def cdr_callback(sample):
        """Handle CDR-encoded messages"""
        encoding = sample.encoding
        payload = sample.payload.to_bytes()

        print("[CDR]   Received message")
        print(f"        Encoding: {encoding}")
        print(f"        Payload size: {len(payload)} bytes")
        print(f"        Raw data: {payload[:50]}...")  # Show first 50 bytes
        print()

    # Subscribe to both topics
    sub_proto = session.declare_subscriber("/interop/protobuf", protobuf_callback)

    sub_cdr = session.declare_subscriber("/interop/cdr", cdr_callback)

    print("✓ Subscriptions active\n")
    print("Press Ctrl+C to exit\n")

    try:
        # Keep alive
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print("\n\nShutting down...")

    # Cleanup
    sub_proto.undeclare()
    sub_cdr.undeclare()
    session.close()

    print("=== Complete ===")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
