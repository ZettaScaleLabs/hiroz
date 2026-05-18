use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::node::NodeImpl;
use crate::rmw_impl_has_impl_ptr;
use crate::ros::*;
use crate::traits::*;
use crate::utils::Notifier;
use hiroz::Builder;

/// Check implementation_identifier and return the appropriate error code.
/// Returns RMW_RET_OK if it matches, RMW_RET_INVALID_ARGUMENT if NULL,
/// or RMW_RET_INCORRECT_RMW_IMPLEMENTATION if wrong.
pub fn check_impl_id_ret(id: *const std::os::raw::c_char) -> rmw_ret_t {
    if id.is_null() {
        RMW_RET_INVALID_ARGUMENT as _
    } else if id == crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const std::os::raw::c_char {
        RMW_RET_OK as _
    } else {
        RMW_RET_INCORRECT_RMW_IMPLEMENTATION as _
    }
}

/// Check if an implementation_identifier pointer matches rmw_zenoh_rs.
/// Returns true if it matches, false otherwise.
pub fn check_impl_id(id: *const std::os::raw::c_char) -> bool {
    check_impl_id_ret(id) == RMW_RET_OK as rmw_ret_t
}

#[repr(C)]
pub struct rmw_error_string_t {
    pub str: [std::os::raw::c_char; 1024],
}

static mut RMW_ERROR_BUFFER: [std::os::raw::c_char; 1024] = [0; 1024];

#[unsafe(no_mangle)]
pub extern "C" fn rmw_set_error_string(error_string: *const std::os::raw::c_char) {
    unsafe {
        if !error_string.is_null() {
            let mut i = 0;
            while i < 1023 {
                let c = *error_string.add(i);
                RMW_ERROR_BUFFER[i] = c;
                if c == 0 {
                    break;
                }
                i += 1;
            }
            RMW_ERROR_BUFFER[i] = 0;
        } else {
            RMW_ERROR_BUFFER[0] = 0;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_get_error_string() -> rmw_error_string_t {
    unsafe {
        if RMW_ERROR_BUFFER[0] == 0 {
            let default_error = b"Mock error\0";
            for i in 0..default_error.len() {
                RMW_ERROR_BUFFER[i] = default_error[i] as std::os::raw::c_char;
            }
        }
        rmw_error_string_t {
            str: RMW_ERROR_BUFFER,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_reset_error() {
    unsafe {
        RMW_ERROR_BUFFER[0] = 0;
    }
}

/// Context implementation for RMW
pub struct ContextImpl {
    pub zcontext: Arc<hiroz::context::ZContext>,
    pub enclave: String,
    pub next_entity_id: Arc<Mutex<usize>>,
    pub is_shutdown: Arc<Mutex<bool>>,
    pub nodes: Arc<Mutex<HashMap<*const rmw_node_t, Arc<NodeImpl>>>>,
    pub notifier: Arc<Notifier>,
}

impl ContextImpl {
    pub fn new(domain_id: usize, enclave: String) -> Result<Self, String> {
        // Create ZContext using the builder
        let zcontext = hiroz::context::ZContextBuilder::default()
            .with_domain_id(domain_id)
            .with_enclave(enclave.clone())
            .build()
            .map_err(|e| format!("Failed to create ZContext: {}", e))?;

        // Set up the guard condition trigger function for graph events
        // This allows graph changes to trigger RMW guard conditions
        let trigger_fn: hiroz::event::GraphGuardConditionTrigger = Box::new(|gc_ptr| {
            crate::guard_condition::rmw_trigger_guard_condition(
                gc_ptr as *const crate::ros::rmw_guard_condition_t,
            );
        });
        zcontext
            .graph()
            .event_manager
            .set_guard_condition_trigger(trigger_fn);

        Ok(Self {
            zcontext: Arc::new(zcontext),
            enclave,
            next_entity_id: Arc::new(Mutex::new(1)),
            is_shutdown: Arc::new(Mutex::new(false)),
            #[allow(clippy::arc_with_non_send_sync)]
            nodes: Arc::new(Mutex::new(HashMap::new())),
            notifier: Arc::new(Notifier::default()),
        })
    }

    pub fn new_node(
        &self,
        name: *const ::std::os::raw::c_char,
        namespace_: *const ::std::os::raw::c_char,
        _context: *mut rmw_context_t,
        _options: *const rcl_node_options_t,
    ) -> Result<NodeImpl, String> {
        let name_str = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_str()
            .map_err(|e| format!("Invalid name string: {}", e))?;
        let namespace_str = unsafe { std::ffi::CStr::from_ptr(namespace_) }
            .to_str()
            .map_err(|e| format!("Invalid namespace string: {}", e))?;

        let node_impl = NodeImpl::new(&self.zcontext, name_str, namespace_str)
            .map_err(|e| format!("Failed to create node: {}", e))?;

        Ok(node_impl)
    }

    pub fn share_notifier(&self) -> Arc<Notifier> {
        self.notifier.clone()
    }
}

rmw_impl_has_impl_ptr!(rmw_context_t, rmw_context_impl_t, ContextImpl);

// RMW Context Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_init_options_init(
    init_options: *mut rmw_init_options_t,
    allocator: rcl_allocator_t,
) -> rmw_ret_t {
    if init_options.is_null() {
        rmw_set_error_string(c"Invalid argument".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Check if already initialized
    if unsafe { !(*init_options).implementation_identifier.is_null() } {
        rmw_set_error_string(c"init_options already initialized".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Check if allocator is valid
    if allocator.allocate.is_none()
        || allocator.deallocate.is_none()
        || allocator.reallocate.is_none()
    {
        rmw_set_error_string(c"Invalid allocator".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        // Set the fields
        (*init_options).domain_id = usize::MAX; // RCL_DEFAULT_DOMAIN_ID
        (*init_options).allocator = allocator;
        (*init_options).implementation_identifier = c"rmw_zenoh_rs".as_ptr();

        // Initialize discovery options (required by rolling)
        (*init_options).discovery_options =
            crate::ros::rmw_get_zero_initialized_discovery_options();
        let allocator_ptr = &allocator as *const _ as *mut _;
        let ret = crate::ros::rmw_discovery_options_init(
            &mut (*init_options).discovery_options,
            0,
            allocator_ptr,
        );
        if ret != (RMW_RET_OK as rmw_ret_t) {
            return ret;
        }
    }

    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_init_options_copy(
    src: *const rmw_init_options_t,
    dst: *mut rmw_init_options_t,
) -> rmw_ret_t {
    if src.is_null() || dst.is_null() {
        rmw_set_error_string(c"Invalid argument".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        // Check implementation identifier on src
        let ret = check_impl_id_ret((*src).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            rmw_set_error_string(c"Invalid or incorrect RMW implementation on src".as_ptr());
            return ret;
        }

        // Check if dst is already initialized (has a non-null implementation_identifier)
        if !(*dst).implementation_identifier.is_null() {
            rmw_set_error_string(c"dst already initialized".as_ptr());
            return RMW_RET_INVALID_ARGUMENT as _;
        }

        // Check if allocator is valid
        if (*src).allocator.allocate.is_none() {
            rmw_set_error_string(c"Invalid allocator".as_ptr());
            return RMW_RET_INVALID_ARGUMENT as _;
        }

        // Copy all fields from src to dst
        (*dst).instance_id = (*src).instance_id;
        (*dst).implementation_identifier = (*src).implementation_identifier;
        (*dst).domain_id = (*src).domain_id;
        (*dst).allocator = (*src).allocator;

        // Copy security options properly (required for deep copy of pointers)
        (*dst).security_options = crate::ros::rmw_get_zero_initialized_security_options();
        let allocator_ptr = &(*src).allocator as *const _ as *mut _;
        let ret = crate::ros::rmw_security_options_copy(
            &(*src).security_options,
            allocator_ptr,
            &mut (*dst).security_options,
        );
        if ret != (RMW_RET_OK as rmw_ret_t) {
            return ret;
        }

        // Copy discovery options (required by rolling)
        (*dst).discovery_options = crate::ros::rmw_get_zero_initialized_discovery_options();
        let allocator_ptr = &(*src).allocator as *const _ as *mut _;
        let ret = crate::ros::rmw_discovery_options_copy(
            &(*src).discovery_options,
            allocator_ptr,
            &mut (*dst).discovery_options,
        );
        if ret != (RMW_RET_OK as rmw_ret_t) {
            return ret;
        }

        // Copy enclave string if it exists
        if !(*src).enclave.is_null() {
            let enclave_cstr = std::ffi::CStr::from_ptr((*src).enclave);
            let enclave_bytes = enclave_cstr.to_bytes_with_nul();
            let allocator = &(*src).allocator;

            if let Some(allocate) = allocator.allocate {
                let new_enclave =
                    allocate(enclave_bytes.len(), allocator.state) as *mut std::os::raw::c_char;

                if new_enclave.is_null() {
                    rmw_set_error_string(c"Failed to allocate memory for enclave".as_ptr());
                    return RMW_RET_BAD_ALLOC as _;
                }

                std::ptr::copy_nonoverlapping(
                    enclave_bytes.as_ptr() as *const std::os::raw::c_char,
                    new_enclave,
                    enclave_bytes.len(),
                );
                (*dst).enclave = new_enclave;
            } else {
                // If no allocator, we can't copy the enclave string
                rmw_set_error_string(c"Invalid allocator for enclave copy".as_ptr());
                return RMW_RET_INVALID_ARGUMENT as _;
            }
        } else {
            (*dst).enclave = std::ptr::null_mut();
        }

        // For rmw_z, we don't have implementation-specific data, so impl_ is null
        (*dst).impl_ = std::ptr::null_mut();
    }

    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_init_options_fini(init_options: *mut rmw_init_options_t) -> rmw_ret_t {
    if init_options.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        // Check implementation identifier
        let ret = check_impl_id_ret((*init_options).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            rmw_set_error_string(c"Invalid or incorrect RMW implementation".as_ptr());
            return ret;
        }

        // Free enclave string if it was allocated
        if !(*init_options).enclave.is_null() {
            let allocator = &(*init_options).allocator;
            if let Some(deallocate) = allocator.deallocate {
                deallocate((*init_options).enclave as *mut _, allocator.state);
                (*init_options).enclave = std::ptr::null_mut();
            }
        }

        // Finalize security options (required to free allocated security_root_path)
        let allocator_ptr = &(*init_options).allocator as *const _ as *mut _;
        let ret = crate::ros::rmw_security_options_fini(
            &mut (*init_options).security_options,
            allocator_ptr,
        );
        if ret != (RMW_RET_OK as rmw_ret_t) {
            return ret;
        }

        // Finalize discovery options (required by rolling)
        let ret = crate::ros::rmw_discovery_options_fini(&mut (*init_options).discovery_options);
        if ret != (RMW_RET_OK as rmw_ret_t) {
            return ret;
        }

        // Free impl if it exists (rmw_z doesn't use it, but be safe)
        if !(*init_options).impl_.is_null() {
            // For now, rmw_z doesn't allocate impl_, so nothing to do
            (*init_options).impl_ = std::ptr::null_mut();
        }

        // Reset implementation_identifier so this struct looks uninitialized
        (*init_options).implementation_identifier = std::ptr::null();
    }

    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_init(
    options: *const rmw_init_options_t,
    context: *mut rmw_context_t,
) -> rmw_ret_t {
    if options.is_null() || context.is_null() {
        rmw_set_error_string(c"Invalid argument: null pointer".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        // Check implementation_identifier on options
        if (*options).implementation_identifier.is_null() {
            rmw_set_error_string(c"Options not initialized".as_ptr());
            return RMW_RET_INVALID_ARGUMENT as _;
        }
        if !check_impl_id((*options).implementation_identifier) {
            rmw_set_error_string(c"Incorrect RMW implementation on options".as_ptr());
            return RMW_RET_INCORRECT_RMW_IMPLEMENTATION as _;
        }

        // Check enclave is not null
        if (*options).enclave.is_null() {
            rmw_set_error_string(c"Enclave is null".as_ptr());
            return RMW_RET_INVALID_ARGUMENT as _;
        }
    }

    // Initialize Zenoh logging
    zenoh::init_log_from_env_or("error");

    // Log RMW initialization
    tracing::info!("rmw_zenoh_rs v{} initialized", env!("CARGO_PKG_VERSION"));

    // Check if already initialized
    if !unsafe { (*context).impl_.is_null() } {
        rmw_set_error_string(c"Context already initialized".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Create context implementation
    let domain_id = unsafe { (*options).domain_id };
    let enclave = unsafe {
        std::ffi::CStr::from_ptr((*options).enclave)
            .to_str()
            .unwrap_or("/")
            .to_string()
    };
    let context_impl = match ContextImpl::new(domain_id, enclave) {
        Ok(impl_) => impl_,
        Err(e) => {
            tracing::error!("Failed to create context: {}", e);
            rmw_set_error_string(c"Failed to create context".as_ptr());
            return RMW_RET_ERROR as _;
        }
    };

    // Assign implementation
    match context.assign_impl(context_impl) {
        Ok(_) => {
            unsafe {
                (*context).implementation_identifier =
                    crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const _;
                (*context).instance_id = 1; // TODO: proper instance ID
                (*context).actual_domain_id = domain_id; // Set the actual domain ID
            }
            RMW_RET_OK as _
        }
        Err(_) => {
            rmw_set_error_string(c"Failed to assign context impl".as_ptr());
            RMW_RET_ERROR as _
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_shutdown(context: *mut rmw_context_t) -> rmw_ret_t {
    if context.is_null() {
        rmw_set_error_string(c"Context is null".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        // Check implementation_identifier
        let ret = check_impl_id_ret((*context).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            rmw_set_error_string(c"Invalid or incorrect context".as_ptr());
            return ret;
        }
    }

    if unsafe { (*context).impl_.is_null() } {
        rmw_set_error_string(c"Context impl is null".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Mark context as shutdown (idempotent — double shutdown returns OK)
    match context.borrow_impl() {
        Ok(context_impl) => {
            *context_impl.is_shutdown.lock().unwrap() = true;
            RMW_RET_OK as _
        }
        Err(_) => RMW_RET_INVALID_ARGUMENT as _,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_context_fini(context: *mut rmw_context_t) -> rmw_ret_t {
    if context.is_null() {
        rmw_set_error_string(c"Context is null".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        // Check implementation_identifier
        let ret = check_impl_id_ret((*context).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            rmw_set_error_string(c"Invalid or incorrect context".as_ptr());
            return ret;
        }
    }

    // First try to borrow the implementation to validate it
    let context_impl = match context.borrow_impl() {
        Ok(impl_) => impl_,
        Err(_) => {
            rmw_set_error_string(c"Context impl is null".as_ptr());
            return RMW_RET_INVALID_ARGUMENT as _;
        }
    };

    // Check if context has been shut down
    if !*context_impl.is_shutdown.lock().unwrap() {
        rmw_set_error_string(c"Context not shut down".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _; // Must call rmw_shutdown before rmw_context_fini
    }

    // Check if there are still nodes attached to this context
    if !context_impl.nodes.lock().unwrap().is_empty() {
        rmw_set_error_string(c"Context still has active nodes".as_ptr());
        return RMW_RET_INVALID_ARGUMENT as _; // Cannot finalize context with active nodes
    }

    // Own and drop the implementation
    match context.own_impl() {
        Ok(_) => {
            // Clear implementation_identifier so re-init is possible
            unsafe {
                (*context).implementation_identifier = std::ptr::null();
            }
            RMW_RET_OK as _
        }
        Err(_) => {
            rmw_set_error_string(c"Failed to finalize context".as_ptr());
            RMW_RET_ERROR as _
        }
    }
}
