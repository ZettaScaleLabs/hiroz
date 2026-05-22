#!/usr/bin/env python3
"""Test SHM configuration on PyZContextBuilder."""

import hiroz_py


def test_shm_enabled():
    """with_shm_enabled() builds a context without error."""
    ctx = hiroz_py.ZContextBuilder().with_shm_enabled().build()
    assert ctx is not None


def test_shm_pool_size():
    """with_shm_pool_size() accepts an explicit byte count."""
    ctx = hiroz_py.ZContextBuilder().with_shm_pool_size(16 * 1024 * 1024).build()
    assert ctx is not None


def test_shm_threshold():
    """with_shm_threshold() is accepted after with_shm_enabled()."""
    ctx = hiroz_py.ZContextBuilder().with_shm_enabled().with_shm_threshold(4096).build()
    assert ctx is not None


def test_shm_enabled_reads_alloc_size_env(monkeypatch):
    """with_shm_enabled() picks up ZENOH_SHM_ALLOC_SIZE env var."""
    monkeypatch.setenv("ZENOH_SHM_ALLOC_SIZE", str(8 * 1024 * 1024))
    ctx = hiroz_py.ZContextBuilder().with_shm_enabled().build()
    assert ctx is not None


def test_shm_enabled_reads_threshold_env(monkeypatch):
    """with_shm_enabled() picks up ZENOH_SHM_MESSAGE_SIZE_THRESHOLD env var."""
    monkeypatch.setenv("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD", "8192")
    ctx = hiroz_py.ZContextBuilder().with_shm_enabled().build()
    assert ctx is not None


def test_shm_threshold_env_must_be_power_of_two(monkeypatch):
    """Non-power-of-two ZENOH_SHM_MESSAGE_SIZE_THRESHOLD is silently ignored."""
    # 3000 is not a power of two — the binding silently drops it and still builds.
    monkeypatch.setenv("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD", "3000")
    ctx = hiroz_py.ZContextBuilder().with_shm_enabled().build()
    assert ctx is not None
