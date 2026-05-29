//! Auto-generation of Python msgspec structs and PyO3 bindings for ROS 2 messages
//!
//! This module generates both Python msgspec structs and complete Rust PyO3 modules
//! from ROS message definitions, eliminating the need for manual registry code.

use crate::types::{ArrayType, FieldType, ResolvedMessage, ResolvedService};
use anyhow::Result;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Generate both Python msgspec structs AND complete Rust PyO3 module
pub fn generate_python_bindings(
    messages: &[ResolvedMessage],
    services: &[ResolvedService],
    python_output_dir: &Path,
    rust_output_path: &Path,
) -> Result<()> {
    // Group messages by package
    let mut packages: HashMap<String, Vec<&ResolvedMessage>> = HashMap::new();
    for msg in messages {
        packages
            .entry(msg.parsed.package.clone())
            .or_default()
            .push(msg);
    }

    // Group services by package so we can emit rclpy-style grouping classes (P4).
    let mut service_groups: HashMap<String, Vec<&ResolvedService>> = HashMap::new();
    for srv in services {
        service_groups
            .entry(srv.parsed.package.clone())
            .or_default()
            .push(srv);
    }

    // Group service Request/Response by package, and track service type hashes
    let mut service_messages: HashMap<String, Vec<&ResolvedMessage>> = HashMap::new();
    let mut service_hashes: HashMap<String, HashMap<String, String>> = HashMap::new();
    for srv in services {
        let svc_hash = srv.type_hash.to_rihs_string();

        // Add request message
        service_messages
            .entry(srv.request.parsed.package.clone())
            .or_default()
            .push(&srv.request);
        service_hashes
            .entry(srv.request.parsed.package.clone())
            .or_default()
            .insert(srv.request.parsed.name.clone(), svc_hash.clone());

        // Add response message
        service_messages
            .entry(srv.response.parsed.package.clone())
            .or_default()
            .push(&srv.response);
        service_hashes
            .entry(srv.response.parsed.package.clone())
            .or_default()
            .insert(srv.response.parsed.name.clone(), svc_hash);
    }

    // Generate Python msgspec structs (one file per package)
    for (package_name, package_msgs) in &packages {
        // Combine regular messages with service Request/Response for this package
        let srv_msgs = service_messages
            .get(package_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let svc_hashes = service_hashes
            .get(package_name)
            .cloned()
            .unwrap_or_default();
        let srv_groups = service_groups
            .get(package_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let python_code = generate_python_package_with_services(
            package_name,
            package_msgs,
            srv_msgs,
            &svc_hashes,
            srv_groups,
        )?;
        let output_path = python_output_dir.join(format!("{}.py", package_name));
        fs::write(output_path, python_code)?;
    }

    // Generate Python files for packages that only have service types
    for (package_name, srv_msgs) in &service_messages {
        if !packages.contains_key(package_name) {
            let svc_hashes = service_hashes
                .get(package_name)
                .cloned()
                .unwrap_or_default();
            let srv_groups = service_groups
                .get(package_name)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let python_code = generate_python_package_with_services(
                package_name,
                &[],
                srv_msgs,
                &svc_hashes,
                srv_groups,
            )?;
            let output_path = python_output_dir.join(format!("{}.py", package_name));
            fs::write(output_path, python_code)?;
        }
    }

    // Generate __init__.py for Python package
    let init_code = generate_python_init(&packages)?;
    fs::write(python_output_dir.join("__init__.py"), init_code)?;

    // Generate COMPLETE Rust PyO3 module (replaces python_registry.rs entirely)
    let rust_tokens = generate_complete_rust_module(&packages, services)?;
    let rust_code = tokens_to_string(rust_tokens);
    fs::write(rust_output_path, rust_code)?;

    Ok(())
}

/// Convert TokenStream to formatted Rust code string
fn tokens_to_string(tokens: TokenStream) -> String {
    let file = syn::parse2::<syn::File>(tokens).expect("Failed to parse generated tokens");
    prettyplease::unparse(&file)
}

/// Generate Python msgspec structs for a package (messages + service Request/Response)
fn generate_python_package_with_services(
    package_name: &str,
    messages: &[&ResolvedMessage],
    service_messages: &[&ResolvedMessage],
    service_hashes: &HashMap<String, String>,
    service_groups: &[&ResolvedService],
) -> Result<String> {
    let mut code = format!(
        "\"\"\"Auto-generated ROS 2 message types for {}.\"\"\"\n\
         import msgspec\n\
         from typing import ClassVar\n\n",
        package_name
    );

    // Generate regular message structs
    for msg in messages {
        code.push_str(&generate_msgspec_struct(msg, None)?);
    }

    // Generate service Request/Response structs with service type hash
    for msg in service_messages {
        let svc_hash = service_hashes.get(&msg.parsed.name);
        code.push_str(&generate_msgspec_struct(msg, svc_hash.map(|s| s.as_str()))?);
    }

    // Emit rclpy-style service grouping classes (P4). These reference the
    // Request/Response structs above, so they must come after them.
    for srv in service_groups {
        code.push_str(&generate_service_grouping_class(srv));
    }

    Ok(code)
}

/// Generate a service grouping class: `AddTwoInts.Request` / `.Response` (P4).
///
/// Lets `create_client`/`create_server` accept a single rclpy-style type
/// (`example_interfaces.AddTwoInts`) instead of the bare Request class.
fn generate_service_grouping_class(srv: &ResolvedService) -> String {
    let srv_name = &srv.parsed.name;
    let package = &srv.parsed.package;
    let request_struct = &srv.request.parsed.name;
    let response_struct = &srv.response.parsed.name;
    format!(
        "class {srv_name}:\n    \
         \"\"\"Service grouping type. Use {srv_name}.Request and {srv_name}.Response.\"\"\"\n    \
         __srvtype__: ClassVar[str] = '{package}/srv/{srv_name}'\n    \
         Request: ClassVar[type] = {request_struct}\n    \
         Response: ClassVar[type] = {response_struct}\n\n"
    )
}

fn rust_to_python_type(field_type: &FieldType, current_package: &str) -> Result<String> {
    // Get the base field type (without array indicators)
    let base_type = &field_type.base_type;

    // Check if this is an array type
    let is_array = !matches!(field_type.array, ArrayType::Single);

    // Special case: byte[] and uint8[] unbounded arrays use bytes type for performance
    if is_array
        && matches!(base_type.as_str(), "byte" | "uint8")
        && matches!(field_type.array, ArrayType::Unbounded)
    {
        return Ok("bytes".to_string());
    }

    // First check if this is a primitive type
    let python_type = match base_type.as_str() {
        "bool" => "bool",
        "byte" | "int8" | "char" | "uint8" | "int16" | "uint16" | "int32" | "uint32" | "int64"
        | "uint64" => "int",
        "float32" | "float64" => "float",
        "string" | "wstring" => "str",
        _ => {
            // Not a primitive type - it's a nested message
            let package = field_type.package.as_deref().unwrap_or(current_package);

            return Ok(if is_array {
                format!("list[\"{}.{}\"]", package, base_type)
            } else {
                // Use Optional (entire annotation as string for forward ref compatibility)
                format!("\"{}.{} | None\"", package, base_type)
            });
        }
    };

    // Return primitive type, with list wrapper if it's an array
    Ok(if is_array {
        format!("list[{}]", python_type)
    } else {
        python_type.to_string()
    })
}

fn get_python_default(field_type: &FieldType) -> String {
    match &field_type.array {
        ArrayType::Single => match field_type.base_type.as_str() {
            "bool" => "False".to_string(),
            "byte" | "int8" | "char" | "uint8" | "int16" | "uint16" | "int32" | "uint32"
            | "int64" | "uint64" => "0".to_string(),
            "float32" | "float64" => "0.0".to_string(),
            "string" | "wstring" => "\"\"".to_string(),
            type_str if type_str.contains("Time") || type_str.contains("Duration") => {
                "msgspec.field(default_factory=lambda: {'sec': 0, 'nanosec': 0})".to_string()
            }
            _ => "None".to_string(), // For nested types - None allows proper kwargs passing
        },
        // Special case: byte[] and uint8[] unbounded arrays default to empty bytes
        ArrayType::Unbounded if matches!(field_type.base_type.as_str(), "byte" | "uint8") => {
            "b\"\"".to_string()
        }
        ArrayType::Fixed(_) | ArrayType::Bounded(_) | ArrayType::Unbounded => {
            "msgspec.field(default_factory=list)".to_string()
        }
    }
}

fn generate_msgspec_struct(msg: &ResolvedMessage, svc_hash: Option<&str>) -> Result<String> {
    let name = &msg.parsed.name;
    let package = &msg.parsed.package;
    let hash = msg.type_hash.to_rihs_string();

    let mut code = format!(
        "class {}(msgspec.Struct, frozen=True, kw_only=True):\n",
        name
    );

    // Generate fields
    for field in &msg.parsed.fields {
        let py_type = rust_to_python_type(&field.field_type, package)?;
        let default = get_python_default(&field.field_type);
        code.push_str(&format!("    {}: {} = {}\n", field.name, py_type, default));
    }

    // Add metadata
    code.push_str(&format!(
        "\n    __msgtype__: ClassVar[str] = '{}/msg/{}'\n",
        package, name
    ));

    // Use service type hash for service request/response, message type hash for regular messages
    if let Some(srv_hash) = svc_hash {
        code.push_str(&format!("    __hash__: ClassVar[str] = '{}'\n", srv_hash));
    } else {
        code.push_str(&format!("    __hash__: ClassVar[str] = '{}'\n", hash));
    }

    code.push('\n');

    Ok(code)
}

/// Generate serialize function for a message using TokenStream
fn generate_serialize_function(msg: &ResolvedMessage) -> TokenStream {
    let package = &msg.parsed.package;
    let name = &msg.parsed.name;
    let fn_name = format_ident!("serialize_{}", name.to_lowercase());
    let package_ident = format_ident!("{}", package.replace('-', "_"));
    let name_ident = format_ident!("{}", name);

    quote! {
        #[allow(clippy::useless_conversion)]
        #[pyfunction]
        pub fn #fn_name(py: Python, msg: &Bound<'_, PyAny>) -> PyResult<Py<::pyo3::types::PyBytes>> {
            use ::pyo3::types::PyBytes;
            use ::hiroz::python_bridge::FromPyMessage;
            // Extract Rust struct from Python object using derive macro
            let rust_msg = <ros::#package_ident::#name_ident>::from_py(msg)?;

            // CDR encapsulation header for little-endian encoding
            const CDR_HEADER_LE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];
            let mut cdr_bytes = hiroz_cdr::to_vec::<_, hiroz_cdr::LittleEndian>(&rust_msg, 256)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            // Prepend header to payload
            let mut result = Vec::with_capacity(4 + cdr_bytes.len());
            result.extend_from_slice(&CDR_HEADER_LE);
            result.append(&mut cdr_bytes);
            // Return as Python bytes (zero-copy when possible)
            Ok(PyBytes::new_bound(py, &result).unbind())
        }
    }
}

/// Generate deserialize function for a message using TokenStream
fn generate_deserialize_function(msg: &ResolvedMessage) -> TokenStream {
    let package = &msg.parsed.package;
    let name = &msg.parsed.name;
    let fn_name = format_ident!("deserialize_{}", name.to_lowercase());
    let package_ident = format_ident!("{}", package.replace('-', "_"));
    let name_ident = format_ident!("{}", name);

    quote! {
        #[allow(clippy::useless_conversion)]
        #[pyfunction]
        pub fn #fn_name(py: Python, bytes: &[u8]) -> PyResult<PyObject> {
            use ::hiroz::python_bridge::IntoPyMessage;
            // Skip 4-byte CDR encapsulation header
            if bytes.len() < 4 {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "CDR data too short: missing encapsulation header"
                ));
            }
            let payload = &bytes[4..];
            let (rust_msg, _): (ros::#package_ident::#name_ident, _) =
                hiroz_cdr::from_bytes::<_, hiroz_cdr::LittleEndian>(payload)
                    .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

            // Convert Rust struct to Python object using derive macro
            rust_msg.into_py_message(py)
        }
    }
}

/// Generate serialize/deserialize functions for a message
fn generate_message_functions(msg: &ResolvedMessage) -> TokenStream {
    let serialize_fn = generate_serialize_function(msg);
    let deserialize_fn = generate_deserialize_function(msg);

    quote! {
        #serialize_fn

        #deserialize_fn
    }
}

/// Generate COMPLETE Rust module with PyO3 bindings
fn generate_complete_rust_module(
    packages: &HashMap<String, Vec<&ResolvedMessage>>,
    services: &[ResolvedService],
) -> Result<TokenStream> {
    // Build lookup map for all messages
    let mut all_messages: HashMap<String, &ResolvedMessage> = HashMap::new();
    for package_msgs in packages.values() {
        for msg in package_msgs {
            let key = format!("{}/{}", msg.parsed.package, msg.parsed.name);
            all_messages.insert(key, *msg);
        }
    }
    // Also add service Request/Response to lookup map
    for srv in services {
        let req_key = format!("{}/{}", srv.request.parsed.package, srv.request.parsed.name);
        let resp_key = format!(
            "{}/{}",
            srv.response.parsed.package, srv.response.parsed.name
        );
        all_messages.insert(req_key, &srv.request);
        all_messages.insert(resp_key, &srv.response);
    }

    // Generate message package modules
    let mut package_modules = Vec::new();
    for (package_name, package_msgs) in packages {
        let mod_name = format_ident!("{}_py", package_name.replace('-', "_"));
        let msg_functions: Vec<TokenStream> = package_msgs
            .iter()
            .map(|msg| generate_message_functions(msg))
            .collect();

        package_modules.push(quote! {
            #[allow(unsafe_op_in_unsafe_fn, clippy::useless_conversion, unused_variables)]
            mod #mod_name {
                use super::*;

                #(#msg_functions)*
            }
        });
    }

    // Generate service package modules
    let mut service_packages: HashMap<String, Vec<&ResolvedService>> = HashMap::new();
    for srv in services {
        service_packages
            .entry(srv.parsed.package.clone())
            .or_default()
            .push(srv);
    }

    let mut service_modules = Vec::new();
    for (package_name, package_srvs) in &service_packages {
        let mod_name = format_ident!("{}_srv_py", package_name.replace('-', "_"));
        let srv_functions: Vec<TokenStream> = package_srvs
            .iter()
            .flat_map(|srv| {
                vec![
                    generate_message_functions(&srv.request),
                    generate_message_functions(&srv.response),
                ]
            })
            .collect();

        service_modules.push(quote! {
            #[allow(unsafe_op_in_unsafe_fn, clippy::useless_conversion, unused_variables)]
            mod #mod_name {
                use super::*;

                #(#srv_functions)*
            }
        });
    }

    // Generate registry registration code
    let registry_registrations = generate_registry_registrations(packages, services);

    // Generate helper functions
    let helper_fns = generate_helper_functions(packages, services, &all_messages);

    // Generate serialize_to_zbuf function
    let serialize_to_zbuf_fn = generate_serialize_to_zbuf(packages, services);

    // Generate Rust API wrappers
    let rust_api_wrappers = quote! {
        /// Serialize a Python message to CDR bytes
        pub fn serialize_to_cdr(type_name: &str, py: Python, msg: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
            unsafe { serialize_message(py, type_name, msg) }
        }

        /// Deserialize CDR bytes to a Python message
        pub fn deserialize_from_cdr(type_name: &str, py: Python, bytes: &[u8]) -> PyResult<PyObject> {
            unsafe { deserialize_message(py, type_name, bytes) }
        }
    };

    Ok(quote! {
        /// Auto-generated Python bindings for ROS 2 messages
        /// Generated by hiroz-codegen - DO NOT EDIT
        use pyo3::prelude::*;
        use pyo3::types::{PyAny, PyDict, PyModule};

        #(#package_modules)*

        #(#service_modules)*

        #[allow(unsafe_op_in_unsafe_fn)]
        #[pymodule]
        pub fn hiroz_msgs(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
            // Create message type registry as Python dict
            let registry = PyDict::new_bound(py);

            #registry_registrations

            m.add("REGISTRY", registry)?;
            m.add_function(wrap_pyfunction!(serialize_message, m)?)?;
            m.add_function(wrap_pyfunction!(deserialize_message, m)?)?;
            m.add_function(wrap_pyfunction!(list_registered_types, m)?)?;
            m.add_function(wrap_pyfunction!(get_type_hash, m)?)?;
            Ok(())
        }

        #helper_fns

        #serialize_to_zbuf_fn

        #rust_api_wrappers
    })
}

/// Generate registry registration code
fn generate_registry_registrations(
    packages: &HashMap<String, Vec<&ResolvedMessage>>,
    services: &[ResolvedService],
) -> TokenStream {
    let mut registrations = Vec::new();

    // Register message types
    for (package_name, package_msgs) in packages {
        for msg in package_msgs {
            let full_name = format!("{}/msg/{}", package_name, msg.parsed.name);
            let hash = msg.type_hash.to_rihs_string();
            let mod_name = format_ident!("{}_py", package_name.replace('-', "_"));
            let ser_fn = format_ident!("serialize_{}", msg.parsed.name.to_lowercase());
            let de_fn = format_ident!("deserialize_{}", msg.parsed.name.to_lowercase());

            registrations.push(quote! {
                {
                    let msg_info = PyDict::new_bound(py);
                    msg_info.set_item("serialize", wrap_pyfunction!(#mod_name::#ser_fn, m)?)?;
                    msg_info.set_item("deserialize", wrap_pyfunction!(#mod_name::#de_fn, m)?)?;
                    msg_info.set_item("hash", #hash)?;
                    registry.set_item(#full_name, msg_info)?;
                }
            });
        }
    }

    // Register service Request/Response types
    for srv in services {
        let package_name = &srv.parsed.package;
        let srv_name = &srv.parsed.name;
        let mod_name = format_ident!("{}_srv_py", package_name.replace('-', "_"));

        // Request type
        let req_full_name = format!("{}/srv/{}_Request", package_name, srv_name);
        let req_hash = srv.request.type_hash.to_rihs_string();
        let req_ser_fn = format_ident!("serialize_{}", srv.request.parsed.name.to_lowercase());
        let req_de_fn = format_ident!("deserialize_{}", srv.request.parsed.name.to_lowercase());

        registrations.push(quote! {
            {
                let msg_info = PyDict::new_bound(py);
                msg_info.set_item("serialize", wrap_pyfunction!(#mod_name::#req_ser_fn, m)?)?;
                msg_info.set_item("deserialize", wrap_pyfunction!(#mod_name::#req_de_fn, m)?)?;
                msg_info.set_item("hash", #req_hash)?;
                registry.set_item(#req_full_name, msg_info)?;
            }
        });

        // Response type
        let resp_full_name = format!("{}/srv/{}_Response", package_name, srv_name);
        let resp_hash = srv.response.type_hash.to_rihs_string();
        let resp_ser_fn = format_ident!("serialize_{}", srv.response.parsed.name.to_lowercase());
        let resp_de_fn = format_ident!("deserialize_{}", srv.response.parsed.name.to_lowercase());

        registrations.push(quote! {
            {
                let msg_info = PyDict::new_bound(py);
                msg_info.set_item("serialize", wrap_pyfunction!(#mod_name::#resp_ser_fn, m)?)?;
                msg_info.set_item("deserialize", wrap_pyfunction!(#mod_name::#resp_de_fn, m)?)?;
                msg_info.set_item("hash", #resp_hash)?;
                registry.set_item(#resp_full_name, msg_info)?;
            }
        });
    }

    quote! {
        #(#registrations)*
    }
}

/// Generate helper functions module
fn generate_helper_functions(
    packages: &HashMap<String, Vec<&ResolvedMessage>>,
    services: &[ResolvedService],
    _all_messages: &HashMap<String, &ResolvedMessage>,
) -> TokenStream {
    // Collect all registered type names
    let mut type_names: Vec<String> = Vec::new();
    for (package_name, package_msgs) in packages {
        for msg in package_msgs {
            type_names.push(format!("{}/msg/{}", package_name, msg.parsed.name));
        }
    }
    for srv in services {
        type_names.push(format!("{}/srv/{}", srv.parsed.package, srv.parsed.name));
        type_names.push(format!(
            "{}/srv/{}_Request",
            srv.parsed.package, srv.parsed.name
        ));
        type_names.push(format!(
            "{}/srv/{}_Response",
            srv.parsed.package, srv.parsed.name
        ));
    }

    // Generate type hash match arms
    let mut hash_arms = Vec::new();
    for (package_name, package_msgs) in packages {
        for msg in package_msgs {
            let full_name = format!("{}/msg/{}", package_name, msg.parsed.name);
            let hash = msg.type_hash.to_rihs_string();
            hash_arms.push(quote! {
                #full_name => Ok(#hash.to_string()),
            });
        }
    }
    for srv in services {
        let full_name = format!("{}/srv/{}", srv.parsed.package, srv.parsed.name);
        let hash = srv.type_hash.to_rihs_string();
        hash_arms.push(quote! { #full_name => Ok(#hash.to_string()), });

        let req_name = format!("{}/srv/{}_Request", srv.parsed.package, srv.parsed.name);
        let req_hash = srv.request.type_hash.to_rihs_string();
        hash_arms.push(quote! { #req_name => Ok(#req_hash.to_string()), });

        let resp_name = format!("{}/srv/{}_Response", srv.parsed.package, srv.parsed.name);
        let resp_hash = srv.response.type_hash.to_rihs_string();
        hash_arms.push(quote! { #resp_name => Ok(#resp_hash.to_string()), });
    }

    quote! {
        #[allow(clippy::useless_conversion, clippy::missing_safety_doc, unsafe_op_in_unsafe_fn)]
        mod helper_fns {
            use super::*;
            use ::pyo3::types::{PyBytes, PyBytesMethods};

            #[pyfunction]
            pub unsafe fn serialize_message(py: Python, type_name: &str, msg: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
                // Get registry from sys.modules (module is already loaded)
                let sys = py.import_bound("sys")?;
                let modules = sys.getattr("modules")?;
                let hiroz_msgs = modules.get_item("hiroz_py.hiroz_msgs")?;
                let registry = hiroz_msgs.getattr("REGISTRY")?;

                let msg_info = registry.get_item(type_name)
                    .map_err(|_| pyo3::exceptions::PyTypeError::new_err(
                        format!("Unknown message type: {}", type_name)
                    ))?;
                let serialize_fn = msg_info.get_item("serialize")?;
                let result = serialize_fn.call1((msg,))?;
                // Extract bytes efficiently using buffer protocol
                let py_bytes = result.downcast::<PyBytes>()?;
                Ok(py_bytes.as_bytes().to_vec())
            }

            #[pyfunction]
            pub unsafe fn deserialize_message(py: Python, type_name: &str, bytes: &[u8]) -> PyResult<PyObject> {
                // Get registry from sys.modules (module is already loaded)
                let sys = py.import_bound("sys")?;
                let modules = sys.getattr("modules")?;
                let hiroz_msgs = modules.get_item("hiroz_py.hiroz_msgs")?;
                let registry = hiroz_msgs.getattr("REGISTRY")?;

                let msg_info = registry.get_item(type_name)
                    .map_err(|_| pyo3::exceptions::PyTypeError::new_err(
                        format!("Unknown message type: {}", type_name)
                    ))?;
                let deserialize_fn = msg_info.get_item("deserialize")?;
                deserialize_fn.call1((bytes,)).map(|obj| obj.unbind())
            }

            /// Get list of all registered message types
            #[pyfunction]
            pub fn list_registered_types() -> Vec<String> {
                vec![
                    #(#type_names.to_string(),)*
                ]
            }

            /// Get the type hash for a given message type
            #[pyfunction]
            pub fn get_type_hash(type_name: &str) -> PyResult<String> {
                match type_name {
                    #(#hash_arms)*
                    _ => Err(pyo3::exceptions::PyValueError::new_err(
                        format!("Unknown message type: {}", type_name)
                    )),
                }
            }
        }

        pub use helper_fns::{serialize_message, deserialize_message, list_registered_types, get_type_hash};
    }
}

/// Generate serialize_to_zbuf function
fn generate_serialize_to_zbuf(
    packages: &HashMap<String, Vec<&ResolvedMessage>>,
    services: &[ResolvedService],
) -> TokenStream {
    let mut match_arms = Vec::new();

    // Generate match arms for message types
    for (package_name, package_msgs) in packages {
        let package_ident = format_ident!("{}", package_name.replace('-', "_"));
        for msg in package_msgs {
            let full_name = format!("{}/msg/{}", package_name, msg.parsed.name);
            let name_ident = format_ident!("{}", msg.parsed.name);

            match_arms.push(quote! {
                #full_name => {
                    let rust_msg = <ros::#package_ident::#name_ident>::from_py(msg)?;
                    Ok(rust_msg.serialize_to_zbuf())
                }
            });
        }
    }

    // Generate match arms for service Request/Response types
    for srv in services {
        let package_ident = format_ident!("{}", srv.parsed.package.replace('-', "_"));

        // Request
        let req_full_name = format!("{}/srv/{}_Request", srv.parsed.package, srv.parsed.name);
        let req_name_ident = format_ident!("{}Request", srv.parsed.name);
        match_arms.push(quote! {
            #req_full_name => {
                let rust_msg = <ros::#package_ident::#req_name_ident>::from_py(msg)?;
                Ok(rust_msg.serialize_to_zbuf())
            }
        });

        // Response
        let resp_full_name = format!("{}/srv/{}_Response", srv.parsed.package, srv.parsed.name);
        let resp_name_ident = format_ident!("{}Response", srv.parsed.name);
        match_arms.push(quote! {
            #resp_full_name => {
                let rust_msg = <ros::#package_ident::#resp_name_ident>::from_py(msg)?;
                Ok(rust_msg.serialize_to_zbuf())
            }
        });
    }

    quote! {
        /// Direct ZBuf serialization - bypasses Python registry for zero-copy performance
        pub fn serialize_to_zbuf(type_name: &str, msg: &Bound<'_, PyAny>) -> PyResult<::zenoh_buffers::ZBuf> {
            use ::hiroz::python_bridge::FromPyMessage;
            use ::hiroz::msg::ZMessage;
            match type_name {
                #(#match_arms)*
                _ => Err(pyo3::exceptions::PyValueError::new_err(
                    format!("Unknown message type: {}", type_name)
                )),
            }
        }
    }
}

fn generate_python_init(packages: &HashMap<String, Vec<&ResolvedMessage>>) -> Result<String> {
    let mut code =
        "\"\"\"Auto-generated ROS 2 message types package.\"\"\"\n\n# Import all message types\n"
            .to_string();

    for package_name in packages.keys() {
        code.push_str(&format!("from . import {}\n", package_name));
    }

    code.push_str("\n__all__ = [\n");
    for package_name in packages.keys() {
        code.push_str(&format!("    \"{}\",\n", package_name));
    }
    code.push_str("]\n");

    Ok(code)
}
