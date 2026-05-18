use std::ffi::CString;

use crate::rmw_impl_has_data_ptr;
use crate::ros::*;
use crate::traits::*;
use hiroz::Builder;

/// Node implementation for RMW
pub struct NodeImpl {
    pub inner: hiroz::node::ZNode,
    pub name: CString,
    pub namespace: CString,
    pub fq_name: CString,
    pub graph_guard_condition: *mut rmw_guard_condition_t,
}

impl NodeImpl {
    pub fn new(
        zcontext: &hiroz::context::ZContext,
        name: &str,
        namespace: &str,
    ) -> Result<Self, String> {
        let name_cstr = CString::new(name).map_err(|e| format!("Invalid name string: {}", e))?;
        let namespace_cstr =
            CString::new(namespace).map_err(|e| format!("Invalid namespace string: {}", e))?;
        let fq_name = if namespace.is_empty() || namespace == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", namespace, name)
        };
        let fq_name_cstr =
            CString::new(fq_name).map_err(|e| format!("Invalid fully qualified name: {}", e))?;

        // Use ZContext's create_node method
        let inner = zcontext
            .create_node(name)
            .with_namespace(namespace)
            .build()
            .map_err(|e| format!("Failed to build node: {}", e))?;

        Ok(Self {
            inner,
            name: name_cstr,
            namespace: namespace_cstr,
            fq_name: fq_name_cstr,
            graph_guard_condition: std::ptr::null_mut(),
        })
    }
}

rmw_impl_has_data_ptr!(rmw_node_t, rmw_node_impl_t, NodeImpl);

// RMW Node Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_create_node(
    context: *mut rmw_context_t,
    name: *const std::os::raw::c_char,
    namespace_: *const std::os::raw::c_char,
) -> *mut rmw_node_t {
    if context.is_null() || name.is_null() || namespace_.is_null() {
        return std::ptr::null_mut();
    }

    // Check context implementation_identifier
    unsafe {
        if !crate::context::check_impl_id((*context).implementation_identifier) {
            return std::ptr::null_mut();
        }
    }

    // Validate node name using rmw_validate_node_name
    unsafe {
        let mut validation_result: std::os::raw::c_int = 0;
        let mut invalid_index: usize = 0;
        let ret =
            crate::ros::rmw_validate_node_name(name, &mut validation_result, &mut invalid_index);
        if ret != RMW_RET_OK as rmw_ret_t || validation_result != 0 {
            return std::ptr::null_mut();
        }
    }

    // Validate namespace using rmw_validate_namespace
    unsafe {
        let mut validation_result: std::os::raw::c_int = 0;
        let mut invalid_index: usize = 0;
        let ret = crate::ros::rmw_validate_namespace(
            namespace_,
            &mut validation_result,
            &mut invalid_index,
        );
        if ret != RMW_RET_OK as rmw_ret_t || validation_result != 0 {
            return std::ptr::null_mut();
        }
    }

    let context_impl = match context.borrow_impl() {
        Ok(impl_) => impl_,
        Err(_) => return std::ptr::null_mut(),
    };

    // Check if context has been shut down
    if *context_impl.is_shutdown.lock().unwrap() {
        return std::ptr::null_mut();
    }

    let _name_str = unsafe { std::ffi::CStr::from_ptr(name) }
        .to_str()
        .unwrap_or("");
    let _namespace_str = unsafe { std::ffi::CStr::from_ptr(namespace_) }
        .to_str()
        .unwrap_or("");

    let mut node_impl = match context_impl.new_node(name, namespace_, context, std::ptr::null()) {
        Ok(impl_) => impl_,
        Err(_) => return std::ptr::null_mut(),
    };

    // Create a graph guard condition for this node
    let graph_guard_condition = crate::guard_condition::rmw_create_guard_condition(context);
    if graph_guard_condition.is_null() {
        return std::ptr::null_mut();
    }
    node_impl.graph_guard_condition = graph_guard_condition;

    // Register the graph guard condition with the graph event manager
    node_impl
        .inner
        .graph()
        .event_manager
        .register_graph_guard_condition(graph_guard_condition as *mut std::ffi::c_void);

    // Add node to local graph for immediate discovery
    if let Err(e) = node_impl
        .inner
        .graph()
        .add_local_entity(hiroz::entity::Entity::Node(
            node_impl.inner.node_entity().clone(),
        ))
    {
        tracing::warn!("Failed to add node to local graph: {}", e);
    }

    // Get pointers to the owned CStrings in node_impl
    let name_ptr = node_impl.name.as_ptr();
    let namespace_ptr = node_impl.namespace.as_ptr();

    let node = Box::new(rmw_node_t {
        implementation_identifier: crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const _,
        data: std::ptr::null_mut(),
        name: name_ptr as *const _,
        namespace_: namespace_ptr as *const _,
        context,
    });

    let node_ptr = Box::into_raw(node);
    unsafe {
        (*node_ptr).data = Box::into_raw(Box::new(node_impl)) as *mut _;
    }

    node_ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_destroy_node(node: *mut rmw_node_t) -> rmw_ret_t {
    if node.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Check implementation_identifier (NULL → INVALID_ARGUMENT, wrong → INCORRECT_RMW_IMPLEMENTATION)
    unsafe {
        let ret = crate::context::check_impl_id_ret((*node).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    // Remove node from local graph and destroy the graph guard condition
    if let Ok(node_impl) = node.borrow_data() {
        // Remove node from local graph
        if let Err(e) = node_impl
            .inner
            .graph()
            .remove_local_entity(&hiroz::entity::Entity::Node(
                node_impl.inner.node_entity().clone(),
            ))
        {
            tracing::warn!("Failed to remove node from local graph: {}", e);
        }

        if !node_impl.graph_guard_condition.is_null() {
            // Unregister from graph event manager
            node_impl
                .inner
                .graph()
                .event_manager
                .unregister_graph_guard_condition(
                    node_impl.graph_guard_condition as *mut std::ffi::c_void,
                );
            crate::guard_condition::rmw_destroy_guard_condition(node_impl.graph_guard_condition);
        }
    }

    // Drop the implementation data
    let _ = node.own_data();

    drop(unsafe { Box::from_raw(node) });
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_get_node_names(
    node: *const rmw_node_t,
    node_names: *mut rcutils_string_array_t,
    node_namespaces: *mut rcutils_string_array_t,
) -> rmw_ret_t {
    if node.is_null() || node_names.is_null() || node_namespaces.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*node).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    // Reject pre-initialized string arrays (must be zero-initialized)
    unsafe {
        if (*node_names).size != 0 || !(*node_names).data.is_null() {
            return RMW_RET_INVALID_ARGUMENT as _;
        }
        if (*node_namespaces).size != 0 || !(*node_namespaces).data.is_null() {
            return RMW_RET_INVALID_ARGUMENT as _;
        }
    }

    let node_impl = match node.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Check context is valid
    let context = unsafe { (*node).context };
    if context.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Get allocator - use default allocator since RCL doesn't pass one
    let allocator = unsafe { rcutils_get_default_allocator() };

    // Query graph for all nodes
    let nodes = node_impl.inner.graph().get_node_names();
    let node_count = nodes.len();

    // Initialize string arrays
    unsafe {
        let ret = rcutils_string_array_init(node_names, node_count, &allocator as *const _);
        if ret != 0 {
            return RMW_RET_BAD_ALLOC as _;
        }

        let ret = rcutils_string_array_init(node_namespaces, node_count, &allocator as *const _);
        if ret != 0 {
            // Clean up node_names on error
            rcutils_string_array_fini(node_names);
            return RMW_RET_BAD_ALLOC as _;
        }

        // Populate the arrays
        for (i, (name, namespace)) in nodes.iter().enumerate() {
            let name_cstr = match std::ffi::CString::new(name.as_str()) {
                Ok(s) => s,
                Err(_) => {
                    rcutils_string_array_fini(node_names);
                    rcutils_string_array_fini(node_namespaces);
                    return RMW_RET_ERROR as _;
                }
            };
            let ns_cstr = match std::ffi::CString::new(namespace.as_str()) {
                Ok(s) => s,
                Err(_) => {
                    rcutils_string_array_fini(node_names);
                    rcutils_string_array_fini(node_namespaces);
                    return RMW_RET_ERROR as _;
                }
            };

            (*node_names)
                .data
                .add(i)
                .write(rcutils_strdup(name_cstr.as_ptr(), allocator));
            if (*node_names).data.add(i).read().is_null() {
                rcutils_string_array_fini(node_names);
                rcutils_string_array_fini(node_namespaces);
                return RMW_RET_BAD_ALLOC as _;
            }

            (*node_namespaces)
                .data
                .add(i)
                .write(rcutils_strdup(ns_cstr.as_ptr(), allocator));
            if (*node_namespaces).data.add(i).read().is_null() {
                rcutils_string_array_fini(node_names);
                rcutils_string_array_fini(node_namespaces);
                return RMW_RET_BAD_ALLOC as _;
            }
        }
    }

    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_get_node_names_with_enclaves(
    node: *const rmw_node_t,
    node_names: *mut rcutils_string_array_t,
    node_namespaces: *mut rcutils_string_array_t,
    enclaves: *mut rcutils_string_array_t,
) -> rmw_ret_t {
    if node.is_null() || node_names.is_null() || node_namespaces.is_null() || enclaves.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*node).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    // Reject pre-initialized string arrays (must be zero-initialized)
    unsafe {
        if (*node_names).size != 0 || !(*node_names).data.is_null() {
            return RMW_RET_INVALID_ARGUMENT as _;
        }
        if (*node_namespaces).size != 0 || !(*node_namespaces).data.is_null() {
            return RMW_RET_INVALID_ARGUMENT as _;
        }
        if (*enclaves).size != 0 || !(*enclaves).data.is_null() {
            return RMW_RET_INVALID_ARGUMENT as _;
        }
    }

    let node_impl = match node.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Check context is valid
    let context = unsafe { (*node).context };
    if context.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    // Get allocator - use default allocator since RCL doesn't pass one
    let allocator = unsafe { rcutils_get_default_allocator() };

    // Query graph for all nodes with enclaves
    let nodes = node_impl.inner.graph().get_node_names_with_enclaves();
    let node_count = nodes.len();

    // Initialize string arrays
    unsafe {
        let ret = rcutils_string_array_init(node_names, node_count, &allocator as *const _);
        if ret != 0 {
            return RMW_RET_BAD_ALLOC as _;
        }

        let ret = rcutils_string_array_init(node_namespaces, node_count, &allocator as *const _);
        if ret != 0 {
            // Clean up node_names on error
            rcutils_string_array_fini(node_names);
            return RMW_RET_BAD_ALLOC as _;
        }

        let ret = rcutils_string_array_init(enclaves, node_count, &allocator as *const _);
        if ret != 0 {
            // Clean up on error
            rcutils_string_array_fini(node_names);
            rcutils_string_array_fini(node_namespaces);
            return RMW_RET_BAD_ALLOC as _;
        }

        // Populate the arrays
        for (i, (name, namespace, enclave)) in nodes.iter().enumerate() {
            let name_cstr = match std::ffi::CString::new(name.as_str()) {
                Ok(s) => s,
                Err(_) => {
                    rcutils_string_array_fini(node_names);
                    rcutils_string_array_fini(node_namespaces);
                    rcutils_string_array_fini(enclaves);
                    return RMW_RET_ERROR as _;
                }
            };
            let ns_cstr = match std::ffi::CString::new(namespace.as_str()) {
                Ok(s) => s,
                Err(_) => {
                    rcutils_string_array_fini(node_names);
                    rcutils_string_array_fini(node_namespaces);
                    rcutils_string_array_fini(enclaves);
                    return RMW_RET_ERROR as _;
                }
            };
            let enclave_cstr = match std::ffi::CString::new(enclave.as_str()) {
                Ok(s) => s,
                Err(_) => {
                    rcutils_string_array_fini(node_names);
                    rcutils_string_array_fini(node_namespaces);
                    rcutils_string_array_fini(enclaves);
                    return RMW_RET_ERROR as _;
                }
            };

            (*node_names)
                .data
                .add(i)
                .write(rcutils_strdup(name_cstr.as_ptr(), allocator));
            if (*node_names).data.add(i).read().is_null() {
                rcutils_string_array_fini(node_names);
                rcutils_string_array_fini(node_namespaces);
                rcutils_string_array_fini(enclaves);
                return RMW_RET_BAD_ALLOC as _;
            }

            (*node_namespaces)
                .data
                .add(i)
                .write(rcutils_strdup(ns_cstr.as_ptr(), allocator));
            if (*node_namespaces).data.add(i).read().is_null() {
                rcutils_string_array_fini(node_names);
                rcutils_string_array_fini(node_namespaces);
                rcutils_string_array_fini(enclaves);
                return RMW_RET_BAD_ALLOC as _;
            }

            (*enclaves)
                .data
                .add(i)
                .write(rcutils_strdup(enclave_cstr.as_ptr(), allocator));
            if (*enclaves).data.add(i).read().is_null() {
                rcutils_string_array_fini(node_names);
                rcutils_string_array_fini(node_namespaces);
                rcutils_string_array_fini(enclaves);
                return RMW_RET_BAD_ALLOC as _;
            }
        }
    }

    RMW_RET_OK as _
}
