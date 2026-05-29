"""Auto-generated ROS 2 message types for rcl_interfaces."""
import msgspec
from typing import ClassVar

class SetParametersResult(msgspec.Struct, frozen=True, kw_only=True):
    successful: bool = False
    reason: str = ""

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/SetParametersResult'
    __hash__: ClassVar[str] = 'RIHS01_cfcc0fb0371ee5159b403960ef4300f8f9d2f1fd6117c8666b7f9654d528a9b1'

class IntegerRange(msgspec.Struct, frozen=True, kw_only=True):
    from_value: int = 0
    to_value: int = 0
    step: int = 0

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/IntegerRange'
    __hash__: ClassVar[str] = 'RIHS01_f7b7fdc0f65f07702e099218e13288c3963bcb9345bde78b560e6cd19800fc5a'

class ListParametersResult(msgspec.Struct, frozen=True, kw_only=True):
    names: list[str] = msgspec.field(default_factory=list)
    prefixes: list[str] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ListParametersResult'
    __hash__: ClassVar[str] = 'RIHS01_237ae3428413dcbcfb452b510c42355f3a2b021dc091afa3e18526d57022f1cd'

class ParameterDescriptor(msgspec.Struct, frozen=True, kw_only=True):
    name: str = ""
    type: int = 0
    description: str = ""
    additional_constraints: str = ""
    read_only: bool = False
    dynamic_typing: bool = False
    floating_point_range: list["rcl_interfaces.FloatingPointRange"] = msgspec.field(default_factory=list)
    integer_range: list["rcl_interfaces.IntegerRange"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ParameterDescriptor'
    __hash__: ClassVar[str] = 'RIHS01_52175dbfda6c51153101d33d2a9da05743f66f02d5ab2ca9ec4709b46b73d704'

class ParameterValue(msgspec.Struct, frozen=True, kw_only=True):
    type: int = 0
    bool_value: bool = False
    integer_value: int = 0
    double_value: float = 0.0
    string_value: str = ""
    byte_array_value: bytes = b""
    bool_array_value: list[bool] = msgspec.field(default_factory=list)
    integer_array_value: list[int] = msgspec.field(default_factory=list)
    double_array_value: list[float] = msgspec.field(default_factory=list)
    string_array_value: list[str] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ParameterValue'
    __hash__: ClassVar[str] = 'RIHS01_115fc089a387e23c7ecd3525c9189c379109119d6ab82e8dfbde0fdf6a7f9b68'

class ParameterEventDescriptors(msgspec.Struct, frozen=True, kw_only=True):
    new_parameters: list["rcl_interfaces.ParameterDescriptor"] = msgspec.field(default_factory=list)
    changed_parameters: list["rcl_interfaces.ParameterDescriptor"] = msgspec.field(default_factory=list)
    deleted_parameters: list["rcl_interfaces.ParameterDescriptor"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ParameterEventDescriptors'
    __hash__: ClassVar[str] = 'RIHS01_456f90915d72ef69379e702ef3ba2a115098ff677eab9c0080fa11239c6b147e'

class ParameterType(msgspec.Struct, frozen=True, kw_only=True):

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ParameterType'
    __hash__: ClassVar[str] = 'RIHS01_df29ed057a834862187be24dd187d981790ff3ea6502f4cd27b432cbc42c6d46'

class Parameter(msgspec.Struct, frozen=True, kw_only=True):
    name: str = ""
    value: "rcl_interfaces.ParameterValue | None" = None

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/Parameter'
    __hash__: ClassVar[str] = 'RIHS01_ddfe6442cffc462317adb5c92536a7b6dd55858c5c3e1e328165a6b73c2831af'

class ParameterEvent(msgspec.Struct, frozen=True, kw_only=True):
    stamp: "builtin_interfaces.Time | None" = msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})
    node: str = ""
    new_parameters: list["rcl_interfaces.Parameter"] = msgspec.field(default_factory=list)
    changed_parameters: list["rcl_interfaces.Parameter"] = msgspec.field(default_factory=list)
    deleted_parameters: list["rcl_interfaces.Parameter"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ParameterEvent'
    __hash__: ClassVar[str] = 'RIHS01_043e627780fcad87a22d225bc2a037361dba713fca6a6b9f4b869a5aa0393204'

class FloatingPointRange(msgspec.Struct, frozen=True, kw_only=True):
    from_value: float = 0.0
    to_value: float = 0.0
    step: float = 0.0

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/FloatingPointRange'
    __hash__: ClassVar[str] = 'RIHS01_e6af23a23c177fee5f3075c8b1e435162a9b63c863d78c06017460b49684262d'

class DescribeParametersRequest(msgspec.Struct, frozen=True, kw_only=True):
    names: list[str] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/DescribeParametersRequest'
    __hash__: ClassVar[str] = 'RIHS01_845b484d71eb0673dae682f2e3ba3c4851a65a3dcfb97bddd82c5b57e91e4cff'

class DescribeParametersResponse(msgspec.Struct, frozen=True, kw_only=True):
    descriptors: list["rcl_interfaces.ParameterDescriptor"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/DescribeParametersResponse'
    __hash__: ClassVar[str] = 'RIHS01_845b484d71eb0673dae682f2e3ba3c4851a65a3dcfb97bddd82c5b57e91e4cff'

class GetParameterTypesRequest(msgspec.Struct, frozen=True, kw_only=True):
    names: list[str] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/GetParameterTypesRequest'
    __hash__: ClassVar[str] = 'RIHS01_da199c878688b3e530bdfe3ca8f74cb9fa0c303101e980a9e8f260e25e1c80ca'

class GetParameterTypesResponse(msgspec.Struct, frozen=True, kw_only=True):
    types: bytes = b""

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/GetParameterTypesResponse'
    __hash__: ClassVar[str] = 'RIHS01_da199c878688b3e530bdfe3ca8f74cb9fa0c303101e980a9e8f260e25e1c80ca'

class GetParametersRequest(msgspec.Struct, frozen=True, kw_only=True):
    names: list[str] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/GetParametersRequest'
    __hash__: ClassVar[str] = 'RIHS01_bf9803d5c74cf989a5de3e0c2e99444599a627c7ff75f97b8c05b01003675cbc'

class GetParametersResponse(msgspec.Struct, frozen=True, kw_only=True):
    values: list["rcl_interfaces.ParameterValue"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/GetParametersResponse'
    __hash__: ClassVar[str] = 'RIHS01_bf9803d5c74cf989a5de3e0c2e99444599a627c7ff75f97b8c05b01003675cbc'

class ListParametersRequest(msgspec.Struct, frozen=True, kw_only=True):
    prefixes: list[str] = msgspec.field(default_factory=list)
    depth: int = 0

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ListParametersRequest'
    __hash__: ClassVar[str] = 'RIHS01_3e6062bfbb27bfb8730d4cef2558221f51a11646d78e7bb30a1e83afac3aad9d'

class ListParametersResponse(msgspec.Struct, frozen=True, kw_only=True):
    result: "rcl_interfaces.ListParametersResult | None" = None

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/ListParametersResponse'
    __hash__: ClassVar[str] = 'RIHS01_3e6062bfbb27bfb8730d4cef2558221f51a11646d78e7bb30a1e83afac3aad9d'

class SetParametersRequest(msgspec.Struct, frozen=True, kw_only=True):
    parameters: list["rcl_interfaces.Parameter"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/SetParametersRequest'
    __hash__: ClassVar[str] = 'RIHS01_56eed9a67e169f9cb6c1f987bc88f868c14a8fc9f743a263bc734c154015d7e0'

class SetParametersResponse(msgspec.Struct, frozen=True, kw_only=True):
    results: list["rcl_interfaces.SetParametersResult"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/SetParametersResponse'
    __hash__: ClassVar[str] = 'RIHS01_56eed9a67e169f9cb6c1f987bc88f868c14a8fc9f743a263bc734c154015d7e0'

class SetParametersAtomicallyRequest(msgspec.Struct, frozen=True, kw_only=True):
    parameters: list["rcl_interfaces.Parameter"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/SetParametersAtomicallyRequest'
    __hash__: ClassVar[str] = 'RIHS01_0e192ef259c07fc3c07a13191d27002222e65e00ccec653ca05e856f79285fcd'

class SetParametersAtomicallyResponse(msgspec.Struct, frozen=True, kw_only=True):
    result: "rcl_interfaces.SetParametersResult | None" = None

    __msgtype__: ClassVar[str] = 'rcl_interfaces/msg/SetParametersAtomicallyResponse'
    __hash__: ClassVar[str] = 'RIHS01_0e192ef259c07fc3c07a13191d27002222e65e00ccec653ca05e856f79285fcd'

class DescribeParameters:
    """Service grouping type. Use DescribeParameters.Request and DescribeParameters.Response."""
    __srvtype__: ClassVar[str] = 'rcl_interfaces/srv/DescribeParameters'
    Request: ClassVar[type] = DescribeParametersRequest
    Response: ClassVar[type] = DescribeParametersResponse

class GetParameterTypes:
    """Service grouping type. Use GetParameterTypes.Request and GetParameterTypes.Response."""
    __srvtype__: ClassVar[str] = 'rcl_interfaces/srv/GetParameterTypes'
    Request: ClassVar[type] = GetParameterTypesRequest
    Response: ClassVar[type] = GetParameterTypesResponse

class GetParameters:
    """Service grouping type. Use GetParameters.Request and GetParameters.Response."""
    __srvtype__: ClassVar[str] = 'rcl_interfaces/srv/GetParameters'
    Request: ClassVar[type] = GetParametersRequest
    Response: ClassVar[type] = GetParametersResponse

class ListParameters:
    """Service grouping type. Use ListParameters.Request and ListParameters.Response."""
    __srvtype__: ClassVar[str] = 'rcl_interfaces/srv/ListParameters'
    Request: ClassVar[type] = ListParametersRequest
    Response: ClassVar[type] = ListParametersResponse

class SetParameters:
    """Service grouping type. Use SetParameters.Request and SetParameters.Response."""
    __srvtype__: ClassVar[str] = 'rcl_interfaces/srv/SetParameters'
    Request: ClassVar[type] = SetParametersRequest
    Response: ClassVar[type] = SetParametersResponse

class SetParametersAtomically:
    """Service grouping type. Use SetParametersAtomically.Request and SetParametersAtomically.Response."""
    __srvtype__: ClassVar[str] = 'rcl_interfaces/srv/SetParametersAtomically'
    Request: ClassVar[type] = SetParametersAtomicallyRequest
    Response: ClassVar[type] = SetParametersAtomicallyResponse

