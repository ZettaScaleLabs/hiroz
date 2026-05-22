#!/usr/bin/env python3
"""Pure Zenoh baseline benchmark to establish minimum possible latency.

This bypasses hiroz entirely to measure raw zenoh performance.
Run with: python benches/zenoh_baseline.py

For quick smoke test:
    python benches/zenoh_baseline.py --quick
"""

import argparse
import struct
import threading
import time

import zenoh

# Import router helper (same directory)
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from router import TestRouter


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


def try_recv_with_timeout(sub, timeout_sec: float):
    """Poll try_recv with timeout."""
    deadline = time.monotonic() + timeout_sec
    while time.monotonic() < deadline:
        sample = sub.try_recv()
        if sample is not None:
            return sample
        time.sleep(0.0001)  # 100us polling
    return None


def benchmark_zenoh_polling(
    session: zenoh.Session, payload_size: int, iterations: int, warmup: int = 10
) -> list[int]:
    """Benchmark pure zenoh with polling receive."""
    pub_ping = session.declare_publisher("bench/ping")
    sub_ping = session.declare_subscriber("bench/ping")
    pub_pong = session.declare_publisher("bench/pong")
    sub_pong = session.declare_subscriber("bench/pong")

    time.sleep(0.3)

    rtts = []
    done = threading.Event()

    def pong_loop():
        while not done.is_set():
            sample = sub_ping.try_recv()
            if sample:
                pub_pong.put(bytes(sample.payload))
            else:
                time.sleep(0.0001)  # 100us polling

    pong_thread = threading.Thread(target=pong_loop, daemon=True)
    pong_thread.start()

    template = bytes([0xAA] * payload_size)

    # Warmup
    for _ in range(warmup):
        pub_ping.put(template)
        try_recv_with_timeout(sub_pong, 0.5)

    # Benchmark
    for i in range(iterations):
        start = time.monotonic_ns()
        data = struct.pack("<Q", start) + template[8:]
        pub_ping.put(data)
        sample = try_recv_with_timeout(sub_pong, 1.0)
        end = time.monotonic_ns()
        if sample:
            rtts.append(end - start)

    done.set()
    pong_thread.join(timeout=1.0)
    return rtts


def benchmark_zenoh_callback(
    session: zenoh.Session, payload_size: int, iterations: int, warmup: int = 10
) -> list[int]:
    """Benchmark pure zenoh with callback receive."""
    pub_ping = session.declare_publisher("bench/ping_cb")
    pub_pong = session.declare_publisher("bench/pong_cb")
    sub_pong = session.declare_subscriber("bench/pong_cb")

    def on_ping(sample):
        pub_pong.put(bytes(sample.payload))

    sub_ping = session.declare_subscriber("bench/ping_cb", on_ping)

    time.sleep(0.3)

    rtts = []
    template = bytes([0xAA] * payload_size)

    # Warmup
    for _ in range(warmup):
        pub_ping.put(template)
        try_recv_with_timeout(sub_pong, 0.5)

    # Benchmark
    for i in range(iterations):
        start = time.monotonic_ns()
        data = struct.pack("<Q", start) + template[8:]
        pub_ping.put(data)
        sample = try_recv_with_timeout(sub_pong, 1.0)
        end = time.monotonic_ns()
        if sample:
            rtts.append(end - start)

    # Keep sub_ping alive
    _ = sub_ping
    return rtts


def main():
    parser = argparse.ArgumentParser(description="Pure Zenoh baseline benchmark")
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

    print("Pure Zenoh Baseline Benchmark")
    print(f"Payload: {args.payload} bytes, Iterations: {args.iterations}")
    print("=" * 60)

    # Start router and open client session
    print("Starting Zenoh router...")
    router = TestRouter.start()

    config = zenoh.Config()
    config.insert_json5("mode", '"client"')
    config.insert_json5("connect/endpoints", f'["{router.endpoint}"]')
    config.insert_json5("scouting/multicast/enabled", "false")
    session = zenoh.open(config)

    # Run benchmarks
    rtts = benchmark_zenoh_polling(session, args.payload, args.iterations, args.warmup)
    print_stats("Zenoh Polling", rtts, args.payload)

    rtts = benchmark_zenoh_callback(session, args.payload, args.iterations, args.warmup)
    print_stats("Zenoh Callback", rtts, args.payload)

    session.close()
    router.close()

    print("\n" + "=" * 60)
    print("Done!")


if __name__ == "__main__":
    main()
