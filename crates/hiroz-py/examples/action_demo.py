#!/usr/bin/env python3
"""
CountTo action demo - runs as server or client.

The server counts from 0 to a target value, publishing feedback at each step,
then returns the final count as the result.  The client can cancel mid-count.

Usage:
    python action_demo.py -r server                    # Run as server
    python action_demo.py -r client -t 5               # Count to 5
    python action_demo.py -r client -t 10 --cancel 3   # Cancel after 3 s
    python action_demo.py -r server -e tcp/...         # Connect to endpoint
"""

import argparse
import sys
import threading
import time
from typing import ClassVar

import msgspec

import hiroz_py


# ---------------------------------------------------------------------------
# Inline message types (Python-to-Python; no ROS 2 type hash)
# ---------------------------------------------------------------------------


# --8<-- [start:message_types]
class CountToGoal(msgspec.Struct):
    __msgtype__: ClassVar[str] = "action_demo/msg/CountToGoal"
    target: int = 10


class CountToResult(msgspec.Struct):
    __msgtype__: ClassVar[str] = "action_demo/msg/CountToResult"
    final_count: int = 0


class CountToFeedback(msgspec.Struct):
    __msgtype__: ClassVar[str] = "action_demo/msg/CountToFeedback"
    current: int = 0


# --8<-- [end:message_types]


# ---------------------------------------------------------------------------
# Server
# ---------------------------------------------------------------------------


# --8<-- [start:run_server]
def run_server(ctx, action: str):
    node = ctx.create_node("count_to_server").build()
    server = node.create_action_server(
        action, CountToGoal, CountToResult, CountToFeedback
    )

    print("SERVER:READY", flush=True)

    while True:
        request = server.recv_goal(timeout=1.0)
        if request is None:
            continue

        goal = request.goal()
        print(f"SERVER:GOAL:{goal.target}", flush=True)
        executing = request.accept_and_execute()

        count = 0
        while count < goal.target:
            time.sleep(0.2)
            count += 1
            executing.publish_feedback(CountToFeedback(current=count))
            print(f"SERVER:FEEDBACK:{count}", flush=True)

            if executing.is_cancel_requested:
                print(f"SERVER:CANCELED:{count}", flush=True)
                executing.canceled(CountToResult(final_count=count))
                break
        else:
            print(f"SERVER:SUCCEEDED:{count}", flush=True)
            executing.succeed(CountToResult(final_count=count))


# --8<-- [end:run_server]


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------


# --8<-- [start:run_client]
def run_client(ctx, action: str, target: int, cancel_after: float | None):
    node = ctx.create_node("count_to_client").build()
    client = node.create_action_client(
        action, CountToGoal, CountToResult, CountToFeedback
    )

    # Give server time to advertise
    time.sleep(1.0)

    print(f"CLIENT:SEND_GOAL:{target}", flush=True)
    handle = client.send_goal(CountToGoal(target=target))

    # Schedule cancellation if requested
    if cancel_after is not None:

        def _cancel():
            time.sleep(cancel_after)
            print("CLIENT:CANCEL", flush=True)
            handle.cancel()

        threading.Thread(target=_cancel, daemon=True).start()

    # Drain feedback
    while True:
        fb = handle.recv_feedback(timeout=0.5)
        if fb is None:
            break
        print(f"CLIENT:FEEDBACK:{fb.current}", flush=True)

    result = handle.get_result(timeout=5.0)
    if result is not None:
        print(f"CLIENT:RESULT:{result.final_count}", flush=True)
    else:
        print("CLIENT:ERROR:no result", flush=True)
        sys.exit(1)


# --8<-- [end:run_client]


def main():
    parser = argparse.ArgumentParser(description="CountTo action demo")
    parser.add_argument(
        "-r",
        "--role",
        required=True,
        choices=["server", "client"],
        help="Role: server or client",
    )
    parser.add_argument(
        "-a",
        "--action",
        default="/count_to",
        help="Action name (default: /count_to)",
    )
    parser.add_argument(
        "-e",
        "--endpoint",
        default=None,
        help="Zenoh endpoint (e.g. tcp/127.0.0.1:7447)",
    )
    parser.add_argument(
        "-t",
        "--target",
        type=int,
        default=5,
        help="Target count (client only, default: 5)",
    )
    parser.add_argument(
        "--cancel",
        type=float,
        default=None,
        metavar="SECONDS",
        help="Cancel goal after N seconds (client only)",
    )

    args = parser.parse_args()

    builder = hiroz_py.ZContextBuilder()
    if args.endpoint:
        builder = builder.with_connect_endpoints([args.endpoint])
        builder = builder.disable_multicast_scouting()
    ctx = builder.build()

    if args.role == "server":
        run_server(ctx, args.action)
    else:
        run_client(ctx, args.action, args.target, args.cancel)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nStopped.", file=sys.stderr)
