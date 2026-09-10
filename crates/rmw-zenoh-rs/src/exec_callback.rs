//! The executor-notification slot shared by every rmw entity that can wake an
//! rclcpp executor: subscriptions, services, and clients.
//!
//! # The defect this type exists to prevent
//!
//! Each of those three entities used to carry the same trio of independent
//! mutexes — `callback`, `callback_user_data`, `unread_count` — and every site
//! that notified the executor locked one or two of them *and then invoked the
//! callback while still holding the guard*:
//!
//! ```ignore
//! if let Ok(cb) = callback_holder.lock() {
//!     if let Some(callback_fn) = *cb {
//!         if let Ok(user_data) = user_data_holder.lock() {
//!             unsafe { callback_fn(user_data_ptr, 1) };   // <-- executor, under two guards
//! ```
//!
//! `std::sync::Mutex` is not reentrant. A callback that re-enters the rmw API
//! for the same entity — which rclcpp does, because installing and clearing
//! `on_new_message` callbacks is how executors attach and detach — blocks on a
//! lock its own thread already holds. That is a deterministic hang, not a race.
//!
//! # The shape of the fix
//!
//! One mutex, and every operation *collects what it needs under the lock, drops
//! the guard, and only then calls into user code*. This is the same pattern the
//! hiroz core fixes established (`GraphEventManager::trigger_graph_change`,
//! `ParameterState::validate_and_apply`, `update_shared_event_status`).
//!
//! Collapsing three mutexes into one is part of the fix, not incidental
//! tidying: with three, "notify" had to hold two guards at once to read a
//! callback and its user-data together, and the correctness of the whole thing
//! rested on every site agreeing on a lock order. With one, the invariant is
//! local and there is no order to get wrong.
//!
//! # Why dropping the guard is not sufficient on its own
//!
//! `rmw_zenoh_cpp` dispatches this callback *under* its `event_mutex_`
//! (`DataCallbackManager::trigger_callback`), and `event_set_callback` takes the
//! same mutex. That is not only exclusion — it is a **lifetime guarantee**: once
//! `set_callback(nullptr)` returns, no callback is in flight, so the caller may
//! free whatever `user_data` pointed at.
//!
//! Collecting the callback and its `user_data` under the lock and dispatching
//! after the guard drops removes that guarantee. The window:
//!
//! 1. A delivery thread enters [`ExecCallback::notify_one`], snapshots
//!    `(callback, user_data)`, and drops the guard.
//! 2. The executor thread calls [`ExecCallback::set`] with `None`. It takes the
//!    lock, clears the slot, and returns — without waiting.
//! 3. The entity is destroyed and `user_data` is freed.
//! 4. The delivery thread calls the callback with its stale snapshot —
//!    use-after-free.
//!
//! Restoring the C++ shape is not an option: dispatching under the lock is
//! exactly the deadlock above. So the exclusion and the lifetime guarantee are
//! separated. Each dispatch registers the thread performing it, and `set` waits
//! for registered dispatches **on other threads** to finish before it returns.
//!
//! The thread distinction is the whole point, and it is what makes this
//! different from the original bug. A callback that re-enters `set` on its own
//! thread must not wait — waiting for itself is the deadlock. A `set` on an
//! unrelated thread must wait, because it is the one about to free the pointer.
//!
//! ## What this still does not survive
//!
//! Two threads *both* dispatching this entity's callback *and* both re-entering
//! `set` from inside it will wait on each other. Each is in a live dispatch that
//! may still touch its `user_data` after `set` returns, so neither wait can
//! safely be skipped.
//!
//! Stated rather than hidden, because it is a real residual — but it is not a
//! regression against the reference. `rmw_zenoh_cpp` cannot survive re-entrant
//! `set` at all: `set_callback` takes `event_mutex_` (and replays its backlog
//! under it), so a callback dispatched from `trigger_callback` that calls
//! `set_callback` re-locks a non-recursive `std::mutex` on the thread that
//! already holds it. That deadlocks with one thread. This deadlocks only with
//! two threads mutually re-entering, and the single-threaded case — the one
//! rclcpp actually exercises when an executor detaches from inside a
//! notification — is the case this type makes work.
//!
//! # Enforcement
//!
//! The mutex is a [`hiroz::reentrancy::TrackedMutex`], so its guards are counted
//! on the current thread, and both call sites go through
//! [`hiroz::invoke_user_callback!`], which asserts the count is zero before
//! dispatching. In debug builds a reintroduction of the defect panics with the
//! site name instead of hanging; in release both the counter and the assertion
//! compile to nothing.
//!
//! Neither the state guard nor the in-flight guard is ever held across user
//! code. Where both are taken, the order is always state then in-flight.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;

use hiroz::invoke_user_callback;
use hiroz::reentrancy::TrackedMutex;

/// The one callback signature rmw uses for all three "new item arrived"
/// notifications. `rmw_subscription_new_message_callback_t`,
/// `rmw_service_new_request_callback_t` and `rmw_client_new_response_callback_t`
/// are all aliases of `Option<ExecCallbackFn>`.
pub type ExecCallbackFn = unsafe extern "C" fn(user_data: *const std::ffi::c_void, count: usize);

#[derive(Default)]
struct State {
    /// The executor callback, or `None` when no executor is attached.
    callback: Option<ExecCallbackFn>,
    /// The executor's opaque pointer, held as a `usize` so `State` stays `Send`.
    user_data: usize,
    /// Items that arrived while `callback` was `None`, replayed on install.
    unread: usize,
}

/// Threads currently inside a dispatch.
///
/// One entry per active dispatch, not per thread: a callback may re-enter
/// `notify_one` on the same thread, so the same `ThreadId` can appear more than
/// once and each occurrence must be removed independently.
#[derive(Default)]
struct InFlight {
    threads: Vec<ThreadId>,
}

/// The in-flight registry and the condvar `set` waits on.
type InFlightSlot = (Mutex<InFlight>, Condvar);

/// Deregisters this thread's dispatch on scope exit, **including on unwind**.
///
/// Without the unwind path, a panicking executor callback would leave its entry
/// behind forever and every later `set` would block on a dispatch that has
/// already finished — trading a use-after-free for a permanent hang.
struct DispatchToken<'a> {
    slot: &'a InFlightSlot,
}

impl Drop for DispatchToken<'_> {
    fn drop(&mut self) {
        let (lock, condvar) = self.slot;
        // Poison-tolerant: this lock is only ever held for a push or a remove,
        // never across user code, so a poisoned state carries no broken
        // invariant — but failing to deregister here would hang every `set`.
        let mut in_flight = lock.lock().unwrap_or_else(|e| e.into_inner());
        let me = std::thread::current().id();
        if let Some(i) = in_flight.threads.iter().position(|t| *t == me) {
            in_flight.threads.swap_remove(i);
        }
        drop(in_flight);
        condvar.notify_all();
    }
}

/// Shared executor-notification state for one rmw entity.
///
/// Cloning shares the underlying slot; the delivery thread and the rmw API
/// entry points hold clones of the same `ExecCallback`.
#[derive(Clone)]
pub struct ExecCallback {
    state: Arc<TrackedMutex<State>>,
    /// Dispatches currently running, so [`set`] can wait for the ones that
    /// captured the outgoing `user_data`.
    ///
    /// [`set`]: Self::set
    in_flight: Arc<InFlightSlot>,
    /// Entity kind, reproduced in the re-entrancy panic message.
    site: &'static str,
}

impl ExecCallback {
    /// `site` names the entity kind ("subscription", "service", "client") and
    /// appears in the re-entrancy assertion message.
    pub fn new(site: &'static str) -> Self {
        Self {
            state: Arc::new(TrackedMutex::new(State::default())),
            in_flight: Arc::new((Mutex::new(InFlight::default()), Condvar::new())),
            site,
        }
    }

    /// Register this thread as dispatching, returning the token that removes it.
    ///
    /// **Must be called while the state guard is still held.** `set` swaps the
    /// slot under that same guard, so registering before releasing it is what
    /// guarantees that every dispatch holding the outgoing `user_data` is
    /// already visible to `set`'s wait. Register after, and step 2 of the race
    /// in the module docs reopens.
    fn enter_dispatch(&self) -> DispatchToken<'_> {
        let (lock, _) = &*self.in_flight;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .threads
            .push(std::thread::current().id());
        DispatchToken {
            slot: &self.in_flight,
        }
    }

    /// Block until no *other* thread is inside a dispatch.
    ///
    /// Entries for the current thread are ignored on purpose: a callback that
    /// re-enters `set` is the deadlock this type exists to prevent, and it
    /// cannot be waiting for a pointer it is itself about to stop using.
    fn wait_for_other_dispatches(&self) {
        let (lock, condvar) = &*self.in_flight;
        let me = std::thread::current().id();
        let mut in_flight = lock.lock().unwrap_or_else(|e| e.into_inner());
        while in_flight.threads.iter().any(|t| *t != me) {
            in_flight = condvar.wait(in_flight).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// One new item arrived. Invokes the executor callback if one is installed,
    /// otherwise records the item as unread so it can be replayed by [`set`].
    ///
    /// Runs on the zenoh delivery thread.
    ///
    /// [`set`]: Self::set
    pub fn notify_one(&self) {
        // Collect under the lock, and register the dispatch before releasing it
        // so `set` cannot conclude that no callback is in flight.
        let armed = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            match state.callback {
                Some(callback_fn) => {
                    let token = self.enter_dispatch();
                    Some((
                        callback_fn,
                        state.user_data as *const std::ffi::c_void,
                        token,
                    ))
                }
                None => {
                    state.unread += 1;
                    None
                }
            }
        };
        // ...guard is dropped, and only now do we call into the executor. The
        // token outlives the call and deregisters on the way out, panic or not.
        if let Some((callback_fn, user_data, _token)) = armed {
            invoke_user_callback!(self.site, unsafe { callback_fn(user_data, 1) });
        }
    }

    /// Install (or clear) the executor callback.
    ///
    /// Backs the `rmw_subscription_set_on_new_message_callback`,
    /// `rmw_service_set_on_new_request_callback` and
    /// `rmw_client_set_on_new_response_callback` entry points.
    ///
    /// Installing a callback replays any backlog accumulated by [`notify_one`]
    /// in a single call, which is what lets an executor that attaches after
    /// messages have already arrived — the common startup race — see them.
    ///
    /// # Blocking
    ///
    /// This returns only once every dispatch that captured the *outgoing*
    /// `user_data` has finished, so that a caller clearing the slot may then
    /// free what it pointed at. It therefore blocks for as long as an executor
    /// callback already running on another thread takes to return —
    /// `rmw_zenoh_cpp` has the same property, where the wait is on
    /// `event_mutex_` instead. A callback re-entering on its own thread never
    /// waits.
    ///
    /// [`notify_one`]: Self::notify_one
    pub fn set(&self, callback: Option<ExecCallbackFn>, user_data: *mut crate::c_void) {
        // Collect under the lock, registering the replay — if there is one —
        // before releasing it, for the same reason `notify_one` does: a
        // concurrent `set` must wait for this replay before freeing the pointer
        // being handed to it here.
        let backlog = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.callback = callback;
            state.user_data = user_data as usize;
            match callback {
                Some(callback_fn) if state.unread > 0 => {
                    let token = self.enter_dispatch();
                    Some((callback_fn, std::mem::take(&mut state.unread), token))
                }
                _ => None,
            }
        };

        // ...guard is dropped. The slot now holds the incoming `user_data`, so
        // any dispatch starting from here on uses the new pointer; the ones
        // still registered captured the old one. Wait for exactly those, which
        // is what lets the caller free it once we return. Our own replay
        // registration is on this thread and so is not waited for.
        self.wait_for_other_dispatches();

        if let Some((callback_fn, count, _token)) = backlog {
            tracing::debug!(
                "[{}] replaying {} unread item(s) to a newly installed callback",
                self.site,
                count
            );
            invoke_user_callback!(self.site, unsafe {
                callback_fn(user_data as usize as *const std::ffi::c_void, count)
            });
        }
    }

    /// Items that arrived with no callback installed. Test/introspection only.
    #[cfg(test)]
    fn unread(&self) -> usize {
        self.state.lock().map(|s| s.unread).unwrap_or(0)
    }
}

impl std::fmt::Debug for ExecCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecCallback")
            .field("site", &self.site)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// How long a re-entrant call is given before we declare it deadlocked.
    /// The fixed code returns in microseconds; the pre-fix code never returns.
    const DEADLOCK_TIMEOUT: Duration = Duration::from_secs(5);

    /// Stands in for the rclcpp executor: the object the `user_data` pointer
    /// actually points at, holding both the entity's slot and whatever
    /// bookkeeping the callback needs. Per-test, so the suite stays parallel.
    struct Executor {
        slot: ExecCallback,
        calls: AtomicUsize,
        last_count: AtomicUsize,
        /// How many more times the callback is allowed to re-enter. Bounded so
        /// a *correct* implementation terminates: with the fix, re-entry is
        /// legal and unbounded re-entry is unbounded recursion, which is a
        /// property of this callback and not of the code under test.
        reentries_left: AtomicUsize,
        /// Set once a callback is inside and about to block on `gate`.
        entered: AtomicBool,
        /// Holds a callback open so a concurrent `set` has something to wait for.
        gate: Gate,
        /// Ticket source for the ordering assertions.
        seq: AtomicUsize,
        /// Ticket taken as the blocking callback returns.
        callback_exit: AtomicUsize,
        /// Ticket taken as the concurrent `set` returns.
        set_returned: AtomicUsize,
    }

    /// A one-shot gate. Used to pin a callback in flight for as long as a test
    /// needs, without a sleep deciding the outcome.
    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        condvar: Condvar,
    }

    impl Gate {
        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.condvar.wait(open).unwrap();
            }
        }

        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.condvar.notify_all();
        }
    }

    /// No ticket taken yet.
    const NO_TICKET: usize = usize::MAX;

    impl Executor {
        fn new(site: &'static str, reentries: usize) -> Box<Self> {
            Box::new(Self {
                slot: ExecCallback::new(site),
                calls: AtomicUsize::new(0),
                last_count: AtomicUsize::new(0),
                reentries_left: AtomicUsize::new(reentries),
                entered: AtomicBool::new(false),
                gate: Gate::default(),
                seq: AtomicUsize::new(0),
                callback_exit: AtomicUsize::new(NO_TICKET),
                set_returned: AtomicUsize::new(NO_TICKET),
            })
        }

        fn ticket(&self) -> usize {
            self.seq.fetch_add(1, Ordering::SeqCst)
        }

        fn user_data(&self) -> *mut crate::c_void {
            self as *const Self as *mut crate::c_void
        }

        fn enter(&self, count: usize) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.last_count.store(count, Ordering::SeqCst);
            self.reentries_left
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        }
    }

    /// An executor callback that re-enters the rmw API on the same entity by
    /// clearing itself — what rclcpp does when an executor detaches from
    /// inside a notification.
    unsafe extern "C" fn reenters_by_clearing(user_data: *const std::ffi::c_void, count: usize) {
        let exec = unsafe { &*(user_data as *const Executor) };
        if exec.enter(count) {
            // Pre-fix, this blocks on a guard this very thread already holds.
            exec.slot.set(None, std::ptr::null_mut());
        }
    }

    /// An executor callback that re-enters by asking for a fresh notification,
    /// covering the delivery-thread entry point rather than the install one.
    unsafe extern "C" fn reenters_by_notifying(user_data: *const std::ffi::c_void, count: usize) {
        let exec = unsafe { &*(user_data as *const Executor) };
        if exec.enter(count) {
            exec.slot.notify_one();
        }
    }

    /// A callback that parks inside the dispatch until the test releases it,
    /// then takes a ticket on the way out. Lets a test observe whether a
    /// concurrent `set` returned before or after the callback finished.
    unsafe extern "C" fn blocks_until_released(user_data: *const std::ffi::c_void, _count: usize) {
        let exec = unsafe { &*(user_data as *const Executor) };
        exec.calls.fetch_add(1, Ordering::SeqCst);
        exec.entered.store(true, Ordering::SeqCst);
        exec.gate.wait();
        exec.callback_exit.store(exec.ticket(), Ordering::SeqCst);
    }

    /// A callback that does not re-enter.
    unsafe extern "C" fn passive(user_data: *const std::ffi::c_void, count: usize) {
        let exec = unsafe { &*(user_data as *const Executor) };
        exec.calls.fetch_add(1, Ordering::SeqCst);
        exec.last_count.store(count, Ordering::SeqCst);
    }

    /// Run `body` on a worker thread and fail if it has not finished within
    /// [`DEADLOCK_TIMEOUT`].
    ///
    /// A deadlocked worker stays blocked for the lifetime of the test binary.
    /// That is deliberate: it cannot be unblocked, and a non-main thread does
    /// not keep the process alive, so the run still terminates and reports.
    fn assert_completes<T: Send + 'static>(
        what: &str,
        body: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(body());
        });
        match rx.recv_timeout(DEADLOCK_TIMEOUT) {
            Ok(v) => v,
            Err(_) => panic!(
                "{what} did not return within {DEADLOCK_TIMEOUT:?}, so it is blocked. \
                 The causes this suite distinguishes: a guard on the state mutex was \
                 still live across the dispatch (the original defect); the in-flight \
                 wait counted this thread and is waiting on itself; or a finished \
                 dispatch failed to deregister and the wait can never be satisfied"
            ),
        }
    }

    // --- the deadlock detectors ---

    /// Site 1 (the worst): `rmw_subscription_set_on_new_message_callback`
    /// replaying a backlog. Pre-fix this held the `callback` guard *and* the
    /// `unread_count` guard across the invocation, so a callback that touched
    /// either self-deadlocked. This is the common startup race: messages
    /// arrive before the executor attaches.
    #[test]
    fn installing_a_callback_over_a_backlog_survives_reentry() {
        let (calls, count) = assert_completes("set() replaying a backlog", || {
            let exec = Executor::new("subscription", 1);
            exec.slot.notify_one();
            exec.slot.notify_one();
            exec.slot.set(Some(reenters_by_clearing), exec.user_data());
            (
                exec.calls.load(Ordering::SeqCst),
                exec.last_count.load(Ordering::SeqCst),
            )
        });
        assert_eq!(calls, 1, "backlog replays in exactly one call");
        assert_eq!(count, 2, "and reports both unread items");
    }

    /// Sites 2/3/4: the delivery-thread notifier, which pre-fix held the
    /// `callback` guard and the `callback_user_data` guard across the
    /// invocation.
    #[test]
    fn delivery_notification_survives_reentry_into_set() {
        let calls = assert_completes("notify_one() dispatching to the executor", || {
            let exec = Executor::new("service", 1);
            exec.slot.set(Some(reenters_by_clearing), exec.user_data());
            exec.slot.notify_one();
            exec.calls.load(Ordering::SeqCst)
        });
        assert_eq!(calls, 1);
    }

    /// A callback that re-enters the *same* entry point it was dispatched
    /// from. Pre-fix this is a self-deadlock on `callback`.
    #[test]
    fn delivery_notification_survives_reentry_into_itself() {
        let calls = assert_completes("notify_one() re-entered from its own callback", || {
            let exec = Executor::new("client", 1);
            exec.slot.set(Some(reenters_by_notifying), exec.user_data());
            exec.slot.notify_one();
            exec.calls.load(Ordering::SeqCst)
        });
        assert_eq!(calls, 2, "outer dispatch plus one re-entrant dispatch");
    }

    /// The tripwire is only meaningful if the guard really is released before
    /// dispatch. Assert that directly, rather than trusting an assertion that
    /// passes vacuously whenever the guard count is zero for the wrong reason.
    #[test]
    fn no_guard_is_live_when_the_callback_runs() {
        unsafe extern "C" fn record_live_guards(user_data: *const std::ffi::c_void, _c: usize) {
            let exec = unsafe { &*(user_data as *const Executor) };
            exec.last_count
                .store(hiroz::reentrancy::live_guards(), Ordering::SeqCst);
        }

        let exec = Executor::new("subscription", 0);
        exec.last_count.store(usize::MAX, Ordering::SeqCst);
        exec.slot.notify_one();
        exec.slot.set(Some(record_live_guards), exec.user_data());
        assert_eq!(
            exec.last_count.load(Ordering::SeqCst),
            0,
            "set() must dispatch the backlog with no guard live"
        );

        exec.last_count.store(usize::MAX, Ordering::SeqCst);
        exec.slot.notify_one();
        assert_eq!(
            exec.last_count.load(Ordering::SeqCst),
            0,
            "notify_one() must dispatch with no guard live"
        );
    }

    // --- the use-after-free detectors ---

    /// The lifetime guarantee `rmw_zenoh_cpp` gets from dispatching under
    /// `event_mutex_`: once `set(None, ..)` returns, no callback is in flight,
    /// so the caller may free what `user_data` pointed at.
    ///
    /// Collect-then-dispatch alone does not provide this. Without the in-flight
    /// wait, `set` takes the lock, clears the slot and returns while a delivery
    /// thread is still inside the callback holding the outgoing pointer — and
    /// the next thing rclcpp does is destroy the entity.
    #[test]
    fn set_waits_for_a_dispatch_in_flight_on_another_thread() {
        // Leaked on purpose. Dropping it here would assume exactly what is
        // under test — that it is safe to free once `set` has returned.
        let exec: &'static Executor = Box::leak(Executor::new("subscription", 0));
        exec.slot.set(Some(blocks_until_released), exec.user_data());

        let delivery = std::thread::spawn(move || exec.slot.notify_one());

        // Park until the callback is genuinely inside the dispatch.
        let start = Instant::now();
        while !exec.entered.load(Ordering::SeqCst) {
            assert!(
                start.elapsed() < DEADLOCK_TIMEOUT,
                "the callback never entered; the test cannot say anything"
            );
            std::thread::yield_now();
        }

        let clearer = std::thread::spawn(move || {
            exec.slot.set(None, std::ptr::null_mut());
            exec.set_returned.store(exec.ticket(), Ordering::SeqCst);
        });

        // The callback is still parked. A `set` that does not wait has already
        // returned by now; one that waits cannot have.
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            exec.set_returned.load(Ordering::SeqCst),
            NO_TICKET,
            "set() returned while a callback was still in flight on another \
             thread — rclcpp is now free to destroy the entity and free the \
             user_data that callback is still holding"
        );

        exec.gate.open();
        delivery.join().expect("delivery thread");
        clearer.join().expect("clearing thread");

        assert_eq!(
            exec.callback_exit.load(Ordering::SeqCst),
            0,
            "the callback should have taken the first ticket"
        );
        assert_eq!(
            exec.set_returned.load(Ordering::SeqCst),
            1,
            "and set() the second — it must return strictly after the callback"
        );
    }

    /// The other half of the same rule, and the reason the wait is scoped to
    /// *other* threads: a callback re-entering `set` on its own thread must not
    /// wait for itself. Waiting for all dispatches unconditionally would
    /// reinstate the deadlock this type exists to remove, in a new place.
    #[test]
    fn set_does_not_wait_for_a_dispatch_on_its_own_thread() {
        let calls = assert_completes("set() re-entered from its own dispatch", || {
            let exec = Executor::new("service", 1);
            exec.slot.set(Some(reenters_by_clearing), exec.user_data());
            // The callback runs on this thread and calls `set` from inside the
            // dispatch. If the wait counted this thread, it would block here.
            exec.slot.notify_one();
            exec.calls.load(Ordering::SeqCst)
        });
        assert_eq!(calls, 1);
    }

    /// An unwind through a live dispatch must not leave its registration
    /// behind: the wait in `set` would then never be satisfiable, turning the
    /// use-after-free into a permanent hang. Covers `DispatchToken`'s `Drop`.
    ///
    /// The unwind is raised directly rather than from a callback, because a
    /// panic *out of a callback* is not the reachable case: the callbacks are
    /// `extern "C"`, and Rust aborts rather than unwinding across that
    /// boundary — a panicking executor callback kills the process before any
    /// `Drop` runs. What can unwind here is Rust code on this side of the
    /// boundary, most notably `invoke_user_callback!`'s own debug assertion,
    /// which fires *before* the call.
    ///
    /// The panic is caught, so the run prints one line from the default hook.
    /// That noise is expected.
    #[test]
    fn an_unwind_through_a_dispatch_releases_its_registration() {
        let exec = Executor::new("client", 0);

        let unwound = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _token = exec.slot.enter_dispatch();
            panic!("something unwound mid-dispatch");
        }));
        assert!(unwound.is_err(), "the panic must still propagate");

        // If the registration leaked, this blocks forever: the wait is looking
        // for a dispatch on another thread that has already gone.
        assert_completes("set() after an unwind mid-dispatch", move || {
            exec.slot.set(None, std::ptr::null_mut());
        });
    }

    // --- semantics preserved from the pre-fix code ---

    #[test]
    fn items_arriving_with_no_callback_are_counted_not_dropped() {
        let exec = Executor::new("subscription", 0);
        exec.slot.notify_one();
        exec.slot.notify_one();
        exec.slot.notify_one();
        assert_eq!(exec.slot.unread(), 3);
        assert_eq!(exec.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn installing_a_callback_drains_the_backlog() {
        let exec = Executor::new("subscription", 0);
        exec.slot.notify_one();
        exec.slot.set(Some(passive), exec.user_data());
        assert_eq!(exec.slot.unread(), 0, "backlog is reset once replayed");
        assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn installing_a_callback_with_no_backlog_does_not_dispatch() {
        let exec = Executor::new("subscription", 0);
        exec.slot.set(Some(passive), exec.user_data());
        assert_eq!(exec.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_cleared_callback_goes_back_to_counting() {
        let exec = Executor::new("subscription", 0);
        exec.slot.set(Some(passive), exec.user_data());
        exec.slot.notify_one();
        assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
        exec.slot.set(None, std::ptr::null_mut());
        exec.slot.notify_one();
        assert_eq!(
            exec.calls.load(Ordering::SeqCst),
            1,
            "no dispatch once cleared"
        );
        assert_eq!(exec.slot.unread(), 1, "and the item is counted instead");
    }
}
