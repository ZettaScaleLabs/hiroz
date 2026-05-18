#!/usr/bin/env python3
"""Zenoh router helper for benchmark tests.

This module provides a simple way to start an embedded Zenoh router
for benchmarks that need isolated pub/sub communication.
"""

import atexit
import random
import time
from dataclasses import dataclass
from typing import Optional

import zenoh


@dataclass
class TestRouter:
    """Manages an embedded Zenoh router for tests.

    Usage:
        router = TestRouter.start()
        # Use router.endpoint with ZContextBuilder:
        ctx = ZContextBuilder()
            .with_connect_endpoints([router.endpoint])
            .disable_multicast_scouting()
            .build()
        # ... run tests ...
        router.close()

    Or as context manager:
        with TestRouter.start() as router:
            ctx = ZContextBuilder()
                .with_connect_endpoints([router.endpoint])
                .disable_multicast_scouting()
                .build()
            # ... run tests ...
    """

    port: int
    endpoint: str
    session: zenoh.Session

    @classmethod
    def start(cls, port: Optional[int] = None) -> "TestRouter":
        """Start a new Zenoh router on the given or a random port."""
        if port is None:
            # Use a random high port to avoid collisions
            port = random.randint(30000, 50000)

        endpoint = f"tcp/127.0.0.1:{port}"

        print(f"Starting Zenoh router on {endpoint}...")

        config = zenoh.Config()
        config.insert_json5("mode", '"router"')
        config.insert_json5("listen/endpoints", f'["{endpoint}"]')
        config.insert_json5("scouting/multicast/enabled", "false")

        session = zenoh.open(config)

        # Wait for router to be ready
        time.sleep(0.3)
        print(f"Zenoh router ready on {endpoint}")

        router = cls(port=port, endpoint=endpoint, session=session)

        # Register cleanup on exit
        atexit.register(router.close)

        return router

    def close(self):
        """Close the router session."""
        if self.session is not None:
            print(f"Closing Zenoh router on {self.endpoint}")
            self.session.close()
            self.session = None

    def __enter__(self) -> "TestRouter":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


def create_context_with_router(router: TestRouter):
    """Create a hiroz-py context connected to the given router.

    Args:
        router: TestRouter instance to connect to

    Returns:
        A configured ZContext connected to the router
    """
    import hiroz_py

    return (
        hiroz_py.ZContextBuilder()
        .with_connect_endpoints([router.endpoint])
        .disable_multicast_scouting()
        .build()
    )


if __name__ == "__main__":
    # Quick test
    print("Testing router startup...")
    with TestRouter.start() as router:
        print(f"Router running on {router.endpoint}")
        time.sleep(1)
    print("Router closed successfully")
