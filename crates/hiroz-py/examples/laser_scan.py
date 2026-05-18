#!/usr/bin/env python3
"""LaserScan publisher/subscriber example for hiroz Python bindings.

This example demonstrates:
1. Publishing sensor_msgs/LaserScan messages
2. Subscribing to sensor_msgs/LaserScan messages
3. Simulating laser scanner data

Usage:
    python laser_scan.py --mode pub    # Run as publisher
    python laser_scan.py --mode sub    # Run as subscriber
"""

import hiroz_py
from hiroz_py import std_msgs, sensor_msgs, builtin_interfaces
import time
import math
import argparse


def run_publisher():
    print("Starting publisher...")
    # Create session and node
    session = hiroz_py.open_session(domain_id=0)
    print("Session created")
    node = hiroz_py.create_node(session, "laser_scan_publisher", "/")
    print("Node created")

    # Create publisher
    pub = node.create_publisher("/scan", sensor_msgs.LaserScan)

    print("Publishing LaserScan messages on /scan...")

    seq = 0
    while True:
        # Simulate a 270-degree laser scanner with 540 points
        angle_min = math.radians(-135.0)
        angle_max = math.radians(135.0)
        num_readings = 540
        angle_increment = (angle_max - angle_min) / (num_readings - 1)

        ranges = []
        intensities = []

        # Generate simulated laser scan data
        for i in range(num_readings):
            angle = angle_min + i * angle_increment

            # Simulate a simple environment: closer ranges in front, farther on sides
            base_range = 3.0 + 2.0 * math.cos(angle)

            # Add some variation
            variation = 0.1 * math.sin(seq * 0.1 + i * 0.05)
            range_val = base_range + variation

            ranges.append(range_val)
            intensities.append(100.0 + 50.0 * (i / num_readings))

        msg = sensor_msgs.LaserScan(
            header=std_msgs.Header(
                stamp=builtin_interfaces.Time(
                    sec=seq // 10, nanosec=(seq % 10) * 100_000_000
                ),
                frame_id="laser",
            ),
            angle_min=angle_min,
            angle_max=angle_max,
            angle_increment=angle_increment,
            time_increment=0.0001,
            scan_time=0.1,
            range_min=0.1,
            range_max=10.0,
            ranges=ranges,
            intensities=intensities,
        )

        pub.publish(msg)
        print(
            f"Published LaserScan #{seq}: {len(msg.ranges)} ranges, angle [{msg.angle_min:.2f}, {msg.angle_max:.2f}] rad"
        )

        seq += 1
        time.sleep(0.1)


def run_subscriber():
    # Create session and node
    session = hiroz_py.open_session(domain_id=0)
    node = hiroz_py.create_node(session, "laser_scan_subscriber", "/")

    # Create subscriber
    sub = node.create_subscriber("/scan", sensor_msgs.LaserScan)

    print("Listening for LaserScan messages on /scan...")

    while True:
        msg = sub.recv(timeout=1.0)
        if msg is None:
            print("No message received (timeout)")
            continue
        print("Received LaserScan:")
        print(f"  Frame: {msg.header.frame_id}")
        print(f"  Angle range: [{msg.angle_min:.2f}, {msg.angle_max:.2f}] rad")
        print(f"  Angle increment: {msg.angle_increment:.4f} rad")
        print(f"  Range: [{msg.range_min:.2f}, {msg.range_max:.2f}] m")
        print(f"  Number of ranges: {len(msg.ranges)}")
        print(f"  Scan time: {msg.scan_time:.3f} s")

        if msg.ranges:
            valid_ranges = [
                r for r in msg.ranges if msg.range_min <= r <= msg.range_max
            ]
            if valid_ranges:
                min_range = min(valid_ranges)
                max_range = max(valid_ranges)
                print(
                    f"  Valid ranges: {len(valid_ranges)} (min: {min_range:.2f}m, max: {max_range:.2f}m)"
                )

        print("---")


def main():
    parser = argparse.ArgumentParser(
        description="LaserScan publisher/subscriber example"
    )
    parser.add_argument(
        "--mode", choices=["pub", "sub"], default="pub", help="Mode: pub or sub"
    )
    args = parser.parse_args()

    if args.mode == "pub":
        run_publisher()
    elif args.mode == "sub":
        run_subscriber()


if __name__ == "__main__":
    main()
