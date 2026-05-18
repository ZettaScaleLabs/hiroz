#!/usr/bin/env python3
"""
AddTwoInts service demo - runs as server or client.

Usage:
    python service_demo.py -r server                    # Run as server
    python service_demo.py -r client -a 5 -b 7          # Run as client
    python service_demo.py -r server -e tcp/...         # Connect to specific endpoint
"""

import argparse
import sys
import time

import hiroz_py
from hiroz_py import example_interfaces


# --8<-- [start:run_server]
def run_server(ctx, service: str, max_requests: int):
    """Run the AddTwoInts service server."""
    node = ctx.create_node("add_two_ints_server").build()
    server = node.create_server(service, example_interfaces.AddTwoIntsRequest)

    print("SERVER:READY", flush=True)

    handled = 0
    while max_requests == 0 or handled < max_requests:
        request_id, req = server.take_request()
        result = req.a + req.b
        print(f"SERVER:{req.a}+{req.b}={result}", flush=True)

        resp = example_interfaces.AddTwoIntsResponse(sum=result)
        server.send_response(resp, request_id)
        handled += 1

    print("SERVER:DONE", flush=True)


# --8<-- [end:run_server]


# --8<-- [start:run_client]
def run_client(ctx, service: str, a: int, b: int, timeout: float):
    """Run the AddTwoInts service client."""
    node = ctx.create_node("add_two_ints_client").build()
    client = node.create_client(service, example_interfaces.AddTwoIntsRequest)

    # Wait for service discovery
    time.sleep(1.0)

    print(f"CLIENT:REQUEST:{a}+{b}", flush=True)

    req = example_interfaces.AddTwoIntsRequest(a=a, b=b)
    try:
        resp = client.call(req, timeout=timeout)
        print(f"CLIENT:RESPONSE:{resp.sum}", flush=True)
    except RuntimeError as e:
        print(f"CLIENT:ERROR:{e}", flush=True)
        sys.exit(1)


# --8<-- [end:run_client]


def main():
    parser = argparse.ArgumentParser(description="AddTwoInts service demo")
    parser.add_argument(
        "-r",
        "--role",
        type=str,
        required=True,
        choices=["server", "client"],
        help="Role: server or client",
    )
    parser.add_argument(
        "-s",
        "--service",
        type=str,
        default="/add_two_ints",
        help="Service name (default: /add_two_ints)",
    )
    parser.add_argument(
        "-e",
        "--endpoint",
        type=str,
        default=None,
        help="Zenoh endpoint to connect to (e.g., tcp/127.0.0.1:7447)",
    )
    parser.add_argument(
        "-a",
        type=int,
        default=2,
        help="First number (client only, default: 2)",
    )
    parser.add_argument(
        "-b",
        type=int,
        default=3,
        help="Second number (client only, default: 3)",
    )
    parser.add_argument(
        "-c",
        "--count",
        type=int,
        default=0,
        help="Max requests to handle (0 for unlimited, server only)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=5.0,
        help="Response timeout in seconds (client only, default: 5.0)",
    )

    args = parser.parse_args()

    # Build context
    builder = hiroz_py.ZContextBuilder()
    if args.endpoint:
        builder = builder.with_connect_endpoints([args.endpoint])
        builder = builder.disable_multicast_scouting()

    ctx = builder.build()

    if args.role == "server":
        run_server(ctx, args.service, args.count)
    else:
        run_client(ctx, args.service, args.a, args.b, args.timeout)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nStopped.", file=sys.stderr)
