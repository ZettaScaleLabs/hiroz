use crate::rmw_impl_has_data_ptr;
use crate::ros::*;
use crate::traits::*;
use crate::utils::Notifier;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The triggerable state of a guard condition, separated from the C handle.
///
/// Behind an `Arc` so it can outlive `rmw_destroy_guard_condition`: hiroz's
/// graph-event manager registers a clone and triggers it **after** dropping its
/// registry lock, so a racing `rmw_destroy_node` would otherwise free the target
/// mid-call. The C handle holds one reference and the registry another.
///
/// `triggered` is atomic because triggering no longer happens under any lock.
#[derive(Debug, Default)]
pub struct GuardConditionState {
    pub(crate) notifier: Option<Arc<Notifier>>,
    pub(crate) triggered: AtomicBool,
}

impl GuardConditionState {
    pub(crate) fn fire(&self) -> Result<(), ()> {
        let notifier = self.notifier.as_ref().ok_or(())?;
        self.triggered.store(true, Ordering::SeqCst);
        notifier.notify_all();
        Ok(())
    }

    pub(crate) fn reset(&self) {
        self.triggered.store(false, Ordering::SeqCst);
    }

    /// Consume the triggered flag, returning what it was.
    ///
    /// One atomic op, so a `trigger()` racing this either wins or is preserved.
    /// `is_triggered()` then `reset()` is two ops and silently drops a trigger
    /// landing between them: the waiter reports this wake, and the next
    /// `rmw_wait` blocks until timeout.
    pub(crate) fn take_triggered(&self) -> bool {
        self.triggered.swap(false, Ordering::SeqCst)
    }

    pub(crate) fn is_triggered(&self) -> bool {
        self.triggered.load(Ordering::SeqCst)
    }
}

impl hiroz::event::GraphGuardCondition for GuardConditionState {
    fn trigger(&self) {
        // A guard condition with no notifier cannot wake anyone; that is not an
        // error worth propagating across the registry.
        let _ = self.fire();
    }
}

/// Guard condition implementation for RMW
#[derive(Debug, Default)]
pub struct GuardConditionImpl {
    pub(crate) state: Arc<GuardConditionState>,
}

impl GuardConditionImpl {
    // `&self`, not `&mut self`: `rmw_wait` holds shared references while
    // scanning the wait set, and a zenoh thread can trigger concurrently.
    // Handing out `&mut` to an object another thread holds a `&` to is UB
    // regardless of field types — the atomic defines the *data race* but says
    // nothing about the aliasing. All mutation goes through the atomics.
    pub(crate) fn trigger(&self) -> Result<(), ()> {
        self.state.fire()
    }

    pub fn reset(&self) {
        self.state.reset();
    }

    /// Consume the triggered flag. See [`GuardConditionState::take_triggered`].
    pub(crate) fn take_triggered(&self) -> bool {
        self.state.take_triggered()
    }

    /// A shared handle to this guard condition's state, for registration with
    /// hiroz's graph-event manager.
    pub(crate) fn share_state(&self) -> Arc<GuardConditionState> {
        self.state.clone()
    }
}

impl crate::traits::Waitable for GuardConditionImpl {
    fn is_ready(&self) -> bool {
        self.state.is_triggered()
    }
}

rmw_impl_has_data_ptr!(
    rmw_guard_condition_t,
    rmw_guard_condition_impl_t,
    GuardConditionImpl
);

// RMW Guard Condition Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_create_guard_condition(
    context: *mut rmw_context_t,
) -> *mut rmw_guard_condition_t {
    if context.is_null() {
        return std::ptr::null_mut();
    }

    let context_impl = match context.borrow_impl() {
        Ok(impl_) => impl_,
        Err(_) => return std::ptr::null_mut(),
    };

    let notifier = Some(context_impl.share_notifier());
    let gc_impl = GuardConditionImpl {
        state: Arc::new(GuardConditionState {
            notifier,
            triggered: AtomicBool::new(false),
        }),
    };
    let gc = Box::new(rmw_guard_condition_t {
        implementation_identifier: crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const _,
        data: std::ptr::null_mut(),
        context,
    });

    let gc_ptr = Box::into_raw(gc);
    gc_ptr.assign_data(gc_impl).unwrap_or(());

    gc_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_destroy_guard_condition(
    guard_condition: *mut rmw_guard_condition_t,
) -> rmw_ret_t {
    if guard_condition.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Drop the implementation data
    let _ = guard_condition.own_data();

    drop(unsafe { Box::from_raw(guard_condition) });
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_trigger_guard_condition(
    guard_condition: *const rmw_guard_condition_t,
) -> rmw_ret_t {
    if guard_condition.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Immutable borrow: see the note on `GuardConditionImpl::trigger`.
    if let Ok(gc_impl) = (guard_condition as *mut rmw_guard_condition_t).borrow_data() {
        let _ = gc_impl.trigger();
    }

    RMW_RET_OK as _
}
