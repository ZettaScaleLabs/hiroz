"""Auto-generated ROS 2 message types for lifecycle_msgs."""
import msgspec
from typing import ClassVar

class TransitionDescription(msgspec.Struct, frozen=True, kw_only=True):
    transition: "lifecycle_msgs.Transition | None" = None
    start_state: "lifecycle_msgs.State | None" = None
    goal_state: "lifecycle_msgs.State | None" = None

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/TransitionDescription'
    __hash__: ClassVar[str] = 'RIHS01_c5f1cd4bb1ad2ba0e3329d4ac7015c52a674a72c1faf7974c37a33f4f6048b28'

class Transition(msgspec.Struct, frozen=True, kw_only=True):
    id: int = 0
    label: str = ""

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/Transition'
    __hash__: ClassVar[str] = 'RIHS01_c65d7b31ea134cba4f54fc867b817979be799f7452035cd35fac9b7421fb3424'

class TransitionEvent(msgspec.Struct, frozen=True, kw_only=True):
    timestamp: "builtin_interfaces.Time | None" = msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})
    transition: "lifecycle_msgs.Transition | None" = None
    start_state: "lifecycle_msgs.State | None" = None
    goal_state: "lifecycle_msgs.State | None" = None

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/TransitionEvent'
    __hash__: ClassVar[str] = 'RIHS01_f90405bf3073265eec4847f23ac63298cf2dd3933a6d541be11a9232dd840c32'

class State(msgspec.Struct, frozen=True, kw_only=True):
    id: int = 0
    label: str = ""

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/State'
    __hash__: ClassVar[str] = 'RIHS01_dd2d02b82f3ebc858e53c431b1e6e91f3ffc71436fc81d0715214ac6ee2107a0'

class GetAvailableStatesRequest(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/GetAvailableStatesRequest'
    __hash__: ClassVar[str] = 'RIHS01_00a07d79d2207d71e81a8cbc1880e5d924cc16d4688ea8e8e06e443dc8f8aa1d'

class GetAvailableStatesResponse(msgspec.Struct, frozen=True, kw_only=True):
    available_states: list["lifecycle_msgs.State"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/GetAvailableStatesResponse'
    __hash__: ClassVar[str] = 'RIHS01_00a07d79d2207d71e81a8cbc1880e5d924cc16d4688ea8e8e06e443dc8f8aa1d'

class GetStateRequest(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/GetStateRequest'
    __hash__: ClassVar[str] = 'RIHS01_800a0a5aae599782b02932de0caf563f6dc4e7e94b794eadde075ba2cbef9795'

class GetStateResponse(msgspec.Struct, frozen=True, kw_only=True):
    current_state: "lifecycle_msgs.State | None" = None

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/GetStateResponse'
    __hash__: ClassVar[str] = 'RIHS01_800a0a5aae599782b02932de0caf563f6dc4e7e94b794eadde075ba2cbef9795'

class ChangeStateRequest(msgspec.Struct, frozen=True, kw_only=True):
    transition: "lifecycle_msgs.Transition | None" = None

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/ChangeStateRequest'
    __hash__: ClassVar[str] = 'RIHS01_356fe34f0475a43acf54542013af4167b0e729f77ea22ffb045c6ad8e20668e5'

class ChangeStateResponse(msgspec.Struct, frozen=True, kw_only=True):
    success: bool = False

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/ChangeStateResponse'
    __hash__: ClassVar[str] = 'RIHS01_356fe34f0475a43acf54542013af4167b0e729f77ea22ffb045c6ad8e20668e5'

class GetAvailableTransitionsRequest(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/GetAvailableTransitionsRequest'
    __hash__: ClassVar[str] = 'RIHS01_59b7ecefce0982a8a844b9f2c4f14764c1c4543cc55e72924e2aa4adad83e9bc'

class GetAvailableTransitionsResponse(msgspec.Struct, frozen=True, kw_only=True):
    available_transitions: list["lifecycle_msgs.TransitionDescription"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'lifecycle_msgs/msg/GetAvailableTransitionsResponse'
    __hash__: ClassVar[str] = 'RIHS01_59b7ecefce0982a8a844b9f2c4f14764c1c4543cc55e72924e2aa4adad83e9bc'

