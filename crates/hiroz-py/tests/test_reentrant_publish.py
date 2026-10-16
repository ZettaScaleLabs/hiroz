#!/usr/bin/env python3
"""Publishing from inside a subscriber callback must not freeze the interpreter.

Two independent defects combined here:

1. hiroz declared every subscriber as a zenoh-ext ``AdvancedSubscriber``, which
   runs the user callback while holding a non-reentrant ``std::sync::Mutex``.
   Zenoh delivers same-session samples synchronously on the publishing thread, so
   a publish from inside a callback re-entered that mutex and deadlocked.
2. ``ZPublisher.publish`` held the GIL across the blocking publish, so the
   deadlock froze the *whole* interpreter -- no exception, no traceback, only an
   external kill.

Defect 1 is fixed differently per durability, and both paths are covered here:

* ``Volatile`` (the ROS 2 default) gets a plain zenoh subscriber. Samples that
  arrived over a transport run inline on the zenoh RX worker; samples published
  by the delivering thread itself are handed to a per-subscriber
  ``hiroz-sub-drain`` thread. That second case is what a re-entrant publish
  hits, so it *iterates* rather than recursing and needs no depth cap.
* ``TransientLocal`` keeps the advanced subscriber -- its reordering guarantees
  need that mutex -- so *every* sample moves off the delivering thread onto the
  drain thread.

Either way the drain thread is a *Rust* thread and is therefore invisible to
``threading.enumerate()``; the leak test below reads ``/proc/self/task``
instead, which is the only view Python has of it.

Everything here is deadline-guarded so a regression fails the suite rather than
wedging it. That guard is itself only meaningful because of fix 2 -- with the GIL
held across the block, no watchdog and no deadline thread can run at all, which
is what ``test_interpreter_stays_alive_during_reentrant_publish`` pins down.
"""

import gc
import itertools
import os
import threading
import time

import hiroz_py
import pytest
from hiroz_py import std_msgs

# Generous relative to the work done (a handful of intra-process publishes).
SCENARIO_TIMEOUT = 20.0
HOPS = 4

# How far the self-feeding loop must run to prove it iterates rather than
# recurses. Two orders of magnitude past the point where the old inline path
# need, and well past the stack depth a recursive implementation survives.
ITERATION_TARGET = 2000

DRAIN_THREAD_NAME = "hiroz-sub-drain"

DURABILITIES = ["volatile", "transient_local"]

_topic_seq = itertools.count()


def _topic(stem):
    """A fresh topic per scenario.

    TransientLocal replays history to late-joining subscribers, so reusing a
    topic across scenarios would leak samples from one into the next and make a
    hop chain appear to complete that never actually ran.
    """
    return f"/reentrant_py_{stem}_{next(_topic_seq)}"


def _qos(durability):
    return hiroz_py.QosProfile(durability=durability)


def _run_on_deadline(fn, timeout=SCENARIO_TIMEOUT):
    """Run ``fn`` on a daemon thread and assert it returned within ``timeout``.

    The seeding publish is what blocks in the unfixed build, so it is what has to
    be time-boxed. A daemon thread means a genuine deadlock leaves a stuck thread
    behind but still lets the process exit with a real failure -- provided the
    GIL is released across the block, which is fix 2.
    """
    error = []

    def target():
        try:
            fn()
        except BaseException as exc:  # noqa: BLE001 - re-raised below
            error.append(exc)

    driver = threading.Thread(target=target, daemon=True)
    driver.start()
    driver.join(timeout)
    assert not driver.is_alive(), (
        f"publish() from inside a subscriber callback did not return within "
        f"{timeout}s - re-entrant publish deadlocked"
    )
    if error:
        raise error[0]


# --------------------------------------------------------------------------
# Preconditions -- fail loudly rather than skipping silently.
#
# The sibling Rust suite (crates/hiroz-tests/tests/reentrant_service.rs) is
# gated behind ``#![cfg(feature = "ros-msgs")]`` and reports a green "0 passed"
# when the feature is off. Nothing here may be able to do that: if the
# environment cannot express a scenario, that is a failure, not a skip.
# --------------------------------------------------------------------------


def test_preconditions(context):
    """The APIs every scenario below depends on must exist and behave."""
    assert hasattr(hiroz_py, "QosProfile"), "hiroz_py.QosProfile is missing"

    for durability in DURABILITIES:
        profile = _qos(durability)
        assert durability in repr(profile).lower(), (
            f"QosProfile(durability={durability!r}) did not round-trip: {profile!r}"
        )

    node = context.create_node("reentrant_precond").with_namespace("/test").build()
    assert hasattr(node, "destroy_subscriber"), (
        "ZNode.destroy_subscriber is missing - the drain-thread leak test cannot "
        "drop subscribers and must not be reported as passing"
    )

    # The leak test can only see the Rust drain thread through procfs.
    assert os.path.isdir("/proc/self/task"), (
        "/proc/self/task is unavailable - the drain-thread leak test cannot run "
        "on this platform and must not be reported as passing"
    )


# --------------------------------------------------------------------------
# The 6-cell matrix: {volatile, transient_local}
#                  x {same-topic, two-topic cycle, unbounded feedback}.
# --------------------------------------------------------------------------


@pytest.mark.parametrize("durability", DURABILITIES)
def test_same_topic_republish(context, durability):
    """A callback republishing to its own topic must complete the hop chain."""
    node = (
        context.create_node(f"reentrant_same_{durability}")
        .with_namespace("/test")
        .build()
    )
    topic = _topic("same")
    qos = _qos(durability)
    publisher = node.create_publisher(topic, std_msgs.String, qos=qos)

    seen = []
    done = threading.Event()

    def on_message(msg):
        seen.append(msg.data)
        hop = int(msg.data)
        if hop < HOPS:
            # The re-entrant publish. Before the fix this never returned.
            publisher.publish(std_msgs.String(data=str(hop + 1)))
        else:
            done.set()

    node.create_subscriber(topic, std_msgs.String, qos=qos, callback=on_message)
    time.sleep(0.5)

    _run_on_deadline(lambda: publisher.publish(std_msgs.String(data="0")))

    assert done.wait(SCENARIO_TIMEOUT), f"only got {seen!r}, expected {HOPS + 1} hops"
    assert seen == [str(i) for i in range(HOPS + 1)], seen


@pytest.mark.parametrize("durability", DURABILITIES)
def test_two_topic_cycle(context, durability):
    """A -> B -> A across two subscribers must not deadlock either.

    Distinct from the same-topic case: the re-entrant publish lands on a
    *different* subscriber, so this exercises the delivering thread rather than
    one subscriber's own state.
    """
    node = (
        context.create_node(f"reentrant_cycle_{durability}")
        .with_namespace("/test")
        .build()
    )
    topic_a = _topic("cycle_a")
    topic_b = _topic("cycle_b")
    qos = _qos(durability)
    pub_a = node.create_publisher(topic_a, std_msgs.String, qos=qos)
    pub_b = node.create_publisher(topic_b, std_msgs.String, qos=qos)

    seen = []
    done = threading.Event()

    def hop_handler(which, forward_to):
        def handler(msg):
            seen.append((which, msg.data))
            hop = int(msg.data)
            if hop < HOPS:
                forward_to.publish(std_msgs.String(data=str(hop + 1)))
            else:
                done.set()

        return handler

    node.create_subscriber(
        topic_a, std_msgs.String, qos=qos, callback=hop_handler("a", pub_b)
    )
    node.create_subscriber(
        topic_b, std_msgs.String, qos=qos, callback=hop_handler("b", pub_a)
    )
    time.sleep(0.5)

    _run_on_deadline(lambda: pub_a.publish(std_msgs.String(data="0")))

    assert done.wait(SCENARIO_TIMEOUT), f"cycle stalled after {seen!r}"
    assert len(seen) == HOPS + 1, seen
    assert [d for _, d in seen] == [str(i) for i in range(HOPS + 1)], seen
    assert [t for t, _ in seen] == ["a", "b", "a", "b", "a"][: HOPS + 1], seen


@pytest.mark.parametrize("durability", DURABILITIES)
def test_self_feeding_loop_iterates(context, durability):
    """A callback that republishes every time must loop, not recurse.

    This is the Python-level view of the change that made the ``afor``
    benchmark's ``intra`` cell expressible. hiroz used to deliver a same-session
    sample inline on the publishing thread, so this shape was recursion: it grew
    the stack; before this fix it deadlocked on the first hop rather than
    it. Delivery now enqueues and returns, so each hop starts from a flat stack
    and the loop runs for as long as it is fed.

    The assertion is the inverse of the one it replaces: the loop must run far
    *past* the old cap.
    """
    node = (
        context.create_node(f"reentrant_loop_{durability}")
        .with_namespace("/test")
        .build()
    )
    topic = _topic("loop")
    qos = _qos(durability)
    publisher = node.create_publisher(topic, std_msgs.String, qos=qos)

    counter = itertools.count()
    delivered = []
    reached = threading.Event()

    def on_message(msg):
        n = next(counter)
        delivered.append(n)
        if n >= ITERATION_TARGET:
            # Stop feeding so the test ends on its own; the loop's *ability* to
            # keep going is what is under test.
            reached.set()
            return
        publisher.publish(std_msgs.String(data=str(n)))

    node.create_subscriber(topic, std_msgs.String, qos=qos, callback=on_message)
    time.sleep(0.5)

    _run_on_deadline(lambda: publisher.publish(std_msgs.String(data="0")))

    assert reached.wait(SCENARIO_TIMEOUT), (
        f"self-feeding loop stalled at {len(delivered)} deliveries, target "
        f"{ITERATION_TARGET}. Under the old inline dispatch this caps out at 16."
    )
    assert len(delivered) >= ITERATION_TARGET, len(delivered)


# --------------------------------------------------------------------------
# Fix 2: the interpreter must stay alive.
# --------------------------------------------------------------------------


def test_interpreter_stays_alive_during_reentrant_publish(context):
    """A watchdog thread must keep running across a re-entrant publish.

    This is the user-facing half of the fix and the only part a Rust test cannot
    check. ``ZPublisher.publish`` used to hold the GIL across the blocking zenoh
    publish, so a deadlock there did not stall one thread -- it stalled every
    thread. No exception, no traceback, no way to diagnose it from inside the
    process. ``py.allow_threads`` downgrades that to a single blocked thread.

    Failure mode on the unfixed build: this test does not fail, it *hangs the
    whole process*, because the assertions below also need the GIL. That is
    precisely the symptom, and it is why the unfixed baseline has to be measured
    under an external wall-clock timeout rather than trusted to self-report.
    """
    node = context.create_node("reentrant_watchdog").with_namespace("/test").build()
    topic = _topic("watchdog")
    publisher = node.create_publisher(topic, std_msgs.String)

    ticks = []
    stop = threading.Event()

    def watchdog():
        while not stop.is_set():
            ticks.append(time.monotonic())
            time.sleep(0.02)

    def on_message(msg):
        hop = int(msg.data)
        if hop < HOPS:
            publisher.publish(std_msgs.String(data=str(hop + 1)))

    node.create_subscriber(topic, std_msgs.String, callback=on_message)
    time.sleep(0.5)

    wd = threading.Thread(target=watchdog, daemon=True)
    wd.start()
    time.sleep(0.2)
    before = len(ticks)
    assert before > 0, "watchdog never started - the detector is broken, not the code"

    _run_on_deadline(lambda: publisher.publish(std_msgs.String(data="0")))

    time.sleep(0.3)
    after = len(ticks)
    stop.set()
    wd.join(5.0)

    assert after > before, (
        "the watchdog thread made no progress across the re-entrant publish - "
        "the GIL was held across the block and the whole interpreter went dark"
    )

    # No single stall longer than a generous multiple of the tick interval.
    gaps = [b - a for a, b in zip(ticks, ticks[1:])]
    worst = max(gaps) if gaps else 0.0
    assert worst < 2.0, (
        f"watchdog stalled for {worst:.2f}s during the publish - the interpreter "
        "was frozen even if it eventually recovered"
    )


# --------------------------------------------------------------------------
# The TransientLocal dispatcher thread must not leak.
# --------------------------------------------------------------------------


def _drain_threads():
    """Count live ``hiroz-sub-drain`` threads via procfs.

    ``threading.enumerate()`` cannot see these: they are Rust threads spawned by
    ``CallbackDispatcher::spawn`` and were never registered with CPython.
    ``/proc/self/task/*/comm`` is the only way Python can observe them, so it is
    the only honest detector for a leak.
    """
    total = 0
    for tid in os.listdir("/proc/self/task"):
        try:
            with open(f"/proc/self/task/{tid}/comm") as handle:
                if handle.read().strip() == DRAIN_THREAD_NAME:
                    total += 1
        except OSError:
            continue  # thread exited between listdir and open
    return total


def _settle(deadline, target=None):
    """Wait up to ``deadline`` for the drain count to reach ``target``."""
    gc.collect()
    end = time.monotonic() + deadline
    current = _drain_threads()
    while time.monotonic() < end:
        if target is not None and current == target:
            return current
        time.sleep(0.2)
        current = _drain_threads()
    return current


def test_transient_local_dispatcher_threads_do_not_leak(context):
    """Create and destroy TransientLocal subscribers; the count must return.

    Volatile *callback* subscribers also carry a dispatcher now (for the
    session-local delivery path), so the leak concern is no longer
    TransientLocal-only -- but TransientLocal spawns one unconditionally, which
    makes it the tighter test of the same teardown path.
    """
    node = context.create_node("reentrant_leak").with_namespace("/test").build()
    qos = _qos("transient_local")

    baseline_py = len(threading.enumerate())
    baseline_drain = _settle(deadline=3.0)

    subs_per_cycle = 4
    cycles = 3
    peaks = []

    for cycle in range(cycles):
        subs = [
            node.create_subscriber(
                _topic(f"leak_{cycle}_{i}"),
                std_msgs.String,
                qos=qos,
                callback=lambda _msg: None,
            )
            for i in range(subs_per_cycle)
        ]
        time.sleep(0.5)

        peak = _drain_threads()
        peaks.append(peak)
        # Precondition: if the dispatcher never spawned, this test is not
        # exercising the TransientLocal path and must fail rather than pass.
        assert peak >= baseline_drain + subs_per_cycle, (
            f"cycle {cycle}: expected at least {subs_per_cycle} new "
            f"{DRAIN_THREAD_NAME!r} threads over a baseline of {baseline_drain}, "
            f"saw {peak}. The TransientLocal dispatcher path was not exercised - "
            "this test proves nothing in this state."
        )

        for sub in subs:
            node.destroy_subscriber(sub)
        subs.clear()

        settled = _settle(deadline=15.0, target=baseline_drain)
        assert settled == baseline_drain, (
            f"cycle {cycle}: {settled - baseline_drain} {DRAIN_THREAD_NAME!r} "
            f"thread(s) leaked after destroying {subs_per_cycle} subscribers "
            f"(baseline {baseline_drain}, peak {peak})"
        )

    # Repeated cycles must not ratchet upward.
    assert max(peaks) - min(peaks) <= 1, (
        f"drain-thread peak drifted across cycles: {peaks} - threads accumulate"
    )

    final_py = len(threading.enumerate())
    assert final_py <= baseline_py, (
        f"Python-level threads grew from {baseline_py} to {final_py}"
    )
