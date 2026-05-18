"""Tests for PEP 561 type stub distribution.

Verifies that:
- py.typed marker is present in the installed package
- __init__.pyi is present (in source tree)
- All expected public names are accessible on the hiroz_py module
- QoS constants are QosProfile instances
"""

import importlib.util
import ast
from pathlib import Path

import hiroz_py


# ── Marker and stub files ─────────────────────────────────────────────────────


def test_py_typed_marker_present():
    """py.typed must be present in the installed package (PEP 561 requirement)."""
    spec = importlib.util.find_spec("hiroz_py")
    assert spec is not None, "hiroz_py not installed"
    pkg_dir = Path(spec.submodule_search_locations[0])
    marker = pkg_dir / "py.typed"
    assert marker.exists(), f"py.typed not found at {marker}"
    assert marker.stat().st_size == 0 or marker.read_bytes() == b"", (
        "py.typed must be an empty marker file"
    )


def test_init_pyi_is_valid_python():
    """__init__.pyi must exist in the source tree and parse as valid Python."""
    spec = importlib.util.find_spec("hiroz_py")
    assert spec is not None
    pkg_dir = Path(spec.submodule_search_locations[0])
    stub = pkg_dir / "__init__.pyi"
    assert stub.exists(), f"__init__.pyi not found at {stub}"
    source = stub.read_text()
    ast.parse(source)


# ── Public API surface ────────────────────────────────────────────────────────

EXPECTED_PUBLIC_NAMES = {
    # Core types
    "ZContextBuilder",
    "ZContext",
    "ZNodeBuilder",
    "ZNode",
    # Pub/sub
    "ZPublisher",
    "ZSubscriber",
    # Services
    "ZClient",
    "ZServer",
    # Actions
    "ZActionClient",
    "ActionGoalHandle",
    "ZActionServer",
    "ServerGoalRequest",
    "ServerGoalHandle",
    "GoalStatus",
    # Buffer types
    "ZPayloadView",
    "ZBufView",
    # QoS
    "QosProfile",
    "QOS_DEFAULT",
    "QOS_SENSOR_DATA",
    "QOS_PARAMETERS",
    "QOS_SERVICES",
    # Exceptions
    "HirozError",
    "TimeoutError",
    "SerializationError",
    "TypeMismatchError",
    # Functions
    "list_registered_types",
    # Message packages
    "types",
    "std_msgs",
    "geometry_msgs",
    "sensor_msgs",
    "nav_msgs",
    "builtin_interfaces",
    "action_msgs",
    "unique_identifier_msgs",
    "example_interfaces",
}


def test_expected_names_are_accessible():
    """All expected public names must be accessible as attributes on hiroz_py."""
    missing = [name for name in EXPECTED_PUBLIC_NAMES if not hasattr(hiroz_py, name)]
    assert not missing, f"Names missing from hiroz_py: {sorted(missing)}"


# ── QoS constants ─────────────────────────────────────────────────────────────


def test_qos_constants_are_qos_profile_instances():
    """QoS constants must be QosProfile instances, not plain dicts."""
    assert isinstance(hiroz_py.QOS_DEFAULT, hiroz_py.QosProfile)
    assert isinstance(hiroz_py.QOS_SENSOR_DATA, hiroz_py.QosProfile)
    assert isinstance(hiroz_py.QOS_PARAMETERS, hiroz_py.QosProfile)
    assert isinstance(hiroz_py.QOS_SERVICES, hiroz_py.QosProfile)


def test_native_submodule_accessible():
    """hiroz_py._native must be importable as a submodule."""
    from hiroz_py import _native  # noqa: F401
