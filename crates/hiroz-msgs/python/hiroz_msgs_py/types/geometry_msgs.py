"""Auto-generated ROS 2 message types for geometry_msgs."""
import msgspec
from typing import ClassVar

class InertiaStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    inertia: "geometry_msgs.Inertia | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/InertiaStamped'
    __hash__: ClassVar[str] = 'RIHS01_766be45976252babf7f9d8ac4ae7c912a7ceccf71035622529f27518b695aa09'

class PoseStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    pose: "geometry_msgs.Pose | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PoseStamped'
    __hash__: ClassVar[str] = 'RIHS01_10f3786d7d40fd2b54367835614bff85d4ad3b5dab62bf8bca0cc232d73b4cd8'

class PoseWithCovarianceStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    pose: "geometry_msgs.PoseWithCovariance | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PoseWithCovarianceStamped'
    __hash__: ClassVar[str] = 'RIHS01_26432f9803e43727d3c8f668d1fdb3c630f548af631e2f4e31382371bfea3b6e'

class VelocityStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    body_frame_id: str = ""
    reference_frame_id: str = ""
    velocity: "geometry_msgs.Twist | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/VelocityStamped'
    __hash__: ClassVar[str] = 'RIHS01_55e7196186c8dbe4375278d7f1ac050dd8c9bacade1cf3eef8460fa667bd2457'

class AccelWithCovarianceStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    accel: "geometry_msgs.AccelWithCovariance | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/AccelWithCovarianceStamped'
    __hash__: ClassVar[str] = 'RIHS01_61c9ad8928e71dd95ce791b2f02809ee2a0bbcc42cd0e4047fd00a822a08e444'

class PointStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    point: "geometry_msgs.Point | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PointStamped'
    __hash__: ClassVar[str] = 'RIHS01_4c0296af86e01e562e9e0405d138a01537247580076c58ea38d7923ac1045897'

class AccelStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    accel: "geometry_msgs.Accel | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/AccelStamped'
    __hash__: ClassVar[str] = 'RIHS01_ef1df9eabae0a708cc049a061ebcddc4e2a5f745730100ba680e086a9698b165'

class PolygonStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    polygon: "geometry_msgs.Polygon | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PolygonStamped'
    __hash__: ClassVar[str] = 'RIHS01_b7cf07932f1523d4b4088075945c1a0141f7cd21da87cc940fc61652e9138b46'

class WrenchStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    wrench: "geometry_msgs.Wrench | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/WrenchStamped'
    __hash__: ClassVar[str] = 'RIHS01_8dc3deaf06b2ab281f9f9a742a8961c328ca7cec16e3fd6586d3a5c83fa78f77'

class QuaternionStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    quaternion: "geometry_msgs.Quaternion | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/QuaternionStamped'
    __hash__: ClassVar[str] = 'RIHS01_381add86c6c3160644d228ca342182c7fd6c7fab11c7a85ad817a9cc22dbac6e'

class Wrench(msgspec.Struct, frozen=True, kw_only=True):
    force: "geometry_msgs.Vector3 | None" = None
    torque: "geometry_msgs.Vector3 | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Wrench'
    __hash__: ClassVar[str] = 'RIHS01_018e8519d57c16adbe97c9fe1460ef21fec7e31bc541de3d653a35895677ce52'

class PolygonInstanceStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    polygon: "geometry_msgs.PolygonInstance | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PolygonInstanceStamped'
    __hash__: ClassVar[str] = 'RIHS01_802f37ea4398d7ce547936aab1fd278923716a13c63373887cd896957434ce2f'

class Inertia(msgspec.Struct, frozen=True, kw_only=True):
    m: float = 0.0
    com: "geometry_msgs.Vector3 | None" = None
    ixx: float = 0.0
    ixy: float = 0.0
    ixz: float = 0.0
    iyy: float = 0.0
    iyz: float = 0.0
    izz: float = 0.0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Inertia'
    __hash__: ClassVar[str] = 'RIHS01_2ddd5dab5c347825ba2e56c895ddccfd0b8efe53ae931bf67f905529930b4bd7'

class Point(msgspec.Struct, frozen=True, kw_only=True):
    x: float = 0.0
    y: float = 0.0
    z: float = 0.0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Point'
    __hash__: ClassVar[str] = 'RIHS01_6963084842a9b04494d6b2941d11444708d892da2f4b09843b9c43f42a7f6881'

class Quaternion(msgspec.Struct, frozen=True, kw_only=True):
    x: float = 0.0
    y: float = 0.0
    z: float = 0.0
    w: float = 0.0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Quaternion'
    __hash__: ClassVar[str] = 'RIHS01_8a765f66778c8ff7c8ab94afcc590a2ed5325a1d9a076ffff38fbce36f458684'

class Pose(msgspec.Struct, frozen=True, kw_only=True):
    position: "geometry_msgs.Point | None" = None
    orientation: "geometry_msgs.Quaternion | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Pose'
    __hash__: ClassVar[str] = 'RIHS01_d501954e9476cea2996984e812054b68026ae0bfae789d9a10b23daf35cc90fa'

class PolygonInstance(msgspec.Struct, frozen=True, kw_only=True):
    polygon: "geometry_msgs.Polygon | None" = None
    id: int = 0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PolygonInstance'
    __hash__: ClassVar[str] = 'RIHS01_fa1cb3dc774329865258afef74f65b0553d487510c6d0f93ba38cc32d62ac0e5'

class PoseArray(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    poses: list["geometry_msgs.Pose"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PoseArray'
    __hash__: ClassVar[str] = 'RIHS01_af0cc36d190e104d546d168d6b39df04fa4b4ccecf59cb4c9ed328d3d5004aa0'

class TwistStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    twist: "geometry_msgs.Twist | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/TwistStamped'
    __hash__: ClassVar[str] = 'RIHS01_5f0fcd4f81d5d06ad9b4c4c63e3ea51b82d6ae4d0558f1d475229b1121db6f64'

class Vector3(msgspec.Struct, frozen=True, kw_only=True):
    x: float = 0.0
    y: float = 0.0
    z: float = 0.0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Vector3'
    __hash__: ClassVar[str] = 'RIHS01_cc12fe83e4c02719f1ce8070bfd14aecd40f75a96696a67a2a1f37f7dbb0765d'

class Twist(msgspec.Struct, frozen=True, kw_only=True):
    linear: "geometry_msgs.Vector3 | None" = None
    angular: "geometry_msgs.Vector3 | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Twist'
    __hash__: ClassVar[str] = 'RIHS01_9c45bf16fe0983d80e3cfe750d6835843d265a9a6c46bd2e609fcddde6fb8d2a'

class Point32(msgspec.Struct, frozen=True, kw_only=True):
    x: float = 0.0
    y: float = 0.0
    z: float = 0.0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Point32'
    __hash__: ClassVar[str] = 'RIHS01_2fc4db7cae16a4582c79a56b66173a8d48d52c7dc520ddc55a0d4bcf2a4bfdbc'

class Transform(msgspec.Struct, frozen=True, kw_only=True):
    translation: "geometry_msgs.Vector3 | None" = None
    rotation: "geometry_msgs.Quaternion | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Transform'
    __hash__: ClassVar[str] = 'RIHS01_beb83fbe698636351461f6f35d1abb20010c43d55374d81bd041f1ba2581fddc'

class Vector3Stamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    vector: "geometry_msgs.Vector3 | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Vector3Stamped'
    __hash__: ClassVar[str] = 'RIHS01_d4829622288cbb443886e7ea94ea5671a3b1be6bab4ad04224432a65f7d7887a'

class Polygon(msgspec.Struct, frozen=True, kw_only=True):
    points: list["geometry_msgs.Point32"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Polygon'
    __hash__: ClassVar[str] = 'RIHS01_3782f9f0bf044964d692d6c017d705e37611afb1f0bf6a9dee248a7dda0f784a'

class Pose2D(msgspec.Struct, frozen=True, kw_only=True):
    x: float = 0.0
    y: float = 0.0
    theta: float = 0.0

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Pose2D'
    __hash__: ClassVar[str] = 'RIHS01_d68efa5b46e70f7b16ca23085474fdac5a44b638783ec42f661da64da4724ccc'

class TransformStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    child_frame_id: str = ""
    transform: "geometry_msgs.Transform | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/TransformStamped'
    __hash__: ClassVar[str] = 'RIHS01_0a241f87d04668d94099cbb5ba11691d5ad32c2f29682e4eb5653424bd275206'

class AccelWithCovariance(msgspec.Struct, frozen=True, kw_only=True):
    accel: "geometry_msgs.Accel | None" = None
    covariance: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/AccelWithCovariance'
    __hash__: ClassVar[str] = 'RIHS01_230d51bd53bc36f260574e73b42941cefe44684753480b6fc330c032c5db5997'

class TwistWithCovariance(msgspec.Struct, frozen=True, kw_only=True):
    twist: "geometry_msgs.Twist | None" = None
    covariance: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/TwistWithCovariance'
    __hash__: ClassVar[str] = 'RIHS01_49f574f033f095d8b6cd1beaca5ca7925e296e84af1716d16c89d38b059c8c18'

class Accel(msgspec.Struct, frozen=True, kw_only=True):
    linear: "geometry_msgs.Vector3 | None" = None
    angular: "geometry_msgs.Vector3 | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Accel'
    __hash__: ClassVar[str] = 'RIHS01_dc448243ded9b1fcbcca24aba0c22f013dae06c354ba2d849571c0a2a3f57ca0'

class TwistWithCovarianceStamped(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    twist: "geometry_msgs.TwistWithCovariance | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/TwistWithCovarianceStamped'
    __hash__: ClassVar[str] = 'RIHS01_77b67434531e6529b7a0091357b186b6ebdb17fd9ffd3e0c7ce9d3fb11a44563'

class PoseWithCovariance(msgspec.Struct, frozen=True, kw_only=True):
    pose: "geometry_msgs.Pose | None" = None
    covariance: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/PoseWithCovariance'
    __hash__: ClassVar[str] = 'RIHS01_9a7c0fd234b7f45c6098745ecccd773ca1085670e64107135397aee31c02e1bb'

