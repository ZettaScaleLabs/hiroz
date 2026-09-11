//! A tiny compatibility shim, feature-gated by `lock-tripwire-guard`.
//!
//! `GuardedMutex<T>` is `lock_tripwire::TrackedMutex<T>` when the feature is
//! on and a plain `std::sync::Mutex<T>` when it's off -- same `::new`, so no
//! call site needs a second, feature-gated construction path. `.lock_guarded
//! (SITE)` is the one method both sides implement, so a call site reads
//! identically either way. `guarded_call!` wraps a call-out that happens
//! while a `GuardedMutex` guard is held on the same thread; with the feature
//! on this is `lock_tripwire::invoke_user_callback!`, with it off it is the
//! call, unwrapped.
//!
//! The plain fix already removed the callback-mutex-vs-GIL deadlock this
//! crate's notify/setter functions used to have: no lock is held across a
//! call-out anymore. What this feature adds is a regression guard -- if a
//! future change ever reintroduces holding one of these locks across a
//! call-out, `guarded_call!` fires immediately and names the site, instead
//! of the change silently reintroducing a hang in production.

#[cfg(feature = "lock-tripwire-guard")]
pub type GuardedMutex<T> = lock_tripwire::TrackedMutex<T>;
#[cfg(not(feature = "lock-tripwire-guard"))]
pub type GuardedMutex<T> = std::sync::Mutex<T>;

#[cfg(feature = "lock-tripwire-guard")]
pub type GuardedMutexGuard<'a, T> = lock_tripwire::TrackedMutexGuard<'a, T>;
#[cfg(not(feature = "lock-tripwire-guard"))]
pub type GuardedMutexGuard<'a, T> = std::sync::MutexGuard<'a, T>;

pub trait LockGuarded<T> {
    fn lock_guarded(&self, site: &'static str) -> std::sync::LockResult<GuardedMutexGuard<'_, T>>;
}

#[cfg(feature = "lock-tripwire-guard")]
impl<T> LockGuarded<T> for GuardedMutex<T> {
    #[inline]
    fn lock_guarded(&self, site: &'static str) -> std::sync::LockResult<GuardedMutexGuard<'_, T>> {
        self.lock_at(site, lock_tripwire::LockKind::State)
    }
}

#[cfg(not(feature = "lock-tripwire-guard"))]
impl<T> LockGuarded<T> for GuardedMutex<T> {
    #[inline]
    fn lock_guarded(&self, _site: &'static str) -> std::sync::LockResult<GuardedMutexGuard<'_, T>> {
        self.lock()
    }
}

#[cfg(feature = "lock-tripwire-guard")]
#[macro_export]
macro_rules! guarded_call {
    ($site:expr, $call:expr) => {
        ::lock_tripwire::invoke_user_callback!($site, $call)
    };
}

#[cfg(not(feature = "lock-tripwire-guard"))]
#[macro_export]
macro_rules! guarded_call {
    ($site:expr, $call:expr) => {
        $call
    };
}
