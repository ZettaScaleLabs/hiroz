"""Auto-generated ROS 2 message types for sensor_msgs."""
import msgspec
from typing import ClassVar

class PointField(msgspec.Struct, frozen=True, kw_only=True):
    name: str = ""
    offset: int = 0
    datatype: int = 0
    count: int = 0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/PointField'
    __hash__: ClassVar[str] = 'RIHS01_5c6a4750728c2bcfbbf7037225b20b02d4429634732146b742dee1726637ef01'

class LaserEcho(msgspec.Struct, frozen=True, kw_only=True):
    echoes: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/LaserEcho'
    __hash__: ClassVar[str] = 'RIHS01_0fbc05a0db7d37fe52c0f0375356db55da0046f7ef5bd27ca6b34bd0582bc952'

class JointState(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    name: list[str] = msgspec.field(default_factory=list)
    position: list[float] = msgspec.field(default_factory=list)
    velocity: list[float] = msgspec.field(default_factory=list)
    effort: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/JointState'
    __hash__: ClassVar[str] = 'RIHS01_a13ee3a330e346c9d87b5aa18d24e11690752bd33a0350f11c5882bc9179260e'

class LaserScan(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    angle_min: float = 0.0
    angle_max: float = 0.0
    angle_increment: float = 0.0
    time_increment: float = 0.0
    scan_time: float = 0.0
    range_min: float = 0.0
    range_max: float = 0.0
    ranges: list[float] = msgspec.field(default_factory=list)
    intensities: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/LaserScan'
    __hash__: ClassVar[str] = 'RIHS01_64c191398013af96509d518dac71d5164f9382553fce5c1f8cca5be7924bd828'

class PointCloud2(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    height: int = 0
    width: int = 0
    fields: list["sensor_msgs.PointField"] = msgspec.field(default_factory=list)
    is_bigendian: bool = False
    point_step: int = 0
    row_step: int = 0
    data: bytes = b""
    is_dense: bool = False

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/PointCloud2'
    __hash__: ClassVar[str] = 'RIHS01_9198cabf7da3796ae6fe19c4cb3bdd3525492988c70522628af5daa124bae2b5'

class NavSatStatus(msgspec.Struct, frozen=True, kw_only=True):
    status: int = 0
    service: int = 0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/NavSatStatus'
    __hash__: ClassVar[str] = 'RIHS01_d1ed3befa628e09571bd273b888ba1c1fd187c9a5e0006b385d7e5e9095a3204'

class ChannelFloat32(msgspec.Struct, frozen=True, kw_only=True):
    name: str = ""
    values: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/ChannelFloat32'
    __hash__: ClassVar[str] = 'RIHS01_92665437ddf39346f4ba39ee32e648390605b633cc077d40f4bd4d7b58af6cd4'

class TimeReference(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    time_ref: "builtin_interfaces.Time | None" = msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})
    source: str = ""

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/TimeReference'
    __hash__: ClassVar[str] = 'RIHS01_dd66e84cf40bbb5d5a40472e6ecf2675a031334d4c426abdb2ad41801a8efc99'

class NavSatFix(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    status: "sensor_msgs.NavSatStatus | None" = None
    latitude: float = 0.0
    longitude: float = 0.0
    altitude: float = 0.0
    position_covariance: list[float] = msgspec.field(default_factory=list)
    position_covariance_type: int = 0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/NavSatFix'
    __hash__: ClassVar[str] = 'RIHS01_62223ab3fe210a15976021da7afddc9e200dc9ec75231c1b6a557fc598a65404'

class Image(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    height: int = 0
    width: int = 0
    encoding: str = ""
    is_bigendian: int = 0
    step: int = 0
    data: bytes = b""

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/Image'
    __hash__: ClassVar[str] = 'RIHS01_d31d41a9a4c4bc8eae9be757b0beed306564f7526c88ea6a4588fb9582527d47'

class RegionOfInterest(msgspec.Struct, frozen=True, kw_only=True):
    x_offset: int = 0
    y_offset: int = 0
    height: int = 0
    width: int = 0
    do_rectify: bool = False

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/RegionOfInterest'
    __hash__: ClassVar[str] = 'RIHS01_ad16bcba5f9131dcdba6fbded19f726f5440e3c513b4fb586dd3027eeed8abb1'

class Range(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    radiation_type: int = 0
    field_of_view: float = 0.0
    min_range: float = 0.0
    max_range: float = 0.0
    range: float = 0.0
    variance: float = 0.0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/Range'
    __hash__: ClassVar[str] = 'RIHS01_b42b62562e93cbfe9d42b82fe5994dfa3d63d7d5c90a317981703f7388adff3a'

class Joy(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    axes: list[float] = msgspec.field(default_factory=list)
    buttons: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/Joy'
    __hash__: ClassVar[str] = 'RIHS01_0d356c79cad3401e35ffeb75a96a96e08be3ef896b8b83841d73e890989372c5'

class MultiDOFJointState(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    joint_names: list[str] = msgspec.field(default_factory=list)
    transforms: list["geometry_msgs.Transform"] = msgspec.field(default_factory=list)
    twist: list["geometry_msgs.Twist"] = msgspec.field(default_factory=list)
    wrench: list["geometry_msgs.Wrench"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/MultiDOFJointState'
    __hash__: ClassVar[str] = 'RIHS01_4d4ded702cfba7ff3ec783835c1a1425f75e53939a430ff355d1fee4b3bbc40b'

class FluidPressure(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    fluid_pressure: float = 0.0
    variance: float = 0.0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/FluidPressure'
    __hash__: ClassVar[str] = 'RIHS01_22dfb2b145a0bd5a31a1ac3882a1b32148b51d9b2f3bab250290d66f3595bc32'

class JoyFeedbackArray(msgspec.Struct, frozen=True, kw_only=True):
    array: list["sensor_msgs.JoyFeedback"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/JoyFeedbackArray'
    __hash__: ClassVar[str] = 'RIHS01_3287c32e1b688cae04555e465443df3cca7dae76ee4ebf85c4658d585037bcaa'

class CameraInfo(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    height: int = 0
    width: int = 0
    distortion_model: str = ""
    d: list[float] = msgspec.field(default_factory=list)
    k: list[float] = msgspec.field(default_factory=list)
    r: list[float] = msgspec.field(default_factory=list)
    p: list[float] = msgspec.field(default_factory=list)
    binning_x: int = 0
    binning_y: int = 0
    roi: "sensor_msgs.RegionOfInterest | None" = None

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/CameraInfo'
    __hash__: ClassVar[str] = 'RIHS01_b3dfd68ff46c9d56c80fd3bd4ed22c7a4ddce8c8348f2f59c299e73118e7e275'

class BatteryState(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    voltage: float = 0.0
    temperature: float = 0.0
    current: float = 0.0
    charge: float = 0.0
    capacity: float = 0.0
    design_capacity: float = 0.0
    percentage: float = 0.0
    power_supply_status: int = 0
    power_supply_health: int = 0
    power_supply_technology: int = 0
    present: bool = False
    cell_voltage: list[float] = msgspec.field(default_factory=list)
    cell_temperature: list[float] = msgspec.field(default_factory=list)
    location: str = ""
    serial_number: str = ""

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/BatteryState'
    __hash__: ClassVar[str] = 'RIHS01_4bee5dfce981c98faa6828b868307a0a73f992ed0789f374ee96c8f840e69741'

class Imu(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    orientation: "geometry_msgs.Quaternion | None" = None
    orientation_covariance: list[float] = msgspec.field(default_factory=list)
    angular_velocity: "geometry_msgs.Vector3 | None" = None
    angular_velocity_covariance: list[float] = msgspec.field(default_factory=list)
    linear_acceleration: "geometry_msgs.Vector3 | None" = None
    linear_acceleration_covariance: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/Imu'
    __hash__: ClassVar[str] = 'RIHS01_7d9a00ff131080897a5ec7e26e315954b8eae3353c3f995c55faf71574000b5b'

class PointCloud(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    points: list["geometry_msgs.Point32"] = msgspec.field(default_factory=list)
    channels: list["sensor_msgs.ChannelFloat32"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/PointCloud'
    __hash__: ClassVar[str] = 'RIHS01_614593df71d3c2b9bd4604a71b750fd218f0d65c045ea988b713719455a65b3b'

class Temperature(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    temperature: float = 0.0
    variance: float = 0.0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/Temperature'
    __hash__: ClassVar[str] = 'RIHS01_72514a14126ab9f8a9abec974c78e5610a367b59db5da355ff1fb982d5bad4b8'

class JoyFeedback(msgspec.Struct, frozen=True, kw_only=True):
    type: int = 0
    id: int = 0
    intensity: float = 0.0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/JoyFeedback'
    __hash__: ClassVar[str] = 'RIHS01_231dd362f71d6fc08272770d07120ad5fe5874ce2dbac70109b28986834290cd'

class MultiEchoLaserScan(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    angle_min: float = 0.0
    angle_max: float = 0.0
    angle_increment: float = 0.0
    time_increment: float = 0.0
    scan_time: float = 0.0
    range_min: float = 0.0
    range_max: float = 0.0
    ranges: list["sensor_msgs.LaserEcho"] = msgspec.field(default_factory=list)
    intensities: list["sensor_msgs.LaserEcho"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/MultiEchoLaserScan'
    __hash__: ClassVar[str] = 'RIHS01_ba5eac341cd5bbb2701527aa4568e8baec172b69cadb9a1945d6f149d087ee48'

class RelativeHumidity(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    relative_humidity: float = 0.0
    variance: float = 0.0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/RelativeHumidity'
    __hash__: ClassVar[str] = 'RIHS01_8687c99b4fb393cb2e545e407b5ea7fd0b5d8960bcd849a0f86c544740138839'

class Illuminance(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    illuminance: float = 0.0
    variance: float = 0.0

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/Illuminance'
    __hash__: ClassVar[str] = 'RIHS01_b954b25f452fcf81a91c9c2a7e3b3fd85c4c873d452aecb3cfd8fd1da732a22d'

class MagneticField(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    magnetic_field: "geometry_msgs.Vector3 | None" = None
    magnetic_field_covariance: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/MagneticField'
    __hash__: ClassVar[str] = 'RIHS01_e80f32f56a20486c9923008fc1a1db07bbb273cbbf6a5b3bfa00835ee00e4dff'

class CompressedImage(msgspec.Struct, frozen=True, kw_only=True):
    header: "std_msgs.Header | None" = None
    format: str = ""
    data: bytes = b""

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/CompressedImage'
    __hash__: ClassVar[str] = 'RIHS01_15640771531571185e2efc8a100baf923961a4d15d5569652e6cb6691e8e371a'

class SetCameraInfoRequest(msgspec.Struct, frozen=True, kw_only=True):
    camera_info: "sensor_msgs.CameraInfo | None" = None

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/SetCameraInfoRequest'
    __hash__: ClassVar[str] = 'RIHS01_a10cca5d33dc637c8d49db50ab288701a3592bb9cd854f2f16a0659613b68984'

class SetCameraInfoResponse(msgspec.Struct, frozen=True, kw_only=True):
    success: bool = False
    status_message: str = ""

    __msgtype__: ClassVar[str] = 'sensor_msgs/msg/SetCameraInfoResponse'
    __hash__: ClassVar[str] = 'RIHS01_a10cca5d33dc637c8d49db50ab288701a3592bb9cd854f2f16a0659613b68984'

