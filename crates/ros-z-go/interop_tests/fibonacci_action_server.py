#!/usr/bin/env python3
"""Minimal ROS2 action server for example_interfaces/action/Fibonacci.

Used by TestROS2ActionServerToGoClient interop test.
"""

import rclpy
from rclpy.action import ActionServer
from rclpy.node import Node
from example_interfaces.action import Fibonacci


class FibonacciActionServer(Node):
    def __init__(self):
        super().__init__("fibonacci_action_server")
        self._action_server = ActionServer(
            self,
            Fibonacci,
            "fibonacci",
            self.execute_callback,
        )

    def execute_callback(self, goal_handle):
        order = goal_handle.request.order
        sequence = [0, 1]
        feedback_msg = Fibonacci.Feedback()

        for i in range(2, order):
            sequence.append(sequence[i - 1] + sequence[i - 2])
            feedback_msg.sequence = sequence
            goal_handle.publish_feedback(feedback_msg)

        goal_handle.succeed()
        result = Fibonacci.Result()
        result.sequence = sequence
        return result


def main():
    rclpy.init()
    node = FibonacciActionServer()
    rclpy.spin(node)
    rclpy.shutdown()


if __name__ == "__main__":
    main()
