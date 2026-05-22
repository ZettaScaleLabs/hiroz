#!/usr/bin/env python3
"""
Pingpong latency measurement example for hiroz-py.

Usage:
    # Terminal 1: Start pong responder
    python pingpong.py --mode pong

    # Terminal 2: Start ping sender and measure latency
    python pingpong.py --mode ping --payload 64 --frequency 10 --samples 100

    # With CSV logging:
    python pingpong.py --mode ping --payload 64 --frequency 10 --samples 100 --log results.csv
"""

import argparse
import csv
import struct
import sys
import threading
import time
from dataclasses import dataclass
from typing import Optional

import hiroz_py
from hiroz_py import std_msgs


@dataclass
class DataLogger:
    """Logs RTT data to CSV file."""

    payload: int
    frequency: int
    path: str

    def write(self, data: list[int]) -> None:
        with open(self.path, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(["Frequency", "Payload", "RTT"])
            for rtt in data:
                writer.writerow([self.frequency, self.payload, rtt])


def get_percentile(data: list[int], percentile: float) -> int:
    """Get the value at a given percentile from sorted data."""
    if not data:
        return 0
    idx = min(int(round(percentile * len(data))), len(data) - 1)
    return data[idx]


def print_statistics(rtts: list[int]) -> None:
    """Print RTT statistics."""
    rtts_sorted = sorted(rtts)
    print("\nRTT stats (nanoseconds):")
    print(f"Min : {rtts_sorted[0]}")
    print(f"p05 : {get_percentile(rtts_sorted, 0.05)}")
    print(f"p25 : {get_percentile(rtts_sorted, 0.25)}")
    print(f"p50 : {get_percentile(rtts_sorted, 0.50)}")
    print(f"p75 : {get_percentile(rtts_sorted, 0.75)}")
    print(f"p95 : {get_percentile(rtts_sorted, 0.95)}")
    print(f"Max : {rtts_sorted[-1]}")

    # Also print in microseconds for readability
    print("\nRTT stats (microseconds):")
    print(f"Min : {rtts_sorted[0] / 1000:.2f}")
    print(f"p50 : {get_percentile(rtts_sorted, 0.50) / 1000:.2f}")
    print(f"p95 : {get_percentile(rtts_sorted, 0.95) / 1000:.2f}")
    print(f"Max : {rtts_sorted[-1] / 1000:.2f}")


def run_ping(args: argparse.Namespace) -> None:
    """Run the ping sender."""
    ctx = hiroz_py.ZContextBuilder().with_logging_enabled().build()
    node = ctx.create_node("ping_node").build()

    pub = node.create_publisher("ping", std_msgs.ByteMultiArray)
    sub = node.create_subscriber("pong", std_msgs.ByteMultiArray)

    period = 1.0 / args.frequency
    finished = threading.Event()

    print(
        f"Freq: {args.frequency} Hz, Payload: {args.payload} bytes, Samples: {args.samples}"
    )

    logger: Optional[DataLogger] = None
    if args.log:
        logger = DataLogger(
            payload=args.payload, frequency=args.frequency, path=args.log
        )

    start_time_ns = time.time_ns()
    rtts: list[int] = []

    def receiver_thread():
        nonlocal rtts
        while len(rtts) < args.samples:
            msg = sub.try_recv()
            if msg is not None:
                # Extract timestamp from first 8 bytes
                if len(msg.data) >= 8:
                    sent_time = struct.unpack("<Q", bytes(msg.data[:8]))[0]
                    now = time.time_ns() - start_time_ns
                    rtt = now - sent_time
                    rtts.append(rtt)
            else:
                time.sleep(0.0001)  # 100us sleep to avoid busy waiting

        if logger:
            logger.write(rtts)
        print_statistics(rtts)
        finished.set()

    # Start receiver thread
    recv_thread = threading.Thread(target=receiver_thread, daemon=True)
    recv_thread.start()

    # Pre-allocate template buffer (similar to Rust version)
    template_buffer = bytearray([0xAA] * args.payload)

    # Main publish loop
    while not finished.is_set():
        now = time.time_ns() - start_time_ns

        # Clone template and update timestamp
        buffer = bytearray(template_buffer)
        buffer[0:8] = struct.pack("<Q", now)

        msg = std_msgs.ByteMultiArray(data=list(buffer))
        pub.publish(msg)
        time.sleep(period)

    recv_thread.join(timeout=1.0)


def run_pong() -> None:
    """Run the pong responder."""
    ctx = hiroz_py.ZContextBuilder().with_logging_enabled().build()
    node = ctx.create_node("pong_node").build()

    sub = node.create_subscriber("ping", std_msgs.ByteMultiArray)
    pub = node.create_publisher("pong", std_msgs.ByteMultiArray)

    print("Pong begin looping...")

    message_count = 0
    last_print_time = time.time()

    while True:
        msg = sub.try_recv()
        if msg is not None:
            message_count += 1

            # Extract timestamp for status printing
            last_timestamp = 0
            if len(msg.data) >= 8:
                last_timestamp = struct.unpack("<Q", bytes(msg.data[:8]))[0]
            last_payload_size = len(msg.data)

            # Echo the message back
            pub.publish(msg)

            current_time = time.time()
            if current_time - last_print_time >= 2.0:
                print(
                    f"Pong status: received {message_count} messages "
                    f"(last payload: {last_payload_size} bytes, last timestamp: {last_timestamp} ns)"
                )
                last_print_time = current_time
        else:
            time.sleep(0.0001)  # 100us sleep to avoid busy waiting


def main():
    parser = argparse.ArgumentParser(
        description="Pingpong latency measurement for hiroz-py",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "-m",
        "--mode",
        choices=["ping", "pong"],
        default="ping",
        help="Mode: ping (sender) or pong (responder)",
    )
    parser.add_argument(
        "-p",
        "--payload",
        type=int,
        default=64,
        help="Payload size in bytes (default: 64)",
    )
    parser.add_argument(
        "-f", "--frequency", type=int, default=10, help="Frequency in Hz (default: 10)"
    )
    parser.add_argument(
        "-s",
        "--samples",
        type=int,
        default=100,
        help="Number of samples to collect (default: 100)",
    )
    parser.add_argument(
        "-l",
        "--log",
        type=str,
        default="",
        help="Log file path for CSV output (optional)",
    )

    args = parser.parse_args()

    # Validate payload size (must be at least 8 bytes for timestamp)
    if args.payload < 8:
        print(
            "Error: Payload size must be at least 8 bytes (for timestamp)",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        if args.mode == "ping":
            run_ping(args)
        else:
            run_pong()
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()
