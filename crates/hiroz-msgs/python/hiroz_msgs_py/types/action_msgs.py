"""Auto-generated ROS 2 message types for action_msgs."""
import msgspec
from typing import ClassVar

class GoalStatusArray(msgspec.Struct, frozen=True, kw_only=True):
    status_list: list["action_msgs.GoalStatus"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'action_msgs/msg/GoalStatusArray'
    __hash__: ClassVar[str] = 'RIHS01_6c1684b00f177d37438febe6e709fc4e2b0d4248dca4854946f9ed8b30cda83e'

class GoalInfo(msgspec.Struct, frozen=True, kw_only=True):
    goal_id: "unique_identifier_msgs.UUID | None" = None
    stamp: "builtin_interfaces.Time | None" = msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})

    __msgtype__: ClassVar[str] = 'action_msgs/msg/GoalInfo'
    __hash__: ClassVar[str] = 'RIHS01_6398fe763154554353930716b225947f93b672f0fb2e49fdd01bb7a7e37933e9'

class GoalStatus(msgspec.Struct, frozen=True, kw_only=True):
    goal_info: "action_msgs.GoalInfo | None" = None
    status: int = 0

    __msgtype__: ClassVar[str] = 'action_msgs/msg/GoalStatus'
    __hash__: ClassVar[str] = 'RIHS01_32f4cfd717735d17657e1178f24431c1ce996c878c515230f6c5b3476819dbb9'

class CancelGoalRequest(msgspec.Struct, frozen=True, kw_only=True):
    goal_info: "action_msgs.GoalInfo | None" = None

    __msgtype__: ClassVar[str] = 'action_msgs/msg/CancelGoalRequest'
    __hash__: ClassVar[str] = 'RIHS01_c66d49f351ea4375bf3eef8569e74b7afc19305d9fa94c71b412262e411f2a8f'

class CancelGoalResponse(msgspec.Struct, frozen=True, kw_only=True):
    return_code: int = 0
    goals_canceling: list["action_msgs.GoalInfo"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'action_msgs/msg/CancelGoalResponse'
    __hash__: ClassVar[str] = 'RIHS01_c66d49f351ea4375bf3eef8569e74b7afc19305d9fa94c71b412262e411f2a8f'

class CancelGoal:
    """Service grouping type. Use CancelGoal.Request and CancelGoal.Response."""
    __srvtype__: ClassVar[str] = 'action_msgs/srv/CancelGoal'
    Request: ClassVar[type] = CancelGoalRequest
    Response: ClassVar[type] = CancelGoalResponse

