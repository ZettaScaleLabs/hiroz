"""hiroz-py: Python bindings for hiroz, a native Rust ROS 2 implementation using Zenoh."""

# Re-export message types from hiroz_msgs_py.types
from typing import Final
from hiroz_msgs_py import types

# Re-export individual message packages for convenience
from hiroz_msgs_py.types import (
    action_msgs,
    builtin_interfaces,
    example_interfaces,
    geometry_msgs,
    nav_msgs,
    sensor_msgs,
    std_msgs,
    unique_identifier_msgs,
)
from ._native import *

# service_msgs was introduced in ROS 2 Iron (May 2023) as part of the service
# introspection feature. It contains types like ServiceEventInfo for monitoring
# service calls. This package doesn't exist in Humble (May 2022).
try:
    from hiroz_msgs_py.types import service_msgs
except ImportError:
    pass

# ---------------------------------------------------------------------------
# QoS constants override
# ---------------------------------------------------------------------------

QOS_DEFAULT: Final[QosProfile] = QosProfile.default()
QOS_SENSOR_DATA: Final[QosProfile] = QosProfile.sensor_data()
QOS_PARAMETERS: Final[QosProfile] = QosProfile.parameters()
QOS_SERVICES: Final[QosProfile] = QosProfile.services()
