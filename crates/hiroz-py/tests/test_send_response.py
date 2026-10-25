"""A malformed request_id must raise a Python exception, never panic."""

import pytest
from hiroz_py import example_interfaces

GID = list(range(16))


def response():
    return example_interfaces.AddTwoIntsResponse(sum=3)


def test_missing_sn_raises_key_error(server):
    with pytest.raises(KeyError, match="sn"):
        server.send_response(response(), {"gid": GID})


def test_missing_gid_raises_key_error(server):
    with pytest.raises(KeyError, match="gid"):
        server.send_response(response(), {"sn": 1})


def test_empty_request_id_raises_key_error(server):
    with pytest.raises(KeyError):
        server.send_response(response(), {})


@pytest.mark.parametrize("length", [0, 4, 15, 17, 32])
def test_gid_of_wrong_length_raises_value_error(server, length):
    with pytest.raises(ValueError, match="16"):
        server.send_response(response(), {"sn": 1, "gid": [0] * length})


def test_non_integer_sn_raises_type_error(server):
    with pytest.raises(TypeError):
        server.send_response(response(), {"sn": "one", "gid": GID})
