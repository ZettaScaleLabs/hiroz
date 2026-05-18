"""Auto-generated ROS 2 message types for unique_identifier_msgs."""
import msgspec
from typing import ClassVar

class UUID(msgspec.Struct, frozen=True, kw_only=True):
    uuid: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'unique_identifier_msgs/msg/UUID'
    __hash__: ClassVar[str] = 'RIHS01_1b8e8aca958cbea28fe6ef60bf6c19b683c97a9ef60bb34752067d0f2f7ab437'

