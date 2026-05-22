use super::{ErrorCode, cstr_to_str};
use crate::Builder;
use crate::context::ZContext;
use std::ffi::c_char;

/// Opaque context handle for FFI
#[repr(C)]
pub struct CContext {
    inner: Box<ZContext>,
}

/// Configuration struct for context creation
#[repr(C)]
pub struct CContextConfig {
    pub domain_id: u32,
    /// Path to a Zenoh JSON5 config file (nullable)
    pub config_file: *const c_char,
    /// Array of connect endpoint strings (nullable)
    pub connect_endpoints: *const *const c_char,
    /// Number of connect endpoints
    pub connect_endpoints_count: usize,
    /// Zenoh mode: "peer", "client", "router" (nullable = default)
    pub mode: *const c_char,
    /// Whether to disable multicast scouting
    pub disable_multicast_scouting: bool,
    /// Whether to connect to local zenohd on tcp/127.0.0.1:7447
    pub connect_to_local_zenohd: bool,
    /// JSON string for arbitrary Zenoh config overrides (nullable)
    /// Format: JSON object with dotted keys, e.g. {"scouting/multicast/enabled": false}
    pub json_config: *const c_char,
    /// Array of remap rule strings in "from:=to" format (nullable)
    pub remap_rules: *const *const c_char,
    /// Number of remap rules
    pub remap_rules_count: usize,
    /// Whether to enable logging
    pub enable_logging: bool,
    /// Default namespace inherited by nodes created from this context (nullable).
    /// Added after all pre-existing fields to preserve ABI compatibility.
    pub namespace: *const c_char,
}

/// Create a new hiroz context with default config (convenience)
///
/// # Safety
/// Must be called from a valid thread. The returned pointer must be freed
/// with `hiroz_context_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_context_create(domain_id: u32) -> *mut CContext {
    match crate::context::ZContextBuilder::default()
        .with_domain_id(domain_id as usize)
        .build()
    {
        Ok(ctx) => Box::into_raw(Box::new(CContext {
            inner: Box::new(ctx),
        })),
        Err(e) => {
            tracing::warn!("hiroz: Failed to create context: {}", e);
            std::ptr::null_mut()
        }
    }
}

/// Create a new hiroz context with full configuration
///
/// # Safety
/// `config` must be a valid pointer to a `CContextConfig` struct, or null.
/// String pointers within the config must be valid null-terminated C strings or null.
/// Array pointers must be valid for the specified count, or null with count 0.
/// The returned pointer must be freed with `hiroz_context_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_context_create_with_config(
    config: *const CContextConfig,
) -> *mut CContext {
    if config.is_null() {
        tracing::warn!("hiroz: Null config pointer");
        return std::ptr::null_mut();
    }

    unsafe {
        let cfg = &*config;
        let mut builder =
            crate::context::ZContextBuilder::default().with_domain_id(cfg.domain_id as usize);

        if let Some(Ok(namespace)) = (!cfg.namespace.is_null()).then(|| cstr_to_str(cfg.namespace))
        {
            builder = builder.with_namespace(namespace);
        }

        // Config file
        if let Some(Ok(path)) = (!cfg.config_file.is_null()).then(|| cstr_to_str(cfg.config_file)) {
            builder = builder.with_config_file(path);
        }

        // Connect endpoints
        if !cfg.connect_endpoints.is_null() && cfg.connect_endpoints_count > 0 {
            let endpoints: Vec<String> = (0..cfg.connect_endpoints_count)
                .filter_map(|i| {
                    let ptr = *cfg.connect_endpoints.add(i);
                    cstr_to_str(ptr).ok().map(|s| s.to_string())
                })
                .collect();
            if !endpoints.is_empty() {
                builder = builder.with_connect_endpoints(endpoints);
            }
        }

        // Mode
        if let Some(Ok(mode)) = (!cfg.mode.is_null()).then(|| cstr_to_str(cfg.mode)) {
            builder = builder.with_mode(mode);
        }

        // Disable multicast scouting
        if cfg.disable_multicast_scouting {
            builder = builder.disable_multicast_scouting();
        }

        // Connect to local zenohd
        if cfg.connect_to_local_zenohd {
            builder = builder.connect_to_local_zenohd();
        }

        // JSON config overrides
        if let Some(Ok(json_str)) =
            (!cfg.json_config.is_null()).then(|| cstr_to_str(cfg.json_config))
        {
            if let Ok(obj) =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(json_str)
            {
                for (key, value) in obj {
                    builder = builder.with_json(key, value);
                }
            } else {
                tracing::warn!("hiroz: Failed to parse json_config as JSON object");
            }
        }

        // Logging (before remap rules which can consume builder on error)
        if cfg.enable_logging {
            builder = builder.with_logging_enabled();
        }

        // Remap rules (last because with_remap_rules consumes self)
        if !cfg.remap_rules.is_null() && cfg.remap_rules_count > 0 {
            let rules: Vec<String> = (0..cfg.remap_rules_count)
                .filter_map(|i| {
                    let ptr = *cfg.remap_rules.add(i);
                    cstr_to_str(ptr).ok().map(|s| s.to_string())
                })
                .collect();
            if !rules.is_empty() {
                match builder.with_remap_rules(rules) {
                    Ok(b) => builder = b,
                    Err(e) => {
                        tracing::warn!("hiroz: Invalid remap rules: {}", e);
                        return std::ptr::null_mut();
                    }
                }
            }
        }

        match builder.build() {
            Ok(ctx) => Box::into_raw(Box::new(CContext {
                inner: Box::new(ctx),
            })),
            Err(e) => {
                tracing::warn!("hiroz: Failed to create context with config: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Shutdown and free context
///
/// # Safety
/// `ctx` must be a valid pointer returned by `hiroz_context_create` or
/// `hiroz_context_create_with_config`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_context_destroy(ctx: *mut CContext) -> i32 {
    if ctx.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let _ = Box::from_raw(ctx);
    }
    ErrorCode::Success as i32
}

/// Get internal context reference (for use by other FFI functions)
///
/// # Safety
/// `ctx` must be a valid pointer to a `CContext` or null.
pub(crate) unsafe fn get_context_ref<'a>(ctx: *mut CContext) -> Option<&'a ZContext> {
    if ctx.is_null() {
        None
    } else {
        unsafe { Some(&(*ctx).inner) }
    }
}
