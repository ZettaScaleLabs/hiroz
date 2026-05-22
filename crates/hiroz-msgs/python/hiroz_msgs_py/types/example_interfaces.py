"""Auto-generated ROS 2 message types for example_interfaces."""
import msgspec
from typing import ClassVar

class UInt64MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt64MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_c05a348bd887aac76a947c901f67f93bf9a7fa14b28dce02eb495dadeb8842e4'

class Float32(msgspec.Struct, frozen=True, kw_only=True):
    data: float = 0.0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Float32'
    __hash__: ClassVar[str] = 'RIHS01_6a112d9235f8e8088d7a2bc77cb955341ac0d5c9870bdc592651a4186bb246f3'

class Float64(msgspec.Struct, frozen=True, kw_only=True):
    data: float = 0.0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Float64'
    __hash__: ClassVar[str] = 'RIHS01_74c137b7930c26339425a95fcfab441199bc41e0e572d3a0c9e95badd72b50da'

class UInt8MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: bytes = b""

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt8MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_7a49ef5ed9b7182488303b4027d54719ba7266e776bd446dac30e79b56fb0a71'

class ByteMultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: bytes = b""

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/ByteMultiArray'
    __hash__: ClassVar[str] = 'RIHS01_257fc3429b3c4dd0aea678c234f543665d1144b396f82c7ee60fed151e1dace3'

class Int16(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int16'
    __hash__: ClassVar[str] = 'RIHS01_332d94306732e4e35da38e5ae744ff35bbdaeca300908dc43488d3a844687cd6'

class Float64MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Float64MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_62a0985e7920b7bc489d1392f08c06ba0080fff5b8eaa09b27dcac122f50ad95'

class Char(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Char'
    __hash__: ClassVar[str] = 'RIHS01_320dcd57e1183fb08463cc3ab50bf7e5ce0ecee39f64d15a9e9eeca3384c91a5'

class MultiArrayLayout(msgspec.Struct, frozen=True, kw_only=True):
    dim: list["example_interfaces.MultiArrayDimension"] = msgspec.field(default_factory=list)
    data_offset: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/MultiArrayLayout'
    __hash__: ClassVar[str] = 'RIHS01_ba42ea30074e5826a1e91f70f3660dda2996937169487f877fc20a8a402e2c27'

class Int64MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int64MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_ebce42c0d2b81bc5309deb0b2d83e30236aeb4899aea3cc7ddd18787af39c94a'

class Bool(msgspec.Struct, frozen=True, kw_only=True):
    data: bool = False

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Bool'
    __hash__: ClassVar[str] = 'RIHS01_4765c142500f8fd4e1a32fb3edd7b7d9d822a16ec270445f5120e772c5f9aed5'

class String(msgspec.Struct, frozen=True, kw_only=True):
    data: str = ""

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/String'
    __hash__: ClassVar[str] = 'RIHS01_5509d866a579951f2fc6c19577c32605ba16f308cae7b498341d79536d4eb06b'

class Int8MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int8MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_6d26932aefc0e5c0c473613376338609bac9925e3527dc4e8fab6b6783b6d680'

class MultiArrayDimension(msgspec.Struct, frozen=True, kw_only=True):
    label: str = ""
    size: int = 0
    stride: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/MultiArrayDimension'
    __hash__: ClassVar[str] = 'RIHS01_a785cb9839e177e3eb760260139a919fec87821edc3314c592f2725abbf0bfcd'

class Byte(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Byte'
    __hash__: ClassVar[str] = 'RIHS01_f014e0424be54b8ba7c35490aea4198be92df1de4e88f4e19a2fbbce2e020bb9'

class Int32MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int32MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_b347aff8e38e2ce3f7f4023db7922d2d4abb9d593b03c40b1427640c26595bfa'

class UInt16MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt16MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_507f6456c1965ef6eab8a9cfa8860bd484e1b7039b4c4ea2c75c648182fceade'

class UInt32MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt32MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_0683176c523a116c76141c6d4abd150dbff492b72185f1260ce38a18a35aafcf'

class Int32(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int32'
    __hash__: ClassVar[str] = 'RIHS01_5cd04cd7f3adb9d6c6064c316047b24c76622eb89144f300b536d657fd55e652'

class UInt32(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt32'
    __hash__: ClassVar[str] = 'RIHS01_e86cccba586f30c16498f3ccd3550a764b255239a6142be3a4d7a2fa9a43515c'

class Float32MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[float] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Float32MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_8705278e8d206a9711133794ce6466281e9698138331c4b3d91e6cc9e9dfee26'

class UInt8(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt8'
    __hash__: ClassVar[str] = 'RIHS01_9255b6d0dd98f5b573afbff223131279b788ac45cf051fb462c12dd9a30f4061'

class UInt16(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt16'
    __hash__: ClassVar[str] = 'RIHS01_e123d0a691fa0ce58b682f1a4eee55137dd3f20c81665f0c556f53596c7fb377'

class Empty(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Empty'
    __hash__: ClassVar[str] = 'RIHS01_73c22a7341eeccf8ef504a991e60d4078223f0931a5d5d212800e7c978903c58'

class Int8(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int8'
    __hash__: ClassVar[str] = 'RIHS01_2e9ef643d84ff37840fe787d1269aa06268960e294f4a0f5eb1e9d4eb21cbb57'

class Int64(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int64'
    __hash__: ClassVar[str] = 'RIHS01_1b3b9a6502f560d079520c73c685a9550e5a1838d2cefd537fe0aba75a3639a0'

class Int16MultiArray(msgspec.Struct, frozen=True, kw_only=True):
    layout: "example_interfaces.MultiArrayLayout | None" = None
    data: list[int] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/Int16MultiArray'
    __hash__: ClassVar[str] = 'RIHS01_d663e739410479619bfb7b9b94b0a764998acffadb8b7f5b200affa87b7ffdd2'

class UInt64(msgspec.Struct, frozen=True, kw_only=True):
    data: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/UInt64'
    __hash__: ClassVar[str] = 'RIHS01_6a3f8548c5818b7add62dd6cbbd840fd1ab17fbf9d73cad6690557b7326d8908'

class AddTwoIntsRequest(msgspec.Struct, frozen=True, kw_only=True):
    a: int = 0
    b: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/AddTwoIntsRequest'
    __hash__: ClassVar[str] = 'RIHS01_e118de6bf5eeb66a2491b5bda11202e7b68f198d6f67922cf30364858239c81a'

class AddTwoIntsResponse(msgspec.Struct, frozen=True, kw_only=True):
    sum: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/AddTwoIntsResponse'
    __hash__: ClassVar[str] = 'RIHS01_e118de6bf5eeb66a2491b5bda11202e7b68f198d6f67922cf30364858239c81a'

class SetBoolRequest(msgspec.Struct, frozen=True, kw_only=True):
    data: bool = False

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/SetBoolRequest'
    __hash__: ClassVar[str] = 'RIHS01_a69782e5631b12e15c8e218410de1685bbf13e382718295adad14037a24afbe8'

class SetBoolResponse(msgspec.Struct, frozen=True, kw_only=True):
    success: bool = False
    message: str = ""

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/SetBoolResponse'
    __hash__: ClassVar[str] = 'RIHS01_a69782e5631b12e15c8e218410de1685bbf13e382718295adad14037a24afbe8'

class TriggerRequest(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/TriggerRequest'
    __hash__: ClassVar[str] = 'RIHS01_cfeeee47f8105dd7685e4c92d46d4074669cb1c477402be1dea37486542a69e0'

class TriggerResponse(msgspec.Struct, frozen=True, kw_only=True):
    success: bool = False
    message: str = ""

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/TriggerResponse'
    __hash__: ClassVar[str] = 'RIHS01_cfeeee47f8105dd7685e4c92d46d4074669cb1c477402be1dea37486542a69e0'

