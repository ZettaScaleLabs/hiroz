//! Safe goal state management with deadlock prevention.
//!
//! This module provides a `SafeGoalManager` that enforces synchronous-only
//! access to goal state, preventing accidental lock-across-await bugs.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{GoalId, GoalStatus, ZAction};

/// Thread-safe goal state manager with compile-time deadlock prevention.
///
/// The `SafeGoalManager` wraps the internal goal state in a way that
/// prevents holding locks across async operations. All access must go
/// through the `modify` method, which only accepts synchronous closures.
pub struct SafeGoalManager<A: ZAction> {
    inner: Mutex<GoalManagerInternal<A>>,
}

impl<A: ZAction> SafeGoalManager<A> {
    pub fn new(result_timeout: Duration, goal_timeout: Option<Duration>) -> Self {
        Self {
            inner: Mutex::new(GoalManagerInternal {
                goals: HashMap::new(),
                result_timeout,
                goal_timeout,
                result_futures: HashMap::new(),
            }),
        }
    }

    /// The ONLY way to access goal state.
    ///
    /// This method enforces that all state access happens in a synchronous closure,
    /// preventing accidental lock-across-await bugs. The lock is automatically
    /// released when the closure returns.
    ///
    /// # Arguments
    ///
    /// * `f` - A synchronous closure that can read/modify the goal state
    ///
    /// # Returns
    ///
    /// The value returned by the closure
    pub fn modify<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut GoalManagerInternal<A>) -> R,
    {
        let mut guard = self.inner.lock().expect("Lock poisoned");
        f(&mut guard)
    }

    /// Read-only access to goal state.
    pub fn read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&GoalManagerInternal<A>) -> R,
    {
        let guard = self.inner.lock().expect("Lock poisoned");
        f(&guard)
    }
}

/// Type alias for result future senders to reduce complexity.
type ResultSenders<A> = Vec<tokio::sync::oneshot::Sender<(<A as ZAction>::Result, GoalStatus)>>;

/// Internal goal state storage.
///
/// This is kept private and only accessible through `SafeGoalManager::modify`.
pub struct GoalManagerInternal<A: ZAction> {
    pub goals: HashMap<GoalId, ServerGoalState<A>>,
    pub result_timeout: Duration,
    pub goal_timeout: Option<Duration>,
    pub result_futures: HashMap<GoalId, ResultSenders<A>>,
}

/// Server-side state for an action goal.
pub enum ServerGoalState<A: ZAction> {
    Accepted {
        goal: A::Goal,
        timestamp: Instant,
        expires_at: Option<Instant>,
    },
    Executing {
        goal: A::Goal,
        cancel_flag: Arc<AtomicBool>,
        expires_at: Option<Instant>,
    },
    Canceling {
        goal: A::Goal,
    },
    Terminated {
        result: A::Result,
        status: GoalStatus,
        timestamp: Instant,
        expires_at: Option<Instant>,
    },
}
