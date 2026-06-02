"""Auto-generated ROS 2 message types for type_description_interfaces."""
import msgspec
from typing import ClassVar

class TypeSource(msgspec.Struct, frozen=True, kw_only=True):
    type_name: str = ""
    encoding: str = ""
    raw_file_contents: str = ""

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/TypeSource'
    __hash__: ClassVar[str] = 'RIHS01_faeaec7596c04ecf5b6e99ad225e4c7cbb997ad5435f793526fb3984d011aae5'

class FieldType(msgspec.Struct, frozen=True, kw_only=True):
    type_id: int = 0
    capacity: int = 0
    string_capacity: int = 0
    nested_type_name: str = ""

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/FieldType'
    __hash__: ClassVar[str] = 'RIHS01_a70b6dd919645a03a3586f7f821defbc886ea3e531a1d95cc0f380a3973ccaa6'

class TypeDescription(msgspec.Struct, frozen=True, kw_only=True):
    type_description: "type_description_interfaces.IndividualTypeDescription | None" = None
    referenced_type_descriptions: list["type_description_interfaces.IndividualTypeDescription"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/TypeDescription'
    __hash__: ClassVar[str] = 'RIHS01_739f2508c9fa3a6f330913ff5b9d25fb74159a077da71e1087f51a60c12a080b'

class Field(msgspec.Struct, frozen=True, kw_only=True):
    name: str = ""
    type: "type_description_interfaces.FieldType | None" = None
    default_value: str = ""

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/Field'
    __hash__: ClassVar[str] = 'RIHS01_c0b01379cd4226281285ccaf6be46653968f855f7c5e41614ff5d7a854efef7c'

class IndividualTypeDescription(msgspec.Struct, frozen=True, kw_only=True):
    type_name: str = ""
    fields: list["type_description_interfaces.Field"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/IndividualTypeDescription'
    __hash__: ClassVar[str] = 'RIHS01_55c827d86c3c141bdd318fe6c22e11190e4d3b37c8f4f9751a084aa05ce96560'

class KeyValue(msgspec.Struct, frozen=True, kw_only=True):
    key: str = ""
    value: str = ""

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/KeyValue'
    __hash__: ClassVar[str] = 'RIHS01_274fe56bf14f33c7512e34c646a37579ee36779f745f049a9760763e817f0c42'

class GetTypeDescriptionRequest(msgspec.Struct, frozen=True, kw_only=True):
    type_name: str = ""
    type_hash: str = ""
    include_type_sources: bool = False

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/GetTypeDescriptionRequest'
    __hash__: ClassVar[str] = 'RIHS01_69b9c19c1021405984cc60dbbb1edceb147a6538b411d812ba6afabeed962cd5'

class GetTypeDescriptionResponse(msgspec.Struct, frozen=True, kw_only=True):
    successful: bool = False
    failure_reason: str = ""
    type_description: "type_description_interfaces.TypeDescription | None" = None
    type_sources: list["type_description_interfaces.TypeSource"] = msgspec.field(default_factory=list)
    extra_information: list["type_description_interfaces.KeyValue"] = msgspec.field(default_factory=list)

    __msgtype__: ClassVar[str] = 'type_description_interfaces/msg/GetTypeDescriptionResponse'
    __hash__: ClassVar[str] = 'RIHS01_69b9c19c1021405984cc60dbbb1edceb147a6538b411d812ba6afabeed962cd5'

