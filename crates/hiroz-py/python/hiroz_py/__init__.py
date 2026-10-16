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


# ---------------------------------------------------------------------------
# rclpy-style method aliases (P3)
#
# hiroz keeps its native names (create_subscriber / create_server) as the
# canonical API; these aliases let rclpy code read naturally. create_service
# is a true alias because create_server now supports rclpy's optional
# callback= form (P6) in addition to pull mode.
# ---------------------------------------------------------------------------

ZNode.create_subscription = ZNode.create_subscriber  # type: ignore[attr-defined]
ZNode.create_service = ZNode.create_server  # type: ignore[attr-defined]


# ---------------------------------------------------------------------------
# QoS policy enum holders (P8)
#
# String-valued so they parse straight through QosProfile, while giving users
# discoverable, typo-proof constants instead of bare strings. Mirrors rclpy's
# rclpy.qos.ReliabilityPolicy / DurabilityPolicy / HistoryPolicy / LivelinessPolicy.
# ---------------------------------------------------------------------------


class ReliabilityPolicy:
    """QoS reliability policy constants."""

    RELIABLE: Final[str] = "reliable"
    BEST_EFFORT: Final[str] = "best_effort"


class DurabilityPolicy:
    """QoS durability policy constants."""

    VOLATILE: Final[str] = "volatile"
    TRANSIENT_LOCAL: Final[str] = "transient_local"


class HistoryPolicy:
    """QoS history policy constants."""

    KEEP_LAST: Final[str] = "keep_last"
    KEEP_ALL: Final[str] = "keep_all"


class LivelinessPolicy:
    """QoS liveliness policy constants."""

    AUTOMATIC: Final[str] = "automatic"
    MANUAL_BY_TOPIC: Final[str] = "manual_by_topic"
    MANUAL_BY_NODE: Final[str] = "manual_by_node"
