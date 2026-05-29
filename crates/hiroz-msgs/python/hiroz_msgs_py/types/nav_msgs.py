"""Auto-generated ROS 2 message types for nav_msgs."""
import msgspec
from typing import ClassVar

class Path(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    poses: list["geometry_msgs.PoseStamped"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/Path'
    __hash__: ClassVar[str] = 'RIHS01_1957a5bb3cee5da65c4e52e52b65a93df227efce4c20f8458b36e73066ca334b'

class GridCells(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    cell_width: float = 0.0
    cell_height: float = 0.0
    cells: list["geometry_msgs.Point"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/GridCells'
    __hash__: ClassVar[str] = 'RIHS01_bb99c2f5d0a04750745a81ec6a8147aa373cce5bd17c8cd6507f2413354a6933'

class OccupancyGrid(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    info: "nav_msgs.MapMetaData | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/OccupancyGrid'
    __hash__: ClassVar[str] = 'RIHS01_8d348150c12913a31ee0ec170fbf25089e4745d17035792a1ba94d6f0bc0cfc7'

class Odometry(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    child_frame_id: str = ""
    pose: "geometry_msgs.PoseWithCovariance | None" = None
    twist: "geometry_msgs.TwistWithCovariance | None" = None

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/Odometry'
    __hash__: ClassVar[str] = 'RIHS01_3cc97dc7fb7502f8714462c526d369e35b603cfc34d946e3f2eda2766dfec6e0'

class MapMetaData(msgspec.Struct, frozen=True, kw_only=True):
    map_load_time: "builtin_interfaces.Time | None" = msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})
    resolution: float = 0.0
    width: int = 0
    height: int = 0
    origin: "geometry_msgs.Pose | None" = None

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/MapMetaData'
    __hash__: ClassVar[str] = 'RIHS01_2772d4b2000ef2b35dbaeb80fd3946c1369f817fb4f75677d916d27c17d763c8'

class Goals(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    goals: list["geometry_msgs.PoseStamped"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/Goals'
    __hash__: ClassVar[str] = 'RIHS01_02305a51633b5c04d8979b878a7577cafd422f8a07465c878b17a920af3759e9'

class GetMapRequest(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/GetMapRequest'
    __hash__: ClassVar[str] = 'RIHS01_c8ae77c9995b3554b5ba80e4d4d443f970ac65143102a1d893ec24fc07b31147'

class GetMapResponse(msgspec.Struct, frozen=True, kw_only=True):
    map: "nav_msgs.OccupancyGrid | None" = None

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/GetMapResponse'
    __hash__: ClassVar[str] = 'RIHS01_c8ae77c9995b3554b5ba80e4d4d443f970ac65143102a1d893ec24fc07b31147'

class GetPlanRequest(msgspec.Struct, frozen=True, kw_only=True):
    start: "geometry_msgs.PoseStamped | None" = None
    goal: "geometry_msgs.PoseStamped | None" = None
    tolerance: float = 0.0

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/GetPlanRequest'
    __hash__: ClassVar[str] = 'RIHS01_234f7aff100f5edb8150366601687b027bcdc253db47decb88fff846193fe5e8'

class GetPlanResponse(msgspec.Struct, frozen=True, kw_only=True):
    plan: "nav_msgs.Path | None" = None

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/GetPlanResponse'
    __hash__: ClassVar[str] = 'RIHS01_234f7aff100f5edb8150366601687b027bcdc253db47decb88fff846193fe5e8'

class LoadMapRequest(msgspec.Struct, frozen=True, kw_only=True):
    map_url: str = ""

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/LoadMapRequest'
    __hash__: ClassVar[str] = 'RIHS01_1a192ac56c40fed2767dac26f0b371785372276bd465c902676d2dca135aae5a'

class LoadMapResponse(msgspec.Struct, frozen=True, kw_only=True):
    map: "nav_msgs.OccupancyGrid | None" = None
    result: int = 0

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/LoadMapResponse'
    __hash__: ClassVar[str] = 'RIHS01_1a192ac56c40fed2767dac26f0b371785372276bd465c902676d2dca135aae5a'

class SetMapRequest(msgspec.Struct, frozen=True, kw_only=True):
    map: "nav_msgs.OccupancyGrid | None" = None
    initial_pose: "geometry_msgs.PoseWithCovarianceStamped | None" = None

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/SetMapRequest'
    __hash__: ClassVar[str] = 'RIHS01_5e11a5b2ca53d8ae85b666a019f16c9904ebc787828f1f566c4e048a1ddedfb4'

class SetMapResponse(msgspec.Struct, frozen=True, kw_only=True):
    success: bool = False

    __msgtype__: ClassVar[str] = 'nav_msgs/msg/SetMapResponse'
    __hash__: ClassVar[str] = 'RIHS01_5e11a5b2ca53d8ae85b666a019f16c9904ebc787828f1f566c4e048a1ddedfb4'

class GetMap:
    """Service grouping type. Use GetMap.Request and GetMap.Response."""
    __srvtype__: ClassVar[str] = 'nav_msgs/srv/GetMap'
    Request: ClassVar[type] = GetMapRequest
    Response: ClassVar[type] = GetMapResponse

class GetPlan:
    """Service grouping type. Use GetPlan.Request and GetPlan.Response."""
    __srvtype__: ClassVar[str] = 'nav_msgs/srv/GetPlan'
    Request: ClassVar[type] = GetPlanRequest
    Response: ClassVar[type] = GetPlanResponse

class LoadMap:
    """Service grouping type. Use LoadMap.Request and LoadMap.Response."""
    __srvtype__: ClassVar[str] = 'nav_msgs/srv/LoadMap'
    Request: ClassVar[type] = LoadMapRequest
    Response: ClassVar[type] = LoadMapResponse

class SetMap:
    """Service grouping type. Use SetMap.Request and SetMap.Response."""
    __srvtype__: ClassVar[str] = 'nav_msgs/srv/SetMap'
    Request: ClassVar[type] = SetMapRequest
    Response: ClassVar[type] = SetMapResponse

