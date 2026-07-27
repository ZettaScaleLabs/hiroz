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
//! # Enforcement
//!
//! The mutex is a [`hiroz::reentrancy::TrackedMutex`], so its guards are counted
//! on the current thread, and both call sites go through
//! [`hiroz::invoke_user_callback!`], which asserts the count is zero before
//! dispatching. In debug builds a reintroduction of the defect panics with the
//! site name instead of hanging; in release both the counter and the assertion
//! compile to nothing.

use std::sync::Arc;

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

/// Shared executor-notification state for one rmw entity.
///
/// Cloning shares the underlying slot; the delivery thread and the rmw API
/// entry points hold clones of the same `ExecCallback`.
#[derive(Clone)]
pub struct ExecCallback {
    state: Arc<TrackedMutex<State>>,
    /// Entity kind, reproduced in the re-entrancy panic message.
    site: &'static str,
}

impl ExecCallback {
    /// `site` names the entity kind ("subscription", "service", "client") and
    /// appears in the re-entrancy assertion message.
    pub fn new(site: &'static str) -> Self {
        Self {
            state: Arc::new(TrackedMutex::new(State::default())),
            site,
        }
    }

    /// One new item arrived. Invokes the executor callback if one is installed,
    /// otherwise records the item as unread so it can be replayed by [`set`].
    ///
    /// Runs on the zenoh delivery thread.
    ///
    /// [`set`]: Self::set
    pub fn notify_one(&self) {
        // Collect under the lock...
        let armed = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            match state.callback {
                Some(callback_fn) => {
                    Some((callback_fn, state.user_data as *const std::ffi::c_void))
                }
                None => {
                    state.unread += 1;
                    None
                }
            }
        };
        // ...guard is dropped, and only now do we call into the executor.
        if let Some((callback_fn, user_data)) = armed {
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
    /// [`notify_one`]: Self::notify_one
    pub fn set(&self, callback: Option<ExecCallbackFn>, user_data: *mut crate::c_void) {
        // Collect under the lock...
        let backlog = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.callback = callback;
            state.user_data = user_data as usize;
            match callback {
                Some(callback_fn) if state.unread > 0 => {
                    Some((callback_fn, std::mem::take(&mut state.unread)))
                }
                _ => None,
            }
        };
        // ...guard is dropped, and only now do we call into the executor.
        if let Some((callback_fn, count)) = backlog {
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

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
    }

    impl Executor {
        fn new(site: &'static str, reentries: usize) -> Box<Self> {
            Box::new(Self {
                slot: ExecCallback::new(site),
                calls: AtomicUsize::new(0),
                last_count: AtomicUsize::new(0),
                reentries_left: AtomicUsize::new(reentries),
            })
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
                "{what} did not return within {DEADLOCK_TIMEOUT:?} — the executor \
                 callback was invoked while a guard on the same mutex was live, so \
                 the re-entrant call is blocked on this thread's own lock"
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
