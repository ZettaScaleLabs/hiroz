#!/usr/bin/env python3
"""Focused pingpong benchmark within hiroz-py.

This benchmark isolates performance within hiroz-py without external comparison.
Run with: python benches/pingpong.py

For quick smoke test:
    python benches/pingpong.py --quick
"""

import argparse
import struct
import threading
import time

from hiroz_py import std_msgs

# Import router helper (same directory)
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from router import TestRouter, create_context_with_router


def percentile(data: list[int], p: float) -> int:
    """Get percentile value from sorted list."""
    if not data:
        return 0
    idx = min(int(p * len(data)), len(data) - 1)
    return data[idx]


def print_stats(name: str, rtts: list[int], payload_size: int) -> None:
    """Print RTT statistics."""
    if not rtts:
        print(f"\n=== {name}: No data ===")
        return
    rtts_sorted = sorted(rtts)
    print(f"\n=== {name} ({payload_size} bytes, {len(rtts)} samples) ===")
    print(f"Min : {rtts_sorted[0] / 1e6:8.2f} ms")
    print(f"p50 : {percentile(rtts_sorted, 0.50) / 1e6:8.2f} ms")
    print(f"p95 : {percentile(rtts_sorted, 0.95) / 1e6:8.2f} ms")
    print(f"Max : {rtts_sorted[-1] / 1e6:8.2f} ms")
    print(f"Throughput: {len(rtts) / (sum(rtts) / 1e9):.1f} msg/s")


def benchmark_msgspec_echo(
    node,
    payload_size: int,
    iterations: int,
    warmup: int = 10,
    timeout_per_msg: float = 0.5,
    max_consecutive_failures: int = 5,
) -> list[int]:
    """Benchmark using msgspec serialization/deserialization (normal path).

    Args:
        timeout_per_msg: Timeout for each message receive (seconds)
        max_consecutive_failures: Abort if this many consecutive messages fail
    """
    pub_ping = node.create_publisher("/bench_ping_msg", std_msgs.ByteMultiArray)
    sub_ping = node.create_subscriber("/bench_ping_msg", std_msgs.ByteMultiArray)
    pub_pong = node.create_publisher("/bench_pong_msg", std_msgs.ByteMultiArray)
    sub_pong = node.create_subscriber("/bench_pong_msg", std_msgs.ByteMultiArray)

    time.sleep(0.3)

    rtts = []
    done = threading.Event()
    consecutive_failures = 0

    def pong_loop():
        while not done.is_set():
            msg = sub_ping.try_recv()  # Non-blocking poll
            if msg:
                pub_pong.publish(msg)
            else:
                time.sleep(0.0001)  # 100us sleep to avoid busy-wait

    pong_thread = threading.Thread(target=pong_loop, daemon=True)
    pong_thread.start()

    template = bytes([0xAA] * payload_size)

    # Warmup
    print(f"  Warmup ({warmup} msgs)...", end="", flush=True)
    for _ in range(warmup):
        msg = std_msgs.ByteMultiArray(
            layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
            data=template,
        )
        pub_ping.publish(msg)
        sub_pong.recv(timeout=timeout_per_msg)
    print(" done")

    # Benchmark
    print(f"  Running {iterations} iterations...", flush=True)
    for i in range(iterations):
        start = time.monotonic_ns()
        data = struct.pack("<Q", start) + template[8:]
        msg = std_msgs.ByteMultiArray(
            layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
            data=data,
        )
        pub_ping.publish(msg)
        pong = sub_pong.recv(timeout=timeout_per_msg)
        end = time.monotonic_ns()
        if pong:
            rtts.append(end - start)
            consecutive_failures = 0
        else:
            consecutive_failures += 1
            print(f"  [!] Timeout at iteration {i + 1}/{iterations}", flush=True)
            if consecutive_failures >= max_consecutive_failures:
                print(
                    f"  [!] Aborting: {max_consecutive_failures} consecutive failures",
                    flush=True,
                )
                break

    done.set()
    pong_thread.join(timeout=1.0)
    print(f"  Completed: {len(rtts)}/{iterations} successful")
    return rtts


def benchmark_raw_echo(
    node,
    payload_size: int,
    iterations: int,
    warmup: int = 10,
    timeout_per_msg: float = 0.5,
    max_consecutive_failures: int = 5,
) -> list[int]:
    """Benchmark using raw bytes (no serialization on pong side)."""
    pub_ping = node.create_publisher("/bench_ping_raw", std_msgs.ByteMultiArray)
    sub_ping = node.create_subscriber("/bench_ping_raw", std_msgs.ByteMultiArray)
    pub_pong = node.create_publisher("/bench_pong_raw", std_msgs.ByteMultiArray)
    sub_pong = node.create_subscriber("/bench_pong_raw", std_msgs.ByteMultiArray)

    time.sleep(0.3)

    rtts = []
    done = threading.Event()
    consecutive_failures = 0

    def pong_loop():
        while not done.is_set():
            raw = sub_ping.try_recv_serialized()  # Non-blocking poll
            if raw:
                pub_pong.publish_raw(raw)
            else:
                time.sleep(0.0001)  # 100us sleep to avoid busy-wait

    pong_thread = threading.Thread(target=pong_loop, daemon=True)
    pong_thread.start()

    template = bytes([0xAA] * payload_size)

    # Warmup
    print(f"  Warmup ({warmup} msgs)...", end="", flush=True)
    for _ in range(warmup):
        msg = std_msgs.ByteMultiArray(
            layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
            data=template,
        )
        pub_ping.publish(msg)
        sub_pong.recv(timeout=timeout_per_msg)
    print(" done")

    # Benchmark
    print(f"  Running {iterations} iterations...", flush=True)
    for i in range(iterations):
        start = time.monotonic_ns()
        data = struct.pack("<Q", start) + template[8:]
        msg = std_msgs.ByteMultiArray(
            layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
            data=data,
        )
        pub_ping.publish(msg)
        pong = sub_pong.recv(timeout=timeout_per_msg)
        end = time.monotonic_ns()
        if pong:
            rtts.append(end - start)
            consecutive_failures = 0
        else:
            consecutive_failures += 1
            print(f"  [!] Timeout at iteration {i + 1}/{iterations}", flush=True)
            if consecutive_failures >= max_consecutive_failures:
                print(
                    f"  [!] Aborting: {max_consecutive_failures} consecutive failures",
                    flush=True,
                )
                break

    done.set()
    pong_thread.join(timeout=1.0)
    print(f"  Completed: {len(rtts)}/{iterations} successful")
    return rtts


def benchmark_callback_echo(
    node,
    payload_size: int,
    iterations: int,
    warmup: int = 10,
    timeout_per_msg: float = 0.5,
    max_consecutive_failures: int = 5,
) -> list[int]:
    """Benchmark using callback subscriber (no polling on pong side)."""
    pub_ping = node.create_publisher("/bench_ping_cb", std_msgs.ByteMultiArray)
    pub_pong = node.create_publisher("/bench_pong_cb", std_msgs.ByteMultiArray)
    sub_pong = node.create_subscriber("/bench_pong_cb", std_msgs.ByteMultiArray)

    def on_ping(raw_bytes: bytes):
        pub_pong.publish_raw(raw_bytes)

    sub_ping = node.create_subscriber_with_raw_callback(
        "/bench_ping_cb", std_msgs.ByteMultiArray, on_ping
    )

    time.sleep(0.3)

    rtts = []
    consecutive_failures = 0
    template = bytes([0xAA] * payload_size)

    # Warmup
    print(f"  Warmup ({warmup} msgs)...", end="", flush=True)
    for _ in range(warmup):
        msg = std_msgs.ByteMultiArray(
            layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
            data=template,
        )
        pub_ping.publish(msg)
        sub_pong.recv(timeout=timeout_per_msg)
    print(" done")

    # Benchmark
    print(f"  Running {iterations} iterations...", flush=True)
    for i in range(iterations):
        start = time.monotonic_ns()
        data = struct.pack("<Q", start) + template[8:]
        msg = std_msgs.ByteMultiArray(
            layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
            data=data,
        )
        pub_ping.publish(msg)
        pong = sub_pong.recv(timeout=timeout_per_msg)
        end = time.monotonic_ns()
        if pong:
            rtts.append(end - start)
            consecutive_failures = 0
        else:
            consecutive_failures += 1
            print(f"  [!] Timeout at iteration {i + 1}/{iterations}", flush=True)
            if consecutive_failures >= max_consecutive_failures:
                print(
                    f"  [!] Aborting: {max_consecutive_failures} consecutive failures",
                    flush=True,
                )
                break

    # Keep sub_ping alive
    _ = sub_ping
    print(f"  Completed: {len(rtts)}/{iterations} successful")
    return rtts


def benchmark_serialize_only(payload_size: int, iterations: int) -> None:
    """Benchmark serialization time only."""
    from hiroz_py.hiroz_msgs import REGISTRY

    serialize = REGISTRY["std_msgs.ByteMultiArray"]["serialize"]

    template = bytes([0xAA] * payload_size)
    msg = std_msgs.ByteMultiArray(
        layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
        data=template,
    )

    # Warmup
    for _ in range(10):
        serialize(msg)

    # Benchmark
    times = []
    for _ in range(iterations):
        start = time.monotonic_ns()
        serialize(msg)
        end = time.monotonic_ns()
        times.append(end - start)

    times.sort()
    print(f"\n=== Serialize Only ({payload_size} bytes, {iterations} samples) ===")
    print(f"Min : {times[0] / 1e6:8.4f} ms")
    print(f"p50 : {percentile(times, 0.50) / 1e6:8.4f} ms")
    print(f"p95 : {percentile(times, 0.95) / 1e6:8.4f} ms")
    print(f"Max : {times[-1] / 1e6:8.4f} ms")


def benchmark_deserialize_only(payload_size: int, iterations: int) -> None:
    """Benchmark deserialization time only."""
    from hiroz_py.hiroz_msgs import REGISTRY

    serialize = REGISTRY["std_msgs.ByteMultiArray"]["serialize"]
    deserialize = REGISTRY["std_msgs.ByteMultiArray"]["deserialize"]

    template = bytes([0xAA] * payload_size)
    msg = std_msgs.ByteMultiArray(
        layout=std_msgs.MultiArrayLayout(dim=[], data_offset=0),
        data=template,
    )
    cdr_bytes = serialize(msg)

    # Warmup
    for _ in range(10):
        deserialize(cdr_bytes)

    # Benchmark
    times = []
    for _ in range(iterations):
        start = time.monotonic_ns()
        deserialize(cdr_bytes)
        end = time.monotonic_ns()
        times.append(end - start)

    times.sort()
    print(f"\n=== Deserialize Only ({payload_size} bytes, {iterations} samples) ===")
    print(f"Min : {times[0] / 1e6:8.4f} ms")
    print(f"p50 : {percentile(times, 0.50) / 1e6:8.4f} ms")
    print(f"p95 : {percentile(times, 0.95) / 1e6:8.4f} ms")
    print(f"Max : {times[-1] / 1e6:8.4f} ms")


def main():
    parser = argparse.ArgumentParser(description="hiroz-py focused pingpong benchmark")
    parser.add_argument(
        "-p", "--payload", type=int, default=1024 * 1024, help="Payload size in bytes"
    )
    parser.add_argument(
        "-n", "--iterations", type=int, default=50, help="Number of iterations"
    )
    parser.add_argument(
        "-w", "--warmup", type=int, default=10, help="Warmup iterations"
    )
    parser.add_argument(
        "--only",
        choices=["msgspec", "raw", "callback", "serialize", "deserialize", "all"],
        default="all",
        help="Run only specific benchmark",
    )
    parser.add_argument(
        "--quick",
        action="store_true",
        help="Quick smoke test mode (1KB payload, 5 iterations, 2 warmup)",
    )
    args = parser.parse_args()

    # Quick mode overrides
    if args.quick:
        args.payload = 1024
        args.iterations = 5
        args.warmup = 2
        print("*** QUICK SMOKE TEST MODE ***")

    print("hiroz-py Focused Benchmark")
    print(f"Payload: {args.payload} bytes, Iterations: {args.iterations}")
    print("=" * 60)

    # Serialization benchmarks (no zenoh needed)
    if args.only in ("serialize", "all"):
        benchmark_serialize_only(args.payload, args.iterations)

    if args.only in ("deserialize", "all"):
        benchmark_deserialize_only(args.payload, args.iterations)

    # Pubsub benchmarks (need zenoh with router)
    if args.only in ("msgspec", "raw", "callback", "all"):
        print("Starting Zenoh router...")
        router = TestRouter.start()
        ctx = create_context_with_router(router)
        node = ctx.create_node("bench_node").build()

        if args.only in ("msgspec", "all"):
            rtts = benchmark_msgspec_echo(
                node, args.payload, args.iterations, args.warmup
            )
            print_stats("Msgspec Echo (serialize both sides)", rtts, args.payload)

        if args.only in ("raw", "all"):
            rtts = benchmark_raw_echo(node, args.payload, args.iterations, args.warmup)
            print_stats("Raw Echo (pong uses raw bytes)", rtts, args.payload)

        if args.only in ("callback", "all"):
            rtts = benchmark_callback_echo(
                node, args.payload, args.iterations, args.warmup
            )
            print_stats("Callback Echo (pong uses callback)", rtts, args.payload)

        # Clean up router
        router.close()

    print("\n" + "=" * 60)
    print("Done!")


if __name__ == "__main__":
    main()
