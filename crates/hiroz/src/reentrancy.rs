//! Debug-time enforcement of hiroz's one hard rule about user callbacks:
//!
//! > **A user callback is never invoked while a hiroz lock guard is live.**
//!
//! Every re-entrancy deadlock hiroz has had was one violation of that sentence.
//! A callback invoked under a guard is user code running inside hiroz's own
//! critical section, and the first thing such code usually does is call back
//! into hiroz — re-acquiring a non-reentrant `Mutex`/`RwLock` on the thread that
//! already holds it. There is no race to lose; it is a deterministic hang.
//!
//! The rule was previously enforced by review. That does not scale, and it
//! demonstrably leaked: a mechanical sweep of this workspace found further
//! instances in `rmw-zenoh-rs` after five had already been fixed by hand.
//!
//! # Why a runtime tripwire and not a lint
//!
//! The obvious candidate, `clippy::significant_drop_in_scrutinee`, was tried and
//! **does not fire on this code at all**. It targets guards that are unnamed
//! temporaries in a `match`/`if let` scrutinee; hiroz's (and rmw's) shape is
//! `if let Ok(cb) = holder.lock()`, where the guard is *bound* to a name and so
//! is not a scrutinee temporary. Verified: `cargo clippy -p rmw-zenoh-rs -W
//! clippy::significant_drop_in_scrutinee` reports zero hits on code containing
//! five genuine instances of the defect.
//!
//! A bespoke lint fares no better. The defect is a *dynamic* guard lifetime
//! spanning a *dynamic* dispatch through `Arc<dyn Fn>` or an `extern "C" fn`
//! pointer. Static analysis cannot see through either, and the FFI pointers are
//! opaque by construction.
//!
//! A counter can see both, trivially.
//!
//! # Cost
//!
//! Zero in release. [`GuardCount`](crate::reentrancy::GuardCount) is a
//! zero-sized newtype whose constructor and `Drop` compile to nothing without
//! `debug_assertions`, and
//! [`assert_no_guards_held`](crate::reentrancy::assert_no_guards_held)
//! expands to nothing. Tests and CI run in debug, which is where the assertion
//! is wanted.
//!
//! # Using it
//!
//! Lock declarations on a user-callback-reachable path use
//! [`TrackedMutex`](crate::reentrancy::TrackedMutex) /
//! [`TrackedRwLock`](crate::reentrancy::TrackedRwLock) instead of the
//! `std::sync` originals. Every site that invokes user code calls
//! [`assert_no_guards_held`](crate::reentrancy::assert_no_guards_held) first — in practice via
//! [`invoke_user_callback!`], which does both in one line and names the site in
//! the panic message.

use std::sync::{LockResult, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(debug_assertions)]
thread_local! {
    /// How many tracked hiroz guards are live on this thread right now.
    static LIVE_GUARDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII counter embedded in every tracked guard.
///
/// The private field is what makes the counter trustworthy. As a fieldless unit
/// struct this was constructible — and therefore *droppable* — by any code that
/// could name it, and `Drop` decrements the thread-local. A stray
/// `drop(GuardCount)` while a tracked guard was live would take the count to
/// zero, `assert_no_guards_held` would pass, and a genuine callback-under-lock
/// would go unreported. `saturating_sub` guaranteed that desync was silent.
/// Only this module can mint one now, so the count can only be moved by
/// acquiring and releasing a real guard.
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
        LIVE_GUARDS.with(|n| n.set(n.get().saturating_sub(1)));
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

/// Panics (debug builds only) if any tracked hiroz guard is live on this thread.
///
/// Call immediately before invoking user code. `site` names the call site and is
/// reproduced in the panic message, because the useful information when this
/// fires is *which* callback was about to run, not the backtrace of the counter.
#[inline(always)]
pub fn assert_no_guards_held(site: &str) {
    #[cfg(debug_assertions)]
    {
        let live = live_guards();
        assert!(
            live == 0,
            "hiroz re-entrancy rule violated at `{site}`: about to invoke a user \
             callback with {live} hiroz lock guard(s) still live on this thread. \
             A callback that re-enters hiroz here will deadlock. Collect what you \
             need under the lock, drop the guard, then call — see \
             `GraphEventManager::trigger_graph_change` for the pattern."
        );
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

    /// The detector must actually detect. Without this, a tripwire that never
    /// fires is indistinguishable from a codebase with no defects.
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "re-entrancy rule violated"))]
    fn assert_fires_while_a_guard_is_live() {
        let m = TrackedMutex::new(0u32);
        let _g = m.lock().unwrap();
        assert_no_guards_held("deliberate violation");
        // In release the assertion is compiled out, so the test must not be
        // expected to panic there.
        #[cfg(not(debug_assertions))]
        assert_eq!(live_guards(), 0);
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
