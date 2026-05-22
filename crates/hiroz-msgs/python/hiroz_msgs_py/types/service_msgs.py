"""Auto-generated ROS 2 message types for service_msgs."""
import msgspec
from typing import ClassVar

class ServiceEventInfo(msgspec.Struct, frozen=True, kw_only=True):
    event_type: int = 0
    stamp: "builtin_interfaces.Time | None" = msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})
    client_gid: list[int] = msgspec.field(default_factory=list)
    sequence_number: int = 0

    __msgtype__: ClassVar[str] = 'service_msgs/msg/ServiceEventInfo'
    __hash__: ClassVar[str] = 'RIHS01_41bcbbe07a75c9b52bc96bfd5c24d7f0fc0a08c0cb7921b3373c5732345a6f45'

