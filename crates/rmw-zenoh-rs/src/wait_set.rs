use crate::ros::*;
use crate::traits::{BorrowImpl, OwnData, Waitable};
use std::sync::Arc;

/// Wait set implementation for RMW
pub struct WaitSetImpl {
    pub subscriptions: Vec<*mut rmw_subscription_impl_t>,
    pub guard_conditions: Vec<*mut rmw_guard_condition_impl_t>,
    pub services: Vec<*mut rmw_service_impl_t>,
    pub clients: Vec<*mut rmw_client_impl_t>,
    pub events: Vec<*mut rmw_event_t>,
    pub notifier: Arc<crate::utils::Notifier>,
}

impl WaitSetImpl {
    pub fn new(max_conditions: usize, notifier: Arc<crate::utils::Notifier>) -> Self {
        Self {
            subscriptions: Vec::with_capacity(max_conditions),
            guard_conditions: Vec::with_capacity(max_conditions),
            services: Vec::with_capacity(max_conditions),
            clients: Vec::with_capacity(max_conditions),
            events: Vec::with_capacity(max_conditions),
            notifier,
        }
    }

    pub fn wait(&self, timeout: &rmw_time_t) -> bool {
        use std::time::{Duration, Instant};

        // If timeout is zero, check ready immediately and return
        if timeout.sec == 0 && timeout.nsec == 0 {
            return self.check_ready();
        }

        // Compute an absolute deadline once so spurious wakeups don't extend
        // the total wait beyond the requested timeout.  This is critical for
        // timer support: rcl passes timeout = time-until-next-timer-fires, and
        // relies on rmw_wait returning RMW_RET_TIMEOUT after exactly that
        // duration so it can mark the timer as expired.
        let deadline = if timeout.sec == u64::MAX {
            None // Infinite wait
        } else {
            let dur = Duration::from_secs(timeout.sec) + Duration::from_nanos(timeout.nsec);
            Some(Instant::now() + dur)
        };

        // Use the notifier's condition variable for efficient waiting.
        // Notifier::notify_all() holds this mutex, so the check-then-wait here is
        // race-free: either we see the data in check_ready() before waiting, or
        // notify_all() fires after wait_for atomically releases the mutex and the
        // thread is in the waiting queue — no lost wakeups.
        let mut mutex_guard = self.notifier.mutex.lock();

        loop {
            // Check readiness while holding the lock before waiting.
            // This handles data that arrived before rmw_wait was called.
            if self.check_ready() {
                return true;
            }

            if let Some(dl) = deadline {
                let now = Instant::now();
                if now >= dl {
                    // Deadline already passed — report timeout
                    return false;
                }
                let remaining = dl - now;
                let wait_result = self.notifier.cv.wait_for(&mut mutex_guard, remaining);

                // After wait (notification or timeout), check if anything is ready
                if self.check_ready() {
                    return true;
                }

                // Nothing ready
                if wait_result.timed_out() {
                    return false;
                }

                // Spurious wakeup - loop back and recompute remaining time
            } else {
                // Infinite wait
                self.notifier.cv.wait(&mut mutex_guard);
                // Loop back to check_ready at the top
            }
        }
    }

    fn check_ready(&self) -> bool {
        // Check subscriptions
        for sub_impl_ptr in &self.subscriptions {
            if sub_impl_ptr.is_null() {
                continue;
            }
            unsafe {
                let sub_impl =
                    &*((*sub_impl_ptr) as *const _ as *const crate::pubsub::SubscriptionImpl);
                if sub_impl.is_ready() {
                    return true;
                }
            }
        }

        // Check guard conditions
        for gc_impl_ptr in &self.guard_conditions {
            if gc_impl_ptr.is_null() {
                continue;
            }
            unsafe {
                let gc_impl = &*(*gc_impl_ptr as *const _
                    as *const crate::guard_condition::GuardConditionImpl);
                if gc_impl.is_ready() {
                    return true;
                }
            }
        }

        // Check services
        for srv_impl_ptr in &self.services {
            if srv_impl_ptr.is_null() {
                continue;
            }
            unsafe {
                let srv_impl = &*(*srv_impl_ptr as *const _ as *const crate::service::ServiceImpl);
                if srv_impl.is_ready() {
                    return true;
                }
            }
        }

        // Check clients
        for cli_impl_ptr in &self.clients {
            if cli_impl_ptr.is_null() {
                continue;
            }
            unsafe {
                let cli_impl = &*(*cli_impl_ptr as *const _ as *const crate::service::ClientImpl);
                if cli_impl.is_ready() {
                    return true;
                }
            }
        }

        // Check events
        for event_ptr in &self.events {
            if event_ptr.is_null() {
                continue;
            }
            unsafe {
                let event = &*(*event_ptr);
                if !event.data.is_null() {
                    let event_handle = &*(event.data as *const hiroz::event::RmEventHandle);
                    if event_handle.is_ready() {
                        return true;
                    }
                }
            }
        }

        false
    }
}

rmw_impl_has_data_ptr!(rmw_wait_set_t, rmw_wait_set_impl_t, WaitSetImpl);

// RMW Wait Set Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_create_wait_set(
    context: *mut rmw_context_t,
    max_conditions: usize,
) -> *mut rmw_wait_set_t {
    if context.is_null() {
        return std::ptr::null_mut();
    }

    // Check context implementation_identifier
    unsafe {
        if !crate::context::check_impl_id((*context).implementation_identifier) {
            return std::ptr::null_mut();
        }
    }

    // Get the shared notifier from the context
    let context_impl = match context.borrow_impl() {
        Ok(impl_) => impl_,
        Err(_) => return std::ptr::null_mut(),
    };
    let notifier = context_impl.share_notifier();

    let wait_set_impl = WaitSetImpl::new(max_conditions, notifier);
    let wait_set = Box::new(rmw_wait_set_t {
        implementation_identifier: crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const _,
        guard_conditions: std::ptr::null_mut(),
        data: std::ptr::null_mut(),
    });

    let wait_set_ptr = Box::into_raw(wait_set);
    unsafe {
        (*wait_set_ptr).data = Box::into_raw(Box::new(wait_set_impl)) as *mut _;
    }

    wait_set_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_destroy_wait_set(wait_set: *mut rmw_wait_set_t) -> rmw_ret_t {
    if wait_set.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Check implementation_identifier
    unsafe {
        let ret = crate::context::check_impl_id_ret((*wait_set).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    drop(unsafe { Box::from_raw(wait_set) });
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_wait(
    subscriptions: *mut rmw_subscriptions_t,
    guard_conditions: *mut rmw_guard_conditions_t,
    services: *mut rmw_services_t,
    clients: *mut rmw_clients_t,
    events: *mut rmw_events_t,
    wait_set: *mut rmw_wait_set_t,
    wait_timeout: *const rmw_time_t,
) -> rmw_ret_t {
    if wait_set.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*wait_set).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let wait_set_impl = match wait_set.borrow_mut_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Clear the wait set
    wait_set_impl.subscriptions.clear();
    wait_set_impl.guard_conditions.clear();
    wait_set_impl.services.clear();
    wait_set_impl.clients.clear();
    wait_set_impl.events.clear();

    // Add subscriptions to wait set
    if !subscriptions.is_null() {
        let sub_array = unsafe { &*subscriptions };
        for i in 0..sub_array.subscriber_count {
            // subscribers is *mut *mut c_void, so subscribers.add(i) gives *mut *mut c_void
            // We dereference once to get *mut c_void, which points to rmw_subscription_impl_t
            let sub_impl = unsafe { *sub_array.subscribers.add(i) as *mut rmw_subscription_impl_t };
            if !sub_impl.is_null() {
                wait_set_impl.subscriptions.push(sub_impl);
            }
        }
    }

    // Add guard conditions
    if !guard_conditions.is_null() {
        let gc_array = unsafe { &*guard_conditions };
        for i in 0..gc_array.guard_condition_count {
            let gc_impl =
                unsafe { *gc_array.guard_conditions.add(i) as *mut rmw_guard_condition_impl_t };
            if !gc_impl.is_null() {
                wait_set_impl.guard_conditions.push(gc_impl);
            }
        }
    }

    // Add services
    if !services.is_null() {
        let srv_array = unsafe { &*services };
        for i in 0..srv_array.service_count {
            let srv_impl = unsafe { *srv_array.services.add(i) as *mut rmw_service_impl_t };
            if !srv_impl.is_null() {
                wait_set_impl.services.push(srv_impl);
            }
        }
    }

    // Add clients
    if !clients.is_null() {
        let cli_array = unsafe { &*clients };
        for i in 0..cli_array.client_count {
            let cli_impl = unsafe { *cli_array.clients.add(i) as *mut rmw_client_impl_t };
            if !cli_impl.is_null() {
                wait_set_impl.clients.push(cli_impl);
            }
        }
    }

    // Add events
    if !events.is_null() {
        let event_array = unsafe { &*events };
        for i in 0..event_array.event_count {
            let event_ptr = unsafe { *event_array.events.add(i) as *mut rmw_event_t };
            if !event_ptr.is_null() {
                wait_set_impl.events.push(event_ptr);
            }
        }
    }

    // Wait for ready entities
    let timeout = if wait_timeout.is_null() {
        rmw_time_t {
            sec: u64::MAX,
            nsec: 0,
        }
    } else {
        unsafe { *wait_timeout }
    };

    let ready = wait_set_impl.wait(&timeout);

    if ready {
        // Update arrays - NULL items in place (do NOT compact!)
        // RCL relies on array indices matching
        if !subscriptions.is_null() {
            let sub_array = unsafe { &mut *subscriptions };
            for i in 0..sub_array.subscriber_count {
                let sub_impl_ptr =
                    unsafe { *sub_array.subscribers.add(i) as *mut rmw_subscription_impl_t };
                if !sub_impl_ptr.is_null() {
                    unsafe {
                        let sub_impl =
                            &*(sub_impl_ptr as *const _ as *const crate::pubsub::SubscriptionImpl);
                        if !sub_impl.is_ready() {
                            // Not ready - NULL in place
                            *sub_array.subscribers.add(i) = std::ptr::null_mut();
                        }
                    }
                }
            }
        }

        // Similar for services - NULL in place
        if !services.is_null() {
            let srv_array = unsafe { &mut *services };
            for i in 0..srv_array.service_count {
                let srv_impl_ptr = unsafe { *srv_array.services.add(i) as *mut rmw_service_impl_t };
                if !srv_impl_ptr.is_null() {
                    unsafe {
                        let srv_impl =
                            &*(srv_impl_ptr as *const _ as *const crate::service::ServiceImpl);
                        if !srv_impl.is_ready() {
                            *srv_array.services.add(i) = std::ptr::null_mut();
                        }
                    }
                }
            }
        }

        // Similar for clients - NULL in place
        if !clients.is_null() {
            let cli_array = unsafe { &mut *clients };
            for i in 0..cli_array.client_count {
                let cli_impl_ptr = unsafe { *cli_array.clients.add(i) as *mut rmw_client_impl_t };
                if !cli_impl_ptr.is_null() {
                    unsafe {
                        let cli_impl =
                            &*(cli_impl_ptr as *const _ as *const crate::service::ClientImpl);
                        if !cli_impl.is_ready() {
                            *cli_array.clients.add(i) = std::ptr::null_mut();
                        }
                    }
                }
            }
        }

        // Similar for guard conditions
        // IMPORTANT: NULL items in place, do NOT compact the array!
        // RCL relies on array indices matching between calls to rcl_wait_set_add_* and rmw_wait results
        if !guard_conditions.is_null() {
            let gc_array = unsafe { &mut *guard_conditions };
            for i in 0..gc_array.guard_condition_count {
                let gc_impl_ptr =
                    unsafe { *gc_array.guard_conditions.add(i) as *mut rmw_guard_condition_impl_t };
                if !gc_impl_ptr.is_null() {
                    unsafe {
                        let gc_impl =
                            &mut *(gc_impl_ptr as *mut crate::guard_condition::GuardConditionImpl);
                        if !gc_impl.is_ready() {
                            // Not ready - set to NULL in place
                            *gc_array.guard_conditions.add(i) = std::ptr::null_mut();
                        } else {
                            // Reset the guard condition after it's been detected as ready
                            // This prevents it from staying triggered forever
                            gc_impl.reset();
                        }
                    }
                }
            }
        }

        // Similar for events - NULL in place
        if !events.is_null() {
            let event_array = unsafe { &mut *events };
            for i in 0..event_array.event_count {
                let event_ptr = unsafe { *event_array.events.add(i) as *mut rmw_event_t };
                if !event_ptr.is_null() {
                    unsafe {
                        let event = &*event_ptr;
                        // Check if event data is ready
                        let is_ready = if !event.data.is_null() {
                            let event_handle = &*(event.data as *const hiroz::event::RmEventHandle);
                            event_handle.is_ready()
                        } else {
                            false
                        };

                        if !is_ready {
                            *event_array.events.add(i) = std::ptr::null_mut();
                        }
                    }
                }
            }
        }

        RMW_RET_OK as _
    } else {
        // On timeout, NULL out all entities per RMW contract: nothing is ready.
        // Without this, rcl reads the stale non-NULL pointers back as "triggered"
        // (rcl/wait.c: `is_ready = guard_conditions[i] != NULL` runs unconditionally).
        // This corrupts ThreadSafeSynchronization::sync_wait's guard-condition check:
        // it sees was_interrupted_by_this_class=true and loops forever instead of
        // returning WaitResultKind::Ready when a timer fires.
        if !subscriptions.is_null() {
            let sub_array = unsafe { &mut *subscriptions };
            for i in 0..sub_array.subscriber_count {
                unsafe {
                    *sub_array.subscribers.add(i) = std::ptr::null_mut();
                }
            }
        }
        if !guard_conditions.is_null() {
            let gc_array = unsafe { &mut *guard_conditions };
            for i in 0..gc_array.guard_condition_count {
                unsafe {
                    *gc_array.guard_conditions.add(i) = std::ptr::null_mut();
                }
            }
        }
        if !services.is_null() {
            let srv_array = unsafe { &mut *services };
            for i in 0..srv_array.service_count {
                unsafe {
                    *srv_array.services.add(i) = std::ptr::null_mut();
                }
            }
        }
        if !clients.is_null() {
            let cli_array = unsafe { &mut *clients };
            for i in 0..cli_array.client_count {
                unsafe {
                    *cli_array.clients.add(i) = std::ptr::null_mut();
                }
            }
        }
        if !events.is_null() {
            let event_array = unsafe { &mut *events };
            for i in 0..event_array.event_count {
                unsafe {
                    *event_array.events.add(i) = std::ptr::null_mut();
                }
            }
        }
        RMW_RET_TIMEOUT as _
    }
}
