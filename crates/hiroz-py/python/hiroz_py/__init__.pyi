"""Public re-exports for hiroz_py package (PEP 561)."""

from __future__ import annotations

from typing import Any, Final

# Re-export message types from hiroz_msgs_py.types
from hiroz_msgs_py import types as types

# Re-export individual message packages for convenience
from hiroz_msgs_py.types import (
    action_msgs as action_msgs,
    builtin_interfaces as builtin_interfaces,
    example_interfaces as example_interfaces,
    geometry_msgs as geometry_msgs,
    nav_msgs as nav_msgs,
    sensor_msgs as sensor_msgs,
    std_msgs as std_msgs,
    unique_identifier_msgs as unique_identifier_msgs,
)

# service_msgs was introduced in ROS 2 Iron (May 2023) as part of the service
# introspection feature. It contains types like ServiceEventInfo for monitoring
# service calls. This package doesn't exist in Humble (May 2022).
try:
    from hiroz_msgs_py.types import service_msgs as service_msgs
except ImportError:
    pass

# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------

class HirozError(Exception): ...
class TimeoutError(HirozError): ...
class SerializationError(HirozError): ...
class TypeMismatchError(HirozError): ...

# ---------------------------------------------------------------------------
# QosProfile
# ---------------------------------------------------------------------------

class QosProfile:
    def __init__(
        self,
        reliability: str | None = None,
        durability: str | None = None,
        history: str | None = None,
        depth: int | None = None,
        liveliness: str | None = None,
        deadline: float | None = None,
        lifespan: float | None = None,
        liveliness_lease_duration: float | None = None,
    ) -> None: ...
    @staticmethod
    def default() -> QosProfile: ...
    @staticmethod
    def sensor_data() -> QosProfile: ...
    @staticmethod
    def parameters() -> QosProfile: ...
    @staticmethod
    def services() -> QosProfile: ...
    @property
    def reliability(self) -> str: ...
    @property
    def durability(self) -> str: ...
    @property
    def history(self) -> str: ...
    @property
    def depth(self) -> int: ...
    def __repr__(self) -> str: ...

# ---------------------------------------------------------------------------
# QoS constants
# ---------------------------------------------------------------------------

QOS_DEFAULT: Final[QosProfile] = QosProfile.default()
QOS_SENSOR_DATA: Final[QosProfile] = QosProfile.sensor_data()
QOS_PARAMETERS: Final[QosProfile] = QosProfile.parameters()
QOS_SERVICES: Final[QosProfile] = QosProfile.services()

# ---------------------------------------------------------------------------
# QoS policy enum holders (P8)
# ---------------------------------------------------------------------------

class ReliabilityPolicy:
    RELIABLE: Final[str]
    BEST_EFFORT: Final[str]

class DurabilityPolicy:
    VOLATILE: Final[str]
    TRANSIENT_LOCAL: Final[str]

class HistoryPolicy:
    KEEP_LAST: Final[str]
    KEEP_ALL: Final[str]

class LivelinessPolicy:
    AUTOMATIC: Final[str]
    MANUAL_BY_TOPIC: Final[str]
    MANUAL_BY_NODE: Final[str]

# A QoS argument: a QosProfile, an int depth shorthand, or a legacy dict.
QosLike = QosProfile | int | dict[str, object]

# ---------------------------------------------------------------------------
# GoalStatus
# ---------------------------------------------------------------------------

class GoalStatus:
    UNKNOWN: int
    ACCEPTED: int
    EXECUTING: int
    CANCELING: int
    SUCCEEDED: int
    CANCELED: int
    ABORTED: int

    def __init__(self, value: int) -> None: ...
    @property
    def value(self) -> int: ...
    def is_active(self) -> bool: ...
    def is_terminal(self) -> bool: ...
    def __repr__(self) -> str: ...
    def __eq__(self, other: object) -> bool: ...

# ---------------------------------------------------------------------------
# ZPayloadView — zero-copy buffer protocol wrapper
# ---------------------------------------------------------------------------

class ZPayloadView:
    def __len__(self) -> int: ...
    def __bool__(self) -> bool: ...
    @property
    def is_zero_copy_py(self) -> bool: ...
    def __buffer__(self, flags: int) -> memoryview: ...

# ---------------------------------------------------------------------------
# ZBufView — zenoh buffer view
# ---------------------------------------------------------------------------

class ZBufView:
    """View into a Zenoh ZBuf. Implements Python buffer protocol."""
    ...

# ---------------------------------------------------------------------------
# ZContextBuilder
# ---------------------------------------------------------------------------

class ZContextBuilder:
    def __init__(self) -> None: ...
    def with_domain_id(self, domain_id: int) -> ZContextBuilder: ...
    def with_logging_enabled(self) -> ZContextBuilder: ...
    def with_connect_endpoints(self, endpoints: list[str]) -> ZContextBuilder: ...
    def disable_multicast_scouting(self) -> ZContextBuilder: ...
    def with_mode(self, mode: str) -> ZContextBuilder: ...
    def with_router_endpoint(self, endpoint: str) -> ZContextBuilder: ...
    def with_config_file(self, path: str) -> ZContextBuilder: ...
    def with_json(self, key: str, value: str) -> ZContextBuilder: ...
    def with_remap_rule(self, rule: str) -> ZContextBuilder: ...
    def with_remap_rules(self, rules: list[str]) -> ZContextBuilder: ...
    def with_enclave(self, enclave: str) -> ZContextBuilder: ...
    def connect_to_local_zenohd(self) -> ZContextBuilder: ...
    def with_shm_enabled(self) -> ZContextBuilder: ...
    def with_shm_pool_size(self, size_bytes: int) -> ZContextBuilder: ...
    def with_shm_threshold(self, threshold: int) -> ZContextBuilder: ...
    def build(self) -> ZContext: ...

# ---------------------------------------------------------------------------
# ZContext
# ---------------------------------------------------------------------------

class ZContext:
    def __enter__(self) -> ZContext: ...
    def __exit__(self, *args: Any) -> None: ...
    def shutdown(self) -> None: ...
    def create_node(self, name: str) -> ZNodeBuilder: ...

# ---------------------------------------------------------------------------
# ZNodeBuilder
# ---------------------------------------------------------------------------

class ZNodeBuilder:
    def with_namespace(self, namespace: str) -> ZNodeBuilder: ...
    def build(self) -> ZNode: ...

# ---------------------------------------------------------------------------
# ZNode
# ---------------------------------------------------------------------------

class ZNode:
    @property
    def name(self) -> str: ...
    @property
    def namespace(self) -> str: ...
    @property
    def fully_qualified_name(self) -> str: ...
    def create_publisher(
        self,
        topic: str,
        msg_type: Any,
        qos: QosLike | None = None,
    ) -> ZPublisher: ...
    def create_subscriber(
        self,
        topic: str,
        msg_type: Any,
        qos: QosLike | None = None,
        callback: Any | None = None,
    ) -> ZSubscriber: ...
    # rclpy-style alias for create_subscriber (P3).
    def create_subscription(
        self,
        topic: str,
        msg_type: Any,
        qos: QosLike | None = None,
        callback: Any | None = None,
    ) -> ZSubscriber: ...
    def create_client(self, service: str, srv_type: Any) -> ZClient: ...
    def create_server(
        self, service: str, srv_type: Any, callback: Any | None = None
    ) -> ZServer: ...
    # rclpy-style alias for create_server (P3); pass callback= for callback mode (P6).
    def create_service(
        self, service: str, srv_type: Any, callback: Any | None = None
    ) -> ZServer: ...
    def create_action_client(
        self,
        action_name: str,
        goal_type: Any,
        result_type: Any | None = None,
        feedback_type: Any | None = None,
    ) -> ZActionClient: ...
    def create_action_server(
        self,
        action_name: str,
        goal_type: Any,
        result_type: Any | None = None,
        feedback_type: Any | None = None,
    ) -> ZActionServer: ...
    def get_topic_names_and_types(self) -> list[tuple[str, str]]: ...
    def get_node_names(self) -> list[tuple[str, str]]: ...
    def get_service_names_and_types(self) -> list[tuple[str, str]]: ...
    def count_publishers(self, topic: str) -> int: ...
    def count_subscribers(self, topic: str) -> int: ...

# ---------------------------------------------------------------------------
# ZPublisher
# ---------------------------------------------------------------------------

class ZPublisher:
    def publish(self, data: Any) -> None: ...
    def publish_raw(self, data: bytes) -> None: ...
    def wait_for_subscription(
        self, count: int = 1, timeout: float | None = None
    ) -> bool: ...
    def get_type_name(self) -> str: ...

# ---------------------------------------------------------------------------
# ZSubscriber
# ---------------------------------------------------------------------------

class ZSubscriber:
    @property
    def is_callback(self) -> bool: ...
    def recv(self, timeout: float | None = None) -> Any | None: ...
    def try_recv(self) -> Any | None: ...
    def recv_serialized(self, timeout: float | None = None) -> bytes | None: ...
    def try_recv_serialized(self) -> bytes | None: ...
    def recv_raw_view(self, timeout: float | None = None) -> ZPayloadView | None: ...
    def try_recv_raw_view(self) -> ZPayloadView | None: ...
    def get_type_name(self) -> str: ...

# ---------------------------------------------------------------------------
# ZClient (service client)
# ---------------------------------------------------------------------------

class ZClient:
    def call(self, data: Any, timeout: float | None = None) -> Any: ...
    def wait_for_service(self, timeout: float | None = None) -> bool: ...
    def get_type_name(self) -> str: ...

# ---------------------------------------------------------------------------
# ZServer (service server)
# ---------------------------------------------------------------------------

class ZServer:
    def take_request(self) -> tuple[dict[str, Any], Any]: ...
    def send_response(self, response: Any, request_id: dict[str, Any]) -> None: ...
    def get_type_name(self) -> str: ...
    @property
    def last_error(self) -> str | None: ...

# ---------------------------------------------------------------------------
# ZActionClient
# ---------------------------------------------------------------------------

class ZActionClient:
    def send_goal(self, goal: Any) -> ActionGoalHandle: ...
    def wait_for_server(self, timeout: float | None = None) -> bool: ...
    @property
    def goal_type(self) -> Any: ...

# ---------------------------------------------------------------------------
# ActionGoalHandle (client side)
# ---------------------------------------------------------------------------

class ActionGoalHandle:
    @property
    def goal_id(self) -> bytes: ...
    @property
    def status(self) -> int: ...
    def recv_feedback(self, timeout: float | None = None) -> Any | None: ...
    def try_recv_feedback(self) -> Any | None: ...
    def get_result(self, timeout: float | None = None) -> Any | None: ...
    def cancel(self) -> None: ...

# ---------------------------------------------------------------------------
# ZActionServer
# ---------------------------------------------------------------------------

class ZActionServer:
    def recv_goal(self, timeout: float | None = None) -> ServerGoalRequest | None: ...

# ---------------------------------------------------------------------------
# ServerGoalRequest (requested state — not yet accepted/rejected)
# ---------------------------------------------------------------------------

class ServerGoalRequest:
    @property
    def goal_id(self) -> bytes: ...
    def goal(self) -> Any: ...
    def accept_and_execute(self) -> ServerGoalHandle: ...
    def reject(self) -> None: ...

# ---------------------------------------------------------------------------
# ServerGoalHandle (executing state)
# ---------------------------------------------------------------------------

class ServerGoalHandle:
    @property
    def goal_id(self) -> bytes: ...
    def goal(self) -> Any: ...
    @property
    def is_cancel_requested(self) -> bool: ...
    def publish_feedback(self, feedback: Any) -> None: ...
    def succeed(self, result: Any) -> None: ...
    def abort(self, result: Any) -> None: ...
    def canceled(self, result: Any) -> None: ...

# ---------------------------------------------------------------------------
# Free functions
# ---------------------------------------------------------------------------

def list_registered_types() -> list[str]: ...
