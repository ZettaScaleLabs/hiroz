#!/usr/bin/env python3
"""
Topic pub/sub demo - publishes or subscribes to String messages.

Usage:
    python topic_demo.py -r talker              # Run as publisher
    python topic_demo.py -r listener            # Run as subscriber
    python topic_demo.py -r talker -e tcp/...   # Connect to specific endpoint
"""

import argparse
import sys
import time

import hiroz_py
from hiroz_py import std_msgs


# --8<-- [start:run_talker]
def run_talker(ctx, topic: str, count: int, interval: float):
    """Run the talker (publisher)."""
    node = ctx.create_node("talker").build()
    pub = node.create_publisher(topic, std_msgs.String)

    print(f"Talker started. Publishing to {topic}...")

    # Wait for at least one subscriber instead of racing (P1).
    pub.wait_for_subscription(count=1, timeout=5.0)

    i = 0
    while count == 0 or i < count:
        message = f"Hello from Python {i}"
        msg = std_msgs.String(data=message)
        pub.publish(msg)
        print(f"PUB:{i}", flush=True)
        i += 1
        time.sleep(interval)

    print("PUB:DONE", flush=True)


# --8<-- [end:run_talker]


# --8<-- [start:run_listener]
def run_listener(ctx, topic: str, timeout: float):
    """Run the listener (subscriber)."""
    node = ctx.create_node("listener").build()
    sub = node.create_subscriber(topic, std_msgs.String)

    print("SUB:READY", flush=True)

    start = time.time()
    received = 0

    while timeout == 0 or (time.time() - start) < timeout:
        msg = sub.recv(timeout=1.0)
        if msg is not None:
            print(f"SUB:{msg.data}", flush=True)
            received += 1

    print(f"SUB:TOTAL:{received}", flush=True)


# --8<-- [end:run_listener]


def main():
    parser = argparse.ArgumentParser(description="Topic pub/sub demo")
    parser.add_argument(
        "-r",
        "--role",
        type=str,
        required=True,
        choices=["talker", "listener"],
        help="Role: talker (publisher) or listener (subscriber)",
    )
    parser.add_argument(
        "-t",
        "--topic",
        type=str,
        default="/chatter",
        help="Topic name (default: /chatter)",
    )
    parser.add_argument(
        "-e",
        "--endpoint",
        type=str,
        default=None,
        help="Zenoh endpoint to connect to (e.g., tcp/127.0.0.1:7447)",
    )
    parser.add_argument(
        "-c",
        "--count",
        type=int,
        default=0,
        help="Number of messages to publish (0 for unlimited, talker only)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=0,
        help="Timeout in seconds (0 for unlimited, listener only)",
    )
    parser.add_argument(
        "--interval",
        type=float,
        default=0.5,
        help="Interval between messages in seconds (talker only)",
    )

    args = parser.parse_args()

    # Build context
    builder = hiroz_py.ZContextBuilder()
    if args.endpoint:
        builder = builder.with_connect_endpoints([args.endpoint])
        builder = builder.disable_multicast_scouting()

    ctx = builder.build()

    if args.role == "talker":
        run_talker(ctx, args.topic, args.count, args.interval)
    else:
        run_listener(ctx, args.topic, args.timeout)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nStopped.", file=sys.stderr)
