//! TokenStream-based code generation for Python msgspec extraction
//!
//! This module provides functions that generate Rust code using proc_macro2::TokenStream
//! and the quote! macro for extracting fields from Python msgspec.Struct objects.

use anyhow::Result;
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use roslibrust_codegen::{ArrayType, FieldInfo, MessageFile};
use std::collections::HashMap;

/// Escape Rust keywords (type, impl, etc.)
fn escape_rust_keyword(name: &str) -> String {
    match name {
        "type" | "struct" | "enum" | "fn" | "impl" | "trait" | "use" | "mod" | "pub" | "crate"
        | "super" | "self" => {
            format!("r#{}", name)
        }
        _ => name.to_string(),
    }
}

/// Generate extraction code for a single primitive field
pub fn generate_primitive_extraction(
    pyobj: &Ident,
    field: &FieldInfo,
) -> Result<TokenStream> {
    let field_name = &field.field_name;
    let escaped_name = format_ident!("{}", escape_rust_keyword(field_name));
    
    let rust_type = match field.field_type.field_type.as_str() {
        "bool" => quote! { bool },
        "int8" => quote! { i8 },
        "byte" | "char" | "uint8" => quote! { u8 },
        "int16" => quote! { i16 },
        "uint16" => quote! { u16 },
        "int32" => quote! { i32 },
        "uint32" => quote! { u32 },
        "int64" => quote! { i64 },
        "uint64" => quote! { u64 },
        "float32" => quote! { f32 },
        "float64" => quote! { f64 },
        "string" | "wstring" => quote! { String },
        _ => return Err(anyhow::anyhow!("Not a primitive type: {}", field.field_type.field_type)),
    };

    Ok(quote! {
        #escaped_name: #pyobj.getattr(#field_name)?.extract::<#rust_type>()?
    })
}

/// Generate extraction for Vec<T> or [T; N]
pub fn generate_array_extraction(
    pyobj: &Ident,
    field: &FieldInfo,
    allmessages: &HashMap<String, &MessageFile>,
) -> Result<TokenStream> {
    let field_name = &field.field_name;
    let escaped_name = format_ident!("{}", escape_rust_keyword(field_name));
    let base_type = &field.field_type.field_type;
    
    let is_fixed_size = matches!(
        field.field_type.array_info,
        ArrayType::FixedLength(_)
    );

    // Determine the element extraction code
    let element_extraction = match base_type.as_str() {
        "bool" => quote! { item.extract::<bool>()? },
        "int8" => quote! { item.extract::<i8>()? },
        "byte" | "char" | "uint8" => quote! { item.extract::<u8>()? },
        "int16" => quote! { item.extract::<i16>()? },
        "uint16" => quote! { item.extract::<u16>()? },
        "int32" => quote! { item.extract::<i32>()? },
        "uint32" => quote! { item.extract::<u32>()? },
        "int64" => quote! { item.extract::<i64>()? },
        "uint64" => quote! { item.extract::<u64>()? },
        "float32" => quote! { item.extract::<f32>()? },
        "float64" => quote! { item.extract::<f64>()? },
        "string" | "wstring" => quote! { item.extract::<String>()? },
        _ => {
            // Nested message in array
            let nested_msg_key = if let Some(ref package_name) = field.field_type.package_name {
                format!("{}/{}", package_name, base_type)
            } else {
                format!("{}/{}", field.field_type.source_package, base_type)
            };

            let nested_msg = allmessages
                .get(&nested_msg_key)
                .ok_or_else(|| anyhow::anyhow!("Message {} not found", nested_msg_key))?;

            let item_ident = format_ident!("item");
            generate_nested_extraction(&item_ident, nested_msg, allmessages)?
        }
    };

    if is_fixed_size {
        Ok(quote! {
            #escaped_name: {
                let pyattr = #pyobj.getattr(#field_name)?;
                let pylist = pyattr.downcast::<pyo3::types::PyList>()?;
                let mut vec = Vec::new();
                for item in pylist.iter() {
                    vec.push(#element_extraction);
                }
                vec.try_into().map_err(|_| pyo3::exceptions::PyValueError::new_err(
                    concat!("Array size mismatch for field '", #field_name, "'")))?
            }
        })
    } else {
        Ok(quote! {
            #escaped_name: {
                let pyattr = #pyobj.getattr(#field_name)?;
                let pylist = pyattr.downcast::<pyo3::types::PyList>()?;
                let mut vec = Vec::new();
                for item in pylist.iter() {
                    vec.push(#element_extraction);
                }
                vec
            }
        })
    }
}

/// Generate extraction for nested msgspec.Struct
pub fn generate_nested_extraction(
    pyobj: &Ident,
    msg: &MessageFile,
    allmessages: &HashMap<String, &MessageFile>,
) -> Result<TokenStream> {
    let struct_name = format_ident!("{}", msg.parsed.name);
    let package = format_ident!("{}", msg.parsed.package.replace("-", "_"));

    // Generate field extractions
    let field_extractions: Vec<TokenStream> = msg
        .parsed
        .fields
        .iter()
        .map(|field| generate_field_extraction(pyobj, field, allmessages))
        .collect::<Result<Vec<_>>>()?;

    Ok(quote! {
        crate::#package::#struct_name {
            #(#field_extractions),*
        }
    })
}

/// Dispatch: primitive, array, or nested
pub fn generate_field_extraction(
    pyobj: &Ident,
    field: &FieldInfo,
    allmessages: &HashMap<String, &MessageFile>,
) -> Result<TokenStream> {
    let is_array = !matches!(field.field_type.array_info, ArrayType::NotArray);
    let base_type = &field.field_type.field_type;

    if is_array {
        generate_array_extraction(pyobj, field, allmessages)
    } else {
        // Check if it's a primitive or nested message
        match base_type.as_str() {
            "bool" | "int8" | "byte" | "char" | "uint8" | "int16" | "uint16" | "int32"
            | "uint32" | "int64" | "uint64" | "float32" | "float64" | "string" | "wstring" => {
                generate_primitive_extraction(pyobj, field)
            }
            _ => {
                // Nested message
                let field_name = &field.field_name;
                let escaped_name = format_ident!("{}", escape_rust_keyword(field_name));
                let nested_obj = format_ident!("{}_obj", escape_rust_keyword(field_name));

                let nested_msg_key = if let Some(ref package_name) = field.field_type.package_name
                {
                    format!("{}/{}", package_name, base_type)
                } else {
                    format!("{}/{}", field.field_type.source_package, base_type)
                };

                let nested_msg = allmessages
                    .get(&nested_msg_key)
                    .ok_or_else(|| anyhow::anyhow!("Message {} not found", nested_msg_key))?;

                let nested_extraction = generate_nested_extraction(&nested_obj, nested_msg, allmessages)?;

                Ok(quote! {
                    #escaped_name: {
                        let #nested_obj = #pyobj.getattr(#field_name)?;
                        #nested_extraction
                    }
                })
            }
        }
    }
}

/// Generate complete #[pyfunction] serialize function using TokenStream
pub fn generate_serialize_function(
    msg: &MessageFile,
    allmessages: &HashMap<String, &MessageFile>,
) -> Result<TokenStream> {
    let func_name = format_ident!("serialize_{}", msg.parsed.name.to_lowercase());
    let _package = format_ident!("{}", msg.parsed.package.replace("-", "_"));
    let _struct_name = format_ident!("{}", msg.parsed.name);
    
    let msg_ident = format_ident!("msg");
    let extraction = generate_nested_extraction(&msg_ident, msg, allmessages)?;

    Ok(quote! {
        #[allow(unsafe_op_in_unsafe_fn, clippy::useless_conversion)]
        #[pyo3::pyfunction]
        pub fn #func_name(#msg_ident: pyo3::Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<Vec<u8>> {
            // Validate msgspec.Struct
            if !#msg_ident.hasattr("__msgtype__")? {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "Expected msgspec.Struct, got something else"
                ));
            }
            
            // Extract fields → Rust struct
            let rust_msg = #extraction;
            
            // Serialize using CDR
            cdr::serialize::<_, _, cdr::CdrLe>(&rust_msg, cdr::Infinite)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }
    })
}
