"""Auto-generated ROS 2 message types for action_tutorials_interfaces."""
import msgspec
from typing import ClassVar

class FibonacciGoal(msgspec.Struct, frozen=True, kw_only=True):
    order: int = 0

    __msgtype__: ClassVar[str] = 'action_tutorials_interfaces/msg/FibonacciGoal'
    __hash__: ClassVar[str] = 'RIHS01_12b2d4be0186b9d26e02c9be2cbbc9438ab3ba78b66806b3f7f4111bb75199cb'

class FibonacciResult(msgspec.Struct, frozen=True, kw_only=True):
    sequence: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'action_tutorials_interfaces/msg/FibonacciResult'
    __hash__: ClassVar[str] = 'RIHS01_12b2d4be0186b9d26e02c9be2cbbc9438ab3ba78b66806b3f7f4111bb75199cb'

class FibonacciFeedback(msgspec.Struct, frozen=True, kw_only=True):
    partial_sequence: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'action_tutorials_interfaces/msg/FibonacciFeedback'
    __hash__: ClassVar[str] = 'RIHS01_12b2d4be0186b9d26e02c9be2cbbc9438ab3ba78b66806b3f7f4111bb75199cb'

class Fibonacci:
    """Action grouping type. Use Fibonacci.Goal, .Result and .Feedback."""
    __actiontype__: ClassVar[str] = 'action_tutorials_interfaces/action/Fibonacci'
    Goal: ClassVar[type] = FibonacciGoal
    Result: ClassVar[type] = FibonacciResult
    Feedback: ClassVar[type] = FibonacciFeedback

