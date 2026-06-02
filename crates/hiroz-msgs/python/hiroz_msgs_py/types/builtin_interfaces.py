"""Auto-generated ROS 2 message types for builtin_interfaces."""
import msgspec
from typing import ClassVar

class Duration(msgspec.Struct, frozen=True, kw_only=True):
    sec: int = 0
    nanosec: int = 0

    __msgtype__: ClassVar[str] = 'builtin_interfaces/msg/Duration'
    __hash__: ClassVar[str] = 'RIHS01_e8d009f659816f758b75334ee1a9ca5b5c0b859843261f14c7f937349599d93b'

class Time(msgspec.Struct, frozen=True, kw_only=True):
    sec: int = 0
    nanosec: int = 0

    __msgtype__: ClassVar[str] = 'builtin_interfaces/msg/Time'
    __hash__: ClassVar[str] = 'RIHS01_b106235e25a4c5ed35098aa0a61a3ee9c9b18d197f398b0e4206cea9acf9c197'

