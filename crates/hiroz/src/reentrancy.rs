//! Debug-time enforcement of: **a user callback is never invoked while a hiroz
//! lock guard is live.**
//!
//! A callback invoked under a guard runs user code inside hiroz's critical
//! section. If it re-enters hiroz it re-acquires a non-reentrant lock on the
//! thread already holding it — a deterministic hang, not a race.
//!
//! No lint catches this. `clippy::significant_drop_in_scrutinee` targets guards
//! that are unnamed scrutinee temporaries; the shape here is
//! `if let Ok(cb) = holder.lock()`, which *binds* the guard. Measured on this
//! crate: zero hits, with the lint confirmed live against its own documented
//! trigger. Nor could a bespoke one do better — the guard lifetime spans a
//! dynamic dispatch through `Arc<dyn Fn>` or an opaque `extern "C" fn`.
//!
//! Zero cost in release: [`GuardCount`] is zero-sized, and it and
//! [`assert_no_guards_held`] compile to nothing without `debug_assertions`.
//! Tests and CI run in debug.
//!
//! Usage: declare locks on callback-reachable paths as [`TrackedMutex`] /
//! [`TrackedRwLock`], and route every user-code invocation through
//! [`invoke_user_callback!`].
//!
//! [`GuardCount`]: crate::reentrancy::GuardCount
//! [`assert_no_guards_held`]: crate::reentrancy::assert_no_guards_held
//! [`TrackedMutex`]: crate::reentrancy::TrackedMutex
//! [`TrackedRwLock`]: crate::reentrancy::TrackedRwLock

use std::sync::{LockResult, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(debug_assertions)]
thread_local! {
    /// How many tracked hiroz guards are live on this thread right now.
    static LIVE_GUARDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII counter embedded in every tracked guard.
///
/// The private field makes it unforgeable: as a fieldless unit struct, any code
/// naming it could `drop` one, decrementing the count to zero while a guard was
/// live and silently disarming [`assert_no_guards_held`].
///
/// `Drop` asserts non-zero before decrementing. `saturating_sub` alone prevents
/// the wrap but hides the desync, which is the failure mode this module exists
/// to remove.
#[derive(Debug)]
pub struct GuardCount(());

impl GuardCount {
    #[inline(always)]
    fn new() -> Self {
        #[cfg(debug_assertions)]
        LIVE_GUARDS.with(|n| n.set(n.get() + 1));
        Self(())
    }
}

impl Drop for GuardCount {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        LIVE_GUARDS.with(|n| {
            let live = n.get();
            // `|| panicking()`: panicking while unwinding aborts, which would
            // replace someone else's failure with this one.
            debug_assert!(
                live > 0 || std::thread::panicking(),
                "hiroz GuardCount underflow: guard released with the live count \
                 already 0. The counter has desynced from the guards it tracks, \
                 so `assert_no_guards_held` can no longer detect a callback \
                 invoked under a lock."
            );
            n.set(live.saturating_sub(1));
        });
    }
}

/// Number of tracked hiroz guards live on this thread. Always 0 in release.
#[inline(always)]
pub fn live_guards() -> usize {
    #[cfg(debug_assertions)]
    {
        LIVE_GUARDS.with(|n| n.get())
    }
    #[cfg(not(debug_assertions))]
    {
        0
    }
}

/// The payload of a re-entrancy violation panic.
///
/// [`local_bus`](crate::local_bus) contains subscriber panics so one bad
/// callback cannot censor its siblings. That containment must not swallow
/// *this* panic: it is the crate reporting its own contract violation, and a
/// nested delivery raises it from inside that `catch_unwind`.
///
/// It is a type rather than a message prefix because the alternative is
/// forgeable: a subscriber that panicked with a `String` beginning with the
/// prefix would be re-raised as though it were a violation, breaking the very
/// isolation guarantee the containment exists to provide. The private field
/// means no code outside this crate can construct one.
#[derive(Debug)]
#[non_exhaustive]
pub struct ReentrancyViolation {
    /// The operator-facing description.
    pub message: String,
}

impl std::fmt::Display for ReentrancyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}


/// Panics (debug only) if any tracked guard is live on this thread.
///
/// Call immediately before invoking user code. `site` is reproduced in the panic
/// message — when this fires, *which* callback was about to run is the useful
/// information, not the counter's backtrace.
#[inline(always)]
pub fn assert_no_guards_held(site: &str) {
    #[cfg(debug_assertions)]
    {
        let live = live_guards();
        if live != 0 {
            std::panic::panic_any(ReentrancyViolation {
                message: format!(
                    "hiroz re-entrancy rule violated at `{site}`: about to invoke a user \
                     callback with {live} lock guard(s) live on this thread. A callback \
                     that re-enters hiroz will deadlock if it touches a lock this thread \
                     holds. Fix: collect what you need into an owned value, drop every \
                     guard, then invoke the callback."
                ),
            });
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = site;
}

/// Assert the re-entrancy rule, then invoke user code.
///
/// ```ignore
/// invoke_user_callback!("EventsManager::set_callback backlog", callback(count));
/// ```
#[macro_export]
macro_rules! invoke_user_callback {
    ($site:expr, $call:expr) => {{
        $crate::reentrancy::assert_no_guards_held($site);
        $call
    }};
}

// ---------------------------------------------------------------------------
// Tracked lock types
// ---------------------------------------------------------------------------

/// A `std::sync::Mutex` whose guards are counted by [`live_guards`].
#[derive(Debug, Default)]
pub struct TrackedMutex<T>(Mutex<T>);

impl<T> TrackedMutex<T> {
    pub fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    pub fn lock(&self) -> LockResult<TrackedMutexGuard<'_, T>> {
        match self.0.lock() {
            Ok(inner) => Ok(TrackedMutexGuard {
                inner,
                _count: GuardCount::new(),
            }),
            Err(poisoned) => Err(std::sync::PoisonError::new(TrackedMutexGuard {
                inner: poisoned.into_inner(),
                _count: GuardCount::new(),
            })),
        }
    }
}

/// Guard for [`TrackedMutex`]. Field order matters: `inner` is declared first so
/// it is released before the counter decrements, never the other way round.
#[derive(Debug)]
pub struct TrackedMutexGuard<'a, T> {
    inner: MutexGuard<'a, T>,
    _count: GuardCount,
}

impl<T> std::ops::Deref for TrackedMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> std::ops::DerefMut for TrackedMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

/// A `std::sync::RwLock` whose guards are counted by [`live_guards`].
#[derive(Debug, Default)]
pub struct TrackedRwLock<T>(RwLock<T>);

impl<T> TrackedRwLock<T> {
    pub fn new(value: T) -> Self {
        Self(RwLock::new(value))
    }

    pub fn read(&self) -> LockResult<TrackedReadGuard<'_, T>> {
        match self.0.read() {
            Ok(inner) => Ok(TrackedReadGuard {
                inner,
                _count: GuardCount::new(),
            }),
            Err(p) => Err(std::sync::PoisonError::new(TrackedReadGuard {
                inner: p.into_inner(),
                _count: GuardCount::new(),
            })),
        }
    }

    pub fn write(&self) -> LockResult<TrackedWriteGuard<'_, T>> {
        match self.0.write() {
            Ok(inner) => Ok(TrackedWriteGuard {
                inner,
                _count: GuardCount::new(),
            }),
            Err(p) => Err(std::sync::PoisonError::new(TrackedWriteGuard {
                inner: p.into_inner(),
                _count: GuardCount::new(),
            })),
        }
    }
}

#[derive(Debug)]
pub struct TrackedReadGuard<'a, T> {
    inner: RwLockReadGuard<'a, T>,
    _count: GuardCount,
}

impl<T> std::ops::Deref for TrackedReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

#[derive(Debug)]
pub struct TrackedWriteGuard<'a, T> {
    inner: RwLockWriteGuard<'a, T>,
    _count: GuardCount,
}

impl<T> std::ops::Deref for TrackedWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> std::ops::DerefMut for TrackedWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_are_counted_and_released() {
        let m = TrackedMutex::new(1u32);
        assert_eq!(live_guards(), 0);
        {
            let g = m.lock().unwrap();
            assert_eq!(*g, 1);
            assert_eq!(live_guards(), 1);
            {
                let rw = TrackedRwLock::new(2u32);
                let _r = rw.read().unwrap();
                assert_eq!(live_guards(), 2);
            }
            assert_eq!(live_guards(), 1);
        }
        assert_eq!(live_guards(), 0);
    }

    #[test]
    fn assert_passes_with_no_guards() {
        assert_no_guards_held("test");
    }

    /// A tripwire that never fires is indistinguishable from a clean codebase.
    ///
    /// The payload is asserted by **type**, not by message. `should_panic` can
    /// only match a string, and matching the message is exactly what made the
    /// old classifier forgeable: a subscriber panicking with the same words
    /// would have been mistaken for a violation. Testing the type is both
    /// stronger and the thing the containment in `local_bus` actually keys on.
    #[test]
    fn assert_fires_while_a_guard_is_live() {
        let fired = std::panic::catch_unwind(|| {
            let m = TrackedMutex::new(0u32);
            let _g = m.lock().unwrap();
            assert_no_guards_held("deliberate violation");
        });

        #[cfg(debug_assertions)]
        {
            let payload = fired.expect_err("the tripwire did not fire while a guard was live");
            let violation = payload
                .downcast_ref::<ReentrancyViolation>()
                .expect("the panic must carry ReentrancyViolation, not a bare string");
            assert!(
                violation.message.contains("deliberate violation"),
                "the site must reach the operator: {}",
                violation.message
            );
        }
        // Compiled out in release, so nothing panics there.
        #[cfg(not(debug_assertions))]
        {
            assert!(fired.is_ok(), "the check is debug-only");
            assert_eq!(live_guards(), 0);
        }
    }

    /// The underflow assertion needs the same proof the tripwire gets. Only this
    /// module can mint a bare `GuardCount`, so only here can it be tested.
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "GuardCount underflow"))]
    fn underflow_is_not_silent() {
        assert_eq!(live_guards(), 0);
        drop(GuardCount(()));
    }

    #[test]
    fn a_poisoned_guard_is_still_counted() {
        let m = std::sync::Arc::new(TrackedMutex::new(0u32));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison it");
        })
        .join();

        let guard = m.lock();
        assert!(guard.is_err(), "expected the mutex to be poisoned");
        let recovered = guard.unwrap_or_else(|e| e.into_inner());
        assert_eq!(live_guards(), 1, "a recovered poisoned guard must count");
        drop(recovered);
        assert_eq!(live_guards(), 0);
    }
}
