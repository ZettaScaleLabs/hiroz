use std::collections::HashSet;

use anyhow::Result;
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::types::{ArrayType, Field, FieldType, ResolvedMessage, ResolvedService};

/// Context for code generation, tracking external vs local packages
#[derive(Default, Clone)]
pub struct GenerationContext {
    /// External crate path for standard message types (e.g., "hiroz_msgs")
    pub external_crate: Option<String>,
    /// Set of local package names (packages being generated in this crate)
    pub local_packages: HashSet<String>,
}

impl GenerationContext {
    /// Create a new generation context
    pub fn new(external_crate: Option<String>, local_packages: HashSet<String>) -> Self {
        Self {
            external_crate,
            local_packages,
        }
    }

    /// Check if a package is local (being generated in this crate)
    pub fn is_local_package(&self, package: &str) -> bool {
        self.local_packages.is_empty() || self.local_packages.contains(package)
    }
}

// ── Plainness detection ───────────────────────────────────────────────────────

/// Returns true if the field type has a CDR wire layout identical to its
/// in-memory layout (i.e., it qualifies for `CdrPlain`).
///
/// A type is plain iff:
/// - It is a fixed-size numeric primitive (not bool, string, char/wchar).
/// - OR it is a nested struct that is itself plain (looked up in `plain_types`).
/// - AND it has no unbounded/bounded sequence dimension.
///
/// Fixed arrays of plain types are themselves plain.
pub fn is_field_plain(field_type: &FieldType, plain_types: &HashSet<String>) -> bool {
    // Sequences (Vec<T>) are never plain — they carry a variable-length prefix.
    if matches!(
        field_type.array,
        ArrayType::Unbounded | ArrayType::Bounded(_)
    ) {
        return false;
    }
    // base type check
    is_base_type_plain(
        &field_type.base_type,
        field_type.package.as_deref(),
        plain_types,
    )
}

fn is_base_type_plain(
    base_type: &str,
    package: Option<&str>,
    plain_types: &HashSet<String>,
) -> bool {
    match base_type {
        // bool: CDR bool is u8(0|1), bytemuck::Pod not impl'd for bool
        "bool" | "string" | "wstring" | "char" | "wchar" => false,
        "byte" | "uint8" | "int8" | "uint16" | "int16" | "uint32" | "int32" | "uint64"
        | "int64" | "float32" | "float64" => true,
        custom => {
            // Look up in the set of already-confirmed-plain struct types.
            let key = match package {
                Some(pkg) => format!("{}::{}", pkg, custom),
                None => custom.to_string(),
            };
            plain_types.contains(&key)
        }
    }
}

/// Returns the alignment (in bytes) of a primitive base type, or None for
/// non-primitive / custom types.
fn primitive_align(base_type: &str) -> Option<usize> {
    match base_type {
        "byte" | "uint8" | "int8" => Some(1),
        "uint16" | "int16" => Some(2),
        "uint32" | "int32" | "float32" => Some(4),
        "uint64" | "int64" | "float64" => Some(8),
        _ => None,
    }
}

/// Compute the set of plain struct types for a slice of resolved messages.
///
/// Returns a `HashSet<String>` where each entry is `"package::TypeName"`.
/// The computation is bottom-up: a struct is plain iff all its fields are plain
/// AND the struct has no inter-field or trailing padding in C/Rust repr(C).
///
/// Padding detection: a struct has no padding iff all fields share the same
/// alignment (or more precisely, every field's alignment divides the struct's
/// natural alignment uniformly). We use the conservative rule: all primitive
/// fields must have the same alignment. Nested plain structs are assumed to
/// already satisfy this invariant.
pub fn compute_plain_types(messages: &[ResolvedMessage]) -> HashSet<String> {
    // Iterate until stable (handles mutually-dependent plain structs, though rare).
    let mut plain: HashSet<String> = HashSet::new();
    loop {
        let before = plain.len();
        'msg: for msg in messages {
            let key = format!("{}::{}", msg.parsed.package, msg.parsed.name);
            if plain.contains(&key) {
                continue;
            }
            // All fields must individually be plain.
            if !msg
                .parsed
                .fields
                .iter()
                .all(|f| is_field_plain(&f.field_type, &plain))
            {
                continue;
            }
            // Additionally, all direct primitive fields must share the same alignment
            // to guarantee no inter-field or trailing padding.
            let mut max_align: Option<usize> = None;
            for field in &msg.parsed.fields {
                if let Some(align) = primitive_align(&field.field_type.base_type) {
                    match max_align {
                        None => max_align = Some(align),
                        Some(existing) if existing != align => continue 'msg, // mixed alignment → padding
                        _ => {}
                    }
                }
                // Custom (nested plain) types: their alignment is inherited from their
                // own max primitive. We don't re-check here — they are already validated.
            }
            plain.insert(key);
        }
        if plain.len() == before {
            break; // stable
        }
    }
    plain
}

// ── CDR trait codegen ─────────────────────────────────────────────────────────

/// Generate `CdrSerialize`, `CdrDeserialize`, `CdrSerializedSize` impls,
/// and (when the struct is plain) the `CdrPlain` + `bytemuck::Pod/Zeroable` derives.
fn generate_cdr_impls(
    msg: &ResolvedMessage,
    plain_types: &HashSet<String>,
    ctx: &GenerationContext,
) -> Result<TokenStream> {
    let name = format_ident!("{}", msg.parsed.name);
    let fields = &msg.parsed.fields;
    let pkg = &msg.parsed.package;
    let is_plain = plain_types.contains(&format!("{}::{}", pkg, &msg.parsed.name));

    // ── CdrSerialize ──────────────────────────────────────────────────────────
    let ser_fields: Vec<TokenStream> = fields
        .iter()
        .map(|f| generate_cdr_serialize_field(f, pkg, plain_types, ctx))
        .collect::<Result<Vec<_>>>()?;

    let ser_impl = quote! {
        impl ::hiroz_cdr::CdrSerialize for #name {
            fn cdr_serialize<BO, B>(
                &self,
                __w: &mut ::hiroz_cdr::CdrWriter<'_, BO, B>,
            )
            where
                BO: ::byteorder::ByteOrder,
                B: ::hiroz_cdr::CdrBuffer,
            {
                #(#ser_fields)*
            }
        }
    };

    // ── CdrDeserialize ────────────────────────────────────────────────────────
    let de_fields: Vec<TokenStream> = fields
        .iter()
        .map(|f| generate_cdr_deserialize_field(f, pkg, plain_types, ctx))
        .collect();

    let field_idents: Vec<Ident> = fields.iter().map(|f| escape_field_name(&f.name)).collect();

    let de_impl = quote! {
        impl ::hiroz_cdr::CdrDeserialize for #name {
            fn cdr_deserialize<'__de, BO>(
                __r: &mut ::hiroz_cdr::CdrReader<'__de, BO>,
            ) -> ::hiroz_cdr::Result<Self>
            where
                BO: ::byteorder::ByteOrder,
            {
                #(#de_fields)*
                Ok(Self { #(#field_idents),* })
            }
        }
    };

    // ── CdrSerializedSize ─────────────────────────────────────────────────────
    let size_fields: Vec<TokenStream> = fields
        .iter()
        .map(|f| generate_cdr_size_field(f, pkg, plain_types, ctx))
        .collect::<Result<Vec<_>>>()?;

    let size_impl = quote! {
        impl ::hiroz_cdr::CdrSerializedSize for #name {
            fn cdr_serialized_size(&self, __pos: usize) -> usize {
                let mut __p = __pos;
                #(#size_fields)*
                __p
            }
        }
    };

    // ── CdrPlain (only when struct is plain) ──────────────────────────────────
    let plain_impl = if is_plain {
        quote! {
            #[cfg(target_endian = "little")]
            unsafe impl ::hiroz_cdr::CdrPlain for #name {}
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #ser_impl
        #de_impl
        #size_impl
        #plain_impl
    })
}

/// Generate a single field's CdrSerialize statement.
fn generate_cdr_serialize_field(
    field: &Field,
    source_pkg: &str,
    plain_types: &HashSet<String>,
    _ctx: &GenerationContext,
) -> Result<TokenStream> {
    let fname = escape_field_name(&field.name);
    let ft = &field.field_type;

    // ZBuf fields: byte sequences stored as hiroz::ZBuf (zero-copy Zenoh type).
    // CdrSerialize is implemented for hiroz::ZBuf in the hiroz crate.
    if is_zbuf_field(field) {
        return Ok(quote! {
            ::hiroz_cdr::CdrSerialize::cdr_serialize(&self.#fname, __w);
        });
    }

    match &ft.array {
        ArrayType::Single => Ok(quote! {
            ::hiroz_cdr::CdrSerialize::cdr_serialize(&self.#fname, __w);
        }),
        ArrayType::Fixed(_) => {
            let elem_plain = is_base_type_plain(
                &ft.base_type,
                ft.package.as_deref().or(Some(source_pkg)),
                plain_types,
            );
            if elem_plain && !matches!(ft.base_type.as_str(), "bool" | "string" | "wstring") {
                Ok(quote! {
                    #[cfg(target_endian = "little")]
                    __w.write_pod_slice(&self.#fname);
                    #[cfg(not(target_endian = "little"))]
                    for __item in &self.#fname {
                        ::hiroz_cdr::CdrSerialize::cdr_serialize(__item, __w);
                    }
                })
            } else {
                Ok(quote! {
                    for __item in &self.#fname {
                        ::hiroz_cdr::CdrSerialize::cdr_serialize(__item, __w);
                    }
                })
            }
        }
        ArrayType::Unbounded | ArrayType::Bounded(_) => {
            // Check if element type is plain → bulk path
            let elem_plain = is_base_type_plain(
                &ft.base_type,
                ft.package.as_deref().or(Some(source_pkg)),
                plain_types,
            );
            if elem_plain && !matches!(ft.base_type.as_str(), "bool" | "string" | "wstring") {
                Ok(quote! {
                    __w.write_sequence_length(self.#fname.len());
                    #[cfg(target_endian = "little")]
                    if !self.#fname.is_empty() {
                        __w.write_pod_slice(&self.#fname);
                    }
                    #[cfg(not(target_endian = "little"))]
                    for __item in &self.#fname {
                        ::hiroz_cdr::CdrSerialize::cdr_serialize(__item, __w);
                    }
                })
            } else {
                Ok(quote! {
                    __w.write_sequence_length(self.#fname.len());
                    for __item in &self.#fname {
                        ::hiroz_cdr::CdrSerialize::cdr_serialize(__item, __w);
                    }
                })
            }
        }
    }
}

/// Generate a single field's CdrDeserialize statement (binds a local variable).
fn generate_cdr_deserialize_field(
    field: &Field,
    source_pkg: &str,
    plain_types: &HashSet<String>,
    ctx: &GenerationContext,
) -> TokenStream {
    let fname = escape_field_name(&field.name);
    let ft = &field.field_type;
    let rust_elem_ty = generate_field_type_tokens_with_context(ft, source_pkg, ctx);

    // ZBuf: CdrDeserialize is implemented for hiroz::ZBuf in the hiroz crate.
    if is_zbuf_field(field) {
        return quote! {
            let #fname: #rust_elem_ty = ::hiroz_cdr::CdrDeserialize::cdr_deserialize(__r)?;
        };
    }

    match &ft.array {
        ArrayType::Single => quote! {
            let #fname = ::hiroz_cdr::CdrDeserialize::cdr_deserialize(__r)?;
        },
        ArrayType::Fixed(n) => {
            let n_lit = proc_macro2::Literal::usize_unsuffixed(*n);
            let elem_plain = is_base_type_plain(
                &ft.base_type,
                ft.package.as_deref().or(Some(source_pkg)),
                plain_types,
            );
            let base_ty = generate_base_type_tokens_with_context(ft, source_pkg, ctx);
            if elem_plain && !matches!(ft.base_type.as_str(), "bool" | "string" | "wstring") {
                quote! {
                    let #fname: #rust_elem_ty = {
                        #[cfg(target_endian = "little")]
                        {
                            let __slice = __r.read_pod_slice::<#base_ty>(#n_lit)?;
                            ::std::convert::TryInto::try_into(__slice)
                                .map_err(|_| ::hiroz_cdr::Error::UnexpectedEof)?
                        }
                        #[cfg(not(target_endian = "little"))]
                        {
                            let mut __arr = [Default::default(); #n_lit];
                            for __slot in __arr.iter_mut() {
                                *__slot = ::hiroz_cdr::CdrDeserialize::cdr_deserialize(__r)?;
                            }
                            __arr
                        }
                    };
                }
            } else {
                quote! {
                    let #fname: #rust_elem_ty = {
                        let mut __arr = [Default::default(); #n_lit];
                        for __slot in __arr.iter_mut() {
                            *__slot = ::hiroz_cdr::CdrDeserialize::cdr_deserialize(__r)?;
                        }
                        __arr
                    };
                }
            }
        }
        ArrayType::Unbounded | ArrayType::Bounded(_) => {
            let elem_plain = is_base_type_plain(
                &ft.base_type,
                ft.package.as_deref().or(Some(source_pkg)),
                plain_types,
            );
            let base_ty = generate_base_type_tokens_with_context(ft, source_pkg, ctx);
            if elem_plain && !matches!(ft.base_type.as_str(), "bool" | "string" | "wstring") {
                quote! {
                    let #fname: Vec<#base_ty> = {
                        let __count = __r.read_sequence_length()?;
                        #[cfg(target_endian = "little")]
                        {
                            if __count > 0 {
                                __r.read_pod_slice::<#base_ty>(__count)?
                            } else {
                                vec![]
                            }
                        }
                        #[cfg(not(target_endian = "little"))]
                        {
                            let mut __v = Vec::with_capacity(__count);
                            for _ in 0..__count {
                                __v.push(::hiroz_cdr::CdrDeserialize::cdr_deserialize(__r)?);
                            }
                            __v
                        }
                    };
                }
            } else {
                quote! {
                    let #fname: Vec<#base_ty> = {
                        let __count = __r.read_sequence_length()?;
                        let mut __v = Vec::with_capacity(__count);
                        for _ in 0..__count {
                            __v.push(::hiroz_cdr::CdrDeserialize::cdr_deserialize(__r)?);
                        }
                        __v
                    };
                }
            }
        }
    }
}

/// Generate a single field's CdrSerializedSize statement (updates `__p`).
fn generate_cdr_size_field(
    field: &Field,
    source_pkg: &str,
    plain_types: &HashSet<String>,
    _ctx: &GenerationContext,
) -> Result<TokenStream> {
    let fname = escape_field_name(&field.name);
    let ft = &field.field_type;

    // ZBuf: u32 length prefix (4-byte aligned) + byte contents.
    if is_zbuf_field(field) {
        return Ok(quote! {
            {
                use ::zenoh_buffers::buffer::Buffer;
                __p += (__p % 4 != 0) as usize * (4 - __p % 4) + 4;
                __p += self.#fname.len();
            }
        });
    }

    match &ft.array {
        ArrayType::Single => Ok(quote! {
            __p = ::hiroz_cdr::CdrSerializedSize::cdr_serialized_size(&self.#fname, __p);
        }),
        ArrayType::Fixed(_) => {
            let elem_plain = is_base_type_plain(
                &ft.base_type,
                ft.package.as_deref().or(Some(source_pkg)),
                plain_types,
            );
            if elem_plain && !matches!(ft.base_type.as_str(), "bool" | "string" | "wstring") {
                // O(1) size for plain fixed arrays: align + N * sizeof(T)
                Ok(quote! {
                    if !self.#fname.is_empty() {
                        let __elem_align = ::std::mem::align_of_val(&self.#fname[0]);
                        __p += (__p % __elem_align != 0) as usize
                            * (__elem_align - __p % __elem_align);
                        __p += self.#fname.len()
                            * ::std::mem::size_of_val(&self.#fname[0]);
                    }
                })
            } else {
                Ok(quote! {
                    for __item in &self.#fname {
                        __p = ::hiroz_cdr::CdrSerializedSize::cdr_serialized_size(__item, __p);
                    }
                })
            }
        }
        ArrayType::Unbounded | ArrayType::Bounded(_) => {
            let elem_plain = is_base_type_plain(
                &ft.base_type,
                ft.package.as_deref().or(Some(source_pkg)),
                plain_types,
            );
            if elem_plain && !matches!(ft.base_type.as_str(), "bool" | "string" | "wstring") {
                // O(1) size for plain sequences: align + count * sizeof(T)
                Ok(quote! {
                    // u32 sequence length prefix
                    __p += (__p % 4 != 0) as usize * (4 - __p % 4) + 4;
                    if !self.#fname.is_empty() {
                        let __elem_align = ::std::mem::align_of_val(&self.#fname[0]);
                        __p += (__p % __elem_align != 0) as usize
                            * (__elem_align - __p % __elem_align);
                        __p += self.#fname.len()
                            * ::std::mem::size_of_val(&self.#fname[0]);
                    }
                })
            } else {
                Ok(quote! {
                    // u32 sequence length prefix
                    __p += (__p % 4 != 0) as usize * (4 - __p % 4) + 4;
                    for __item in &self.#fname {
                        __p = ::hiroz_cdr::CdrSerializedSize::cdr_serialized_size(__item, __p);
                    }
                })
            }
        }
    }
}

/// Generate Rust module for a package containing messages
pub fn generate_package_module(package: &str, messages: &[ResolvedMessage]) -> Result<TokenStream> {
    let package_ident = format_ident!("{}", package);
    let message_impls: Vec<TokenStream> = messages
        .iter()
        .map(generate_message_impl)
        .collect::<Result<Vec<_>>>()?;

    Ok(quote! {
        pub mod #package_ident {
            #(#message_impls)*
        }
    })
}

/// Generate Rust implementation for a single message
pub fn generate_message_impl(msg: &ResolvedMessage) -> Result<TokenStream> {
    generate_message_impl_with_context(msg, &GenerationContext::default())
}

/// Generate Rust implementation for a single message with external type support
pub fn generate_message_impl_with_context(
    msg: &ResolvedMessage,
    ctx: &GenerationContext,
) -> Result<TokenStream> {
    generate_message_impl_with_cdr(msg, ctx, &HashSet::new())
}

/// Generate Rust implementation with CDR trait impls and plainness information.
pub fn generate_message_impl_with_cdr(
    msg: &ResolvedMessage,
    ctx: &GenerationContext,
    plain_types: &HashSet<String>,
) -> Result<TokenStream> {
    let name = format_ident!("{}", msg.parsed.name);

    let msg_is_plain =
        plain_types.contains(&format!("{}::{}", msg.parsed.package, msg.parsed.name));
    let struct_def = generate_struct_with_context(
        &msg.parsed.package,
        &msg.parsed.name,
        &msg.parsed.fields,
        &msg.parsed.constants,
        ctx,
        msg_is_plain,
    )?;
    let type_info = generate_message_type_info(
        &msg.parsed.package,
        &msg.parsed.name,
        &msg.type_hash,
        &msg.parsed.fields,
        ctx,
    );

    let size_estimation_impl =
        generate_size_estimation_impl(&name, &msg.parsed.fields, &msg.parsed.package, ctx)?;

    let cdr_impls = generate_cdr_impls(msg, plain_types, ctx)?;

    Ok(quote! {
        #struct_def
        #type_info
        #size_estimation_impl
        #cdr_impls
    })
}

/// Generate struct definition with constants
#[allow(dead_code)]
fn generate_struct(
    package: &str,
    name: &str,
    fields: &[Field],
    constants: &[crate::types::Constant],
) -> Result<TokenStream> {
    generate_struct_with_context(
        package,
        name,
        fields,
        constants,
        &GenerationContext::default(),
        false,
    )
}

/// Generate struct definition with constants (with external type support)
fn generate_struct_with_context(
    package: &str,
    name: &str,
    fields: &[Field],
    constants: &[crate::types::Constant],
    ctx: &GenerationContext,
    is_plain: bool,
) -> Result<TokenStream> {
    let name_ident = format_ident!("{}", name);
    let field_defs: Vec<TokenStream> = fields
        .iter()
        .map(|f| generate_field_def_with_context(f, package, ctx))
        .collect::<Result<Vec<_>>>()?;

    // Check if we have large arrays (>32 elements) that need smart-default
    let has_large_array = fields
        .iter()
        .any(|f| matches!(&f.field_type.array, ArrayType::Fixed(n) if *n > 32));

    // Generate constants as associated constants
    let const_defs: Vec<TokenStream> = constants
        .iter()
        .map(|c| {
            let const_name = format_ident!("{}", c.name);
            let const_type = generate_constant_type(&c.const_type);
            let const_value = generate_constant_value_from_string(&c.const_type, &c.value);
            quote! {
                pub const #const_name: #const_type = #const_value;
            }
        })
        .collect();

    // Python bridge module path for derive macros
    let py_module_path = format!("hiroz_msgs_py.types.{}", package);

    let bytemuck_derives = if is_plain {
        quote! {
            #[cfg_attr(target_endian = "little", repr(C))]
            #[cfg_attr(target_endian = "little", derive(Copy, ::bytemuck::Pod, ::bytemuck::Zeroable))]
        }
    } else {
        quote! {}
    };

    if has_large_array {
        // Large array messages need smart-default for arrays >32 elements
        Ok(quote! {
            #[derive(Debug, Clone, ::smart_default::SmartDefault, ::serde::Serialize, ::serde::Deserialize)]
            #[cfg_attr(feature = "python_registry", derive(::hiroz_derive::FromPyMessage, ::hiroz_derive::IntoPyMessage))]
            #[cfg_attr(feature = "python_registry", ros_msg(module = #py_module_path))]
            #bytemuck_derives
            pub struct #name_ident {
                #(#field_defs),*
            }

            impl #name_ident {
                #(#const_defs)*
            }
        })
    } else {
        // Simple messages with standard Default
        Ok(quote! {
            #[derive(Debug, Clone, Default, ::serde::Serialize, ::serde::Deserialize)]
            #[cfg_attr(feature = "python_registry", derive(::hiroz_derive::FromPyMessage, ::hiroz_derive::IntoPyMessage))]
            #[cfg_attr(feature = "python_registry", ros_msg(module = #py_module_path))]
            #bytemuck_derives
            pub struct #name_ident {
                #(#field_defs),*
            }

            impl #name_ident {
                #(#const_defs)*
            }
        })
    }
}

/// Generate constant type tokens
fn generate_constant_type(const_type: &str) -> TokenStream {
    match const_type {
        "bool" => quote! { bool },
        "byte" | "uint8" => quote! { u8 },
        "char" | "int8" => quote! { i8 },
        "uint16" => quote! { u16 },
        "int16" => quote! { i16 },
        "uint32" => quote! { u32 },
        "int32" => quote! { i32 },
        "uint64" => quote! { u64 },
        "int64" => quote! { i64 },
        "float32" => quote! { f32 },
        "float64" => quote! { f64 },
        "string" => quote! { &'static str },
        _ => quote! { &'static str },
    }
}

/// Generate constant value tokens from string representation
fn generate_constant_value_from_string(const_type: &str, value: &str) -> TokenStream {
    match const_type {
        "bool" => {
            let b: bool = value.parse().unwrap_or(false);
            quote! { #b }
        }
        "byte" | "uint8" | "char" | "int8" | "uint16" | "int16" | "uint32" | "int32" | "uint64"
        | "int64" => {
            let lit = syn::parse_str::<syn::LitInt>(value).unwrap();
            quote! { #lit }
        }
        "float32" | "float64" => {
            let lit = syn::parse_str::<syn::LitFloat>(value).unwrap();
            quote! { #lit }
        }
        _ => quote! { #value },
    }
}

/// Generate a field definition
#[allow(dead_code)]
fn generate_field_def(field: &Field, source_package: &str) -> Result<TokenStream> {
    generate_field_def_with_context(field, source_package, &GenerationContext::default())
}

/// Generate a field definition (with external type support)
fn generate_field_def_with_context(
    field: &Field,
    source_package: &str,
    ctx: &GenerationContext,
) -> Result<TokenStream> {
    // Escape Rust keywords with r# prefix
    let name = escape_field_name(&field.name);
    let field_type =
        generate_field_type_tokens_with_context(&field.field_type, source_package, ctx);

    // Check if this is a ZBuf field (for Python bridge attribute)
    let is_zbuf = is_zbuf_field(field);

    // Add attributes for large fixed arrays (>32 elements)
    let serde_attributes = if let ArrayType::Fixed(n) = &field.field_type.array {
        if *n > 32 {
            // Generate a default value code string for the array
            let default_code = generate_array_default_code(&field.field_type, *n)?;
            quote! {
                #[serde(with = "serde_big_array::BigArray")]
                #[default(_code = #default_code)]
            }
        } else {
            quote! {}
        }
    } else {
        quote! {}
    };

    // Add Python bridge attribute for ZBuf fields
    let python_attributes = if is_zbuf {
        quote! {
            #[cfg_attr(feature = "python_registry", ros_msg(zbuf))]
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #serde_attributes
        #python_attributes
        pub #name: #field_type
    })
}

/// Generate default value code string for an array
fn generate_array_default_code(field_type: &FieldType, size: usize) -> Result<String> {
    let elem_default = match field_type.base_type.as_str() {
        "bool" => "false",
        "byte" | "uint8" | "char" | "int8" | "uint16" | "int16" | "uint32" | "int32" | "uint64"
        | "int64" => "0",
        "float32" | "float64" => "0.0",
        _ => anyhow::bail!(
            "Cannot generate default for large array of type {}",
            field_type.base_type
        ),
    };

    Ok(format!("[{}; {}]", elem_default, size))
}

/// Check if a name is a Rust keyword
fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
    )
}

/// Escape a field name if it's a Rust keyword
fn escape_field_name(name: &str) -> Ident {
    if is_rust_keyword(name) {
        format_ident!("r#{}", name)
    } else {
        format_ident!("{}", name)
    }
}

/// Generate Rust type tokens for a field type
#[allow(dead_code)]
fn generate_field_type_tokens(field_type: &FieldType, source_package: &str) -> TokenStream {
    generate_field_type_tokens_with_context(
        field_type,
        source_package,
        &GenerationContext::default(),
    )
}

/// Generate Rust type tokens for a field type (with external type support)
fn generate_field_type_tokens_with_context(
    field_type: &FieldType,
    source_package: &str,
    ctx: &GenerationContext,
) -> TokenStream {
    let base = generate_base_type_tokens_with_context(field_type, source_package, ctx);

    match &field_type.array {
        ArrayType::Single => base,
        ArrayType::Fixed(n) => {
            let n_lit = proc_macro2::Literal::usize_unsuffixed(*n);
            quote! { [#base; #n_lit] }
        }
        ArrayType::Unbounded => {
            // Use hiroz::ZBuf wrapper for uint8[]/byte[] (zero-copy with optimized serde)
            if matches!(field_type.base_type.as_str(), "uint8" | "byte") {
                quote! { ::hiroz::ZBuf }
            } else {
                quote! { ::std::vec::Vec<#base> }
            }
        }
        ArrayType::Bounded(_n) => {
            // Using Vec<T> for bounded arrays (standard Rust approach)
            // Custom bounded type could be added for strict memory guarantees if needed
            quote! { ::std::vec::Vec<#base> }
        }
    }
}

/// Generate base type tokens
#[allow(dead_code)]
fn generate_base_type_tokens(field_type: &FieldType, source_package: &str) -> TokenStream {
    generate_base_type_tokens_with_context(
        field_type,
        source_package,
        &GenerationContext::default(),
    )
}

/// Generate base type tokens (with external type support)
fn generate_base_type_tokens_with_context(
    field_type: &FieldType,
    source_package: &str,
    ctx: &GenerationContext,
) -> TokenStream {
    match field_type.base_type.as_str() {
        "bool" => quote! { bool },
        "byte" | "uint8" => quote! { u8 },
        "char" | "int8" => quote! { i8 },
        "uint16" => quote! { u16 },
        "int16" => quote! { i16 },
        "uint32" => quote! { u32 },
        "int32" => quote! { i32 },
        "uint64" => quote! { u64 },
        "int64" => quote! { i64 },
        "float32" => quote! { f32 },
        "float64" => quote! { f64 },
        "string" => quote! { ::std::string::String },
        custom => {
            // Use explicit package or infer same package
            let pkg = field_type.package.as_deref().unwrap_or(source_package);
            let pkg_ident = format_ident!("{}", pkg);
            let type_ident = format_ident!("{}", custom);

            // Check if this is an external package reference
            if let Some(ref ext_crate) = ctx.external_crate
                && !ctx.is_local_package(pkg)
            {
                // External package - use fully qualified path
                let crate_ident = format_ident!("{}", ext_crate);
                return quote! { ::#crate_ident::ros::#pkg_ident::#type_ident };
            }

            // Local package - use super:: as before
            quote! { super::#pkg_ident::#type_ident }
        }
    }
}

/// Generate MessageTypeInfo trait implementation
fn generate_message_type_info(
    package: &str,
    name: &str,
    type_hash: &crate::types::TypeHash,
    fields: &[Field],
    ctx: &GenerationContext,
) -> TokenStream {
    let name_ident = format_ident!("{}", name);
    let type_name = format!("{}::msg::dds_::{}_", package, name);
    let schema_type_name = format!("{}/msg/{}", package, name);
    let hash_str = type_hash.to_rihs_string();
    let schema_field_tokens: Vec<TokenStream> = fields
        .iter()
        .map(|field| generate_schema_builder_field_tokens(field, package, ctx))
        .collect();

    quote! {
        impl ::hiroz::MessageTypeInfo for #name_ident {
            fn type_name() -> &'static str {
                #type_name
            }

            fn type_hash() -> ::hiroz::entity::TypeHash {
                ::hiroz::entity::TypeHash::from_rihs_string(#hash_str)
                    .expect("Invalid RIHS hash")
            }

            fn message_schema() -> Option<::std::sync::Arc<::hiroz::dynamic::MessageSchema>> {
                static SCHEMA: ::std::sync::OnceLock<::std::sync::Arc<::hiroz::dynamic::MessageSchema>> =
                    ::std::sync::OnceLock::new();

                Some(
                    SCHEMA
                        .get_or_init(|| {
                            ::hiroz::dynamic::MessageSchema::builder(#schema_type_name)
                                #(#schema_field_tokens)*
                                .build()
                                .expect("generated message schema must be valid")
                        })
                        .clone(),
                )
            }
        }

        impl ::hiroz::ros_msg::WithTypeInfo for #name_ident {}
    }
}

/// Generate schema builder tokens for one message field.
fn generate_schema_builder_field_tokens(
    field: &Field,
    source_package: &str,
    ctx: &GenerationContext,
) -> TokenStream {
    let field_type = generate_schema_field_type_tokens(&field.field_type, source_package, ctx);
    let field_name = &field.name;
    quote! { .field(#field_name, #field_type) }
}

/// Generate runtime dynamic `FieldType` tokens for one field.
fn generate_schema_field_type_tokens(
    field_type: &FieldType,
    source_package: &str,
    ctx: &GenerationContext,
) -> TokenStream {
    let base = generate_schema_base_field_type_tokens(field_type, source_package, ctx);

    match &field_type.array {
        ArrayType::Single => base,
        ArrayType::Fixed(n) => {
            quote! { ::hiroz::dynamic::FieldType::Array(::std::boxed::Box::new(#base), #n) }
        }
        ArrayType::Unbounded => {
            quote! { ::hiroz::dynamic::FieldType::Sequence(::std::boxed::Box::new(#base)) }
        }
        ArrayType::Bounded(n) => {
            quote! {
                ::hiroz::dynamic::FieldType::BoundedSequence(::std::boxed::Box::new(#base), #n)
            }
        }
    }
}

/// Generate runtime dynamic `FieldType` tokens for the non-array base type.
fn generate_schema_base_field_type_tokens(
    field_type: &FieldType,
    source_package: &str,
    ctx: &GenerationContext,
) -> TokenStream {
    match field_type.base_type.as_str() {
        "bool" => quote! { ::hiroz::dynamic::FieldType::Bool },
        "byte" | "uint8" | "char" => quote! { ::hiroz::dynamic::FieldType::Uint8 },
        "int8" => quote! { ::hiroz::dynamic::FieldType::Int8 },
        "uint16" | "wchar" => quote! { ::hiroz::dynamic::FieldType::Uint16 },
        "int16" => quote! { ::hiroz::dynamic::FieldType::Int16 },
        "uint32" => quote! { ::hiroz::dynamic::FieldType::Uint32 },
        "int32" => quote! { ::hiroz::dynamic::FieldType::Int32 },
        "uint64" => quote! { ::hiroz::dynamic::FieldType::Uint64 },
        "int64" => quote! { ::hiroz::dynamic::FieldType::Int64 },
        "float32" => quote! { ::hiroz::dynamic::FieldType::Float32 },
        "float64" => quote! { ::hiroz::dynamic::FieldType::Float64 },
        "string" => {
            if let Some(bound) = field_type.string_bound {
                quote! { ::hiroz::dynamic::FieldType::BoundedString(#bound) }
            } else {
                quote! { ::hiroz::dynamic::FieldType::String }
            }
        }
        _ => {
            let nested_type =
                generate_base_type_tokens_with_context(field_type, source_package, ctx);
            quote! {
                ::hiroz::dynamic::FieldType::Message(
                    <#nested_type as ::hiroz::MessageTypeInfo>::message_schema()
                        .expect("generated nested message schema must be available")
                )
            }
        }
    }
}

/// Generate accurate size estimation implementation for SHM serialization
fn generate_size_estimation_impl(
    name: &Ident,
    fields: &[Field],
    source_package: &str,
    ctx: &GenerationContext,
) -> Result<TokenStream> {
    // Always generate size estimation for all messages (even fixed-size ones)
    // This ensures nested messages can call estimated_cdr_size() on their fields

    // Generate size calculation for each field
    let field_size_exprs: Vec<TokenStream> = fields
        .iter()
        .map(|f| generate_field_size_expr(f, source_package, ctx))
        .collect::<Result<Vec<_>>>()?;

    // Only mark size as mutable if we have fields to add
    let size_decl = if fields.is_empty() {
        quote! { let size = 0usize; }
    } else {
        quote! { let mut size = 0usize; }
    };

    Ok(quote! {
        impl crate::size_estimation::SizeEstimation for #name {
            fn estimated_cdr_size(&self) -> usize {
                #size_decl
                #(#field_size_exprs)*
                size + 16  // Conservative alignment padding
            }
        }

        impl #name {
            /// Get an accurate estimate of the serialized CDR size.
            ///
            /// This implementation accounts for dynamic fields (Vec, String, ZBuf)
            /// and provides a conservative but accurate upper bound for SHM allocation.
            pub fn estimated_serialized_size(&self) -> usize {
                4 + crate::size_estimation::SizeEstimation::estimated_cdr_size(self)  // 4 for CDR header
            }
        }
    })
}

/// Generate size calculation expression for a single field
fn generate_field_size_expr(
    field: &Field,
    _source_package: &str,
    _ctx: &GenerationContext,
) -> Result<TokenStream> {
    let field_name = escape_field_name(&field.name);

    // Handle different field types
    if is_zbuf_field(field) {
        // ZBuf: 4 bytes length prefix + data length
        // Note: hiroz::ZBuf derefs to zenoh_buffers::ZBuf, so .len() works via Deref
        Ok(quote! {
            size += 4 + {
                use ::zenoh_buffers::buffer::Buffer;
                self.#field_name.len()
            };
        })
    } else if field.field_type.base_type == "string" {
        match &field.field_type.array {
            ArrayType::Single => {
                // Single string: 4 bytes length prefix + string length
                Ok(quote! {
                    size += 4 + self.#field_name.len();
                })
            }
            ArrayType::Unbounded | ArrayType::Bounded(_) => {
                // Vec<String>: 4 bytes vec length + (4 + len) for each string
                Ok(quote! {
                    size += 4;
                    for s in &self.#field_name {
                        size += 4 + s.len();
                    }
                })
            }
            ArrayType::Fixed(_) => {
                // Fixed array of strings
                Ok(quote! {
                    for s in &self.#field_name {
                        size += 4 + s.len();
                    }
                })
            }
        }
    } else if is_primitive_type(&field.field_type.base_type) {
        // Primitive type
        let elem_size = get_primitive_size(&field.field_type.base_type)?;

        match &field.field_type.array {
            ArrayType::Single => {
                // Single primitive
                Ok(quote! {
                    size += #elem_size;
                })
            }
            ArrayType::Unbounded | ArrayType::Bounded(_) => {
                // Vec<primitive>: 4 bytes length + elements
                if elem_size == 1 {
                    // Optimization: for byte arrays, no need to multiply by 1
                    Ok(quote! {
                        size += 4 + self.#field_name.len();
                    })
                } else {
                    Ok(quote! {
                        size += 4 + (self.#field_name.len() * #elem_size);
                    })
                }
            }
            ArrayType::Fixed(n) => {
                // Fixed array: just the elements
                let total_size = n * elem_size;
                Ok(quote! {
                    size += #total_size;
                })
            }
        }
    } else {
        // Nested message type (anything not primitive or string)
        match &field.field_type.array {
            ArrayType::Single => {
                // Single nested message
                Ok(quote! {
                    size += crate::size_estimation::SizeEstimation::estimated_cdr_size(&self.#field_name);
                })
            }
            ArrayType::Unbounded | ArrayType::Bounded(_) => {
                // Vec<NestedType>
                Ok(quote! {
                    size += 4;
                    for item in &self.#field_name {
                        size += crate::size_estimation::SizeEstimation::estimated_cdr_size(item);
                    }
                })
            }
            ArrayType::Fixed(_) => {
                // Fixed array of nested messages
                Ok(quote! {
                    for item in &self.#field_name {
                        size += crate::size_estimation::SizeEstimation::estimated_cdr_size(item);
                    }
                })
            }
        }
    }
}

/// Check if a type is a primitive
fn is_primitive_type(base_type: &str) -> bool {
    matches!(
        base_type,
        "bool"
            | "byte"
            | "uint8"
            | "int8"
            | "char"
            | "uint16"
            | "int16"
            | "wchar"
            | "uint32"
            | "int32"
            | "float32"
            | "uint64"
            | "int64"
            | "float64"
    )
}

/// Get the size in bytes of a primitive type
fn get_primitive_size(base_type: &str) -> Result<usize> {
    Ok(match base_type {
        "bool" | "byte" | "uint8" | "int8" | "char" => 1,
        "uint16" | "int16" | "wchar" => 2,
        "uint32" | "int32" | "float32" => 4,
        "uint64" | "int64" | "float64" => 8,
        _ => anyhow::bail!("Unknown primitive type: {}", base_type),
    })
}

/// Generate service type implementation
pub fn generate_service_impl(srv: &ResolvedService) -> Result<TokenStream> {
    let name = format_ident!("{}", srv.parsed.name);
    let request_type = format_ident!("{}Request", srv.parsed.name);
    let response_type = format_ident!("{}Response", srv.parsed.name);
    let service_type_name = format!("{}::srv::dds_::{}_", srv.parsed.package, srv.parsed.name);
    let hash_str = srv.type_hash.to_rihs_string();

    Ok(quote! {
        pub struct #name;

        impl ::hiroz::msg::ZService for #name {
            type Request = super::#request_type;
            type Response = super::#response_type;
        }

        impl ::hiroz::ServiceTypeInfo for #name {
            fn service_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    #service_type_name,
                    ::hiroz::entity::TypeHash::from_rihs_string(#hash_str)
                        .expect("Invalid RIHS hash")
                )
            }
        }
    })
}

/// Generate action type implementation
/// Actions are generated similarly to services - the struct is at the root level,
/// while Goal/Result/Feedback types are in the package module
pub fn generate_action_impl(action: &crate::types::ResolvedAction) -> Result<TokenStream> {
    let name = format_ident!("{}", action.parsed.name);
    let goal_type = format_ident!("{}Goal", action.parsed.name);
    let result_type = format_ident!("{}Result", action.parsed.name);
    let feedback_type = format_ident!("{}Feedback", action.parsed.name);
    let action_type_name = format!(
        "{}::action::dds_::{}_",
        action.parsed.package, action.parsed.name
    );
    let hash_str = action.type_hash.to_rihs_string();

    // Type names for action-related services and messages (ROS2 format with underscore)
    let send_goal_type_name = format!(
        "{}::action::dds_::{}_SendGoal_",
        action.parsed.package, action.parsed.name
    );
    let get_result_type_name = format!(
        "{}::action::dds_::{}_GetResult_",
        action.parsed.package, action.parsed.name
    );
    let feedback_msg_type_name = format!(
        "{}::action::dds_::{}_FeedbackMessage_",
        action.parsed.package, action.parsed.name
    );

    // Get type hashes from resolved action service hashes (not the Goal/Result/Feedback hashes)
    let send_goal_hash_str = action.send_goal_hash.to_rihs_string();
    let get_result_hash_str = action.get_result_hash.to_rihs_string();
    let feedback_hash_str = action.feedback_message_hash.to_rihs_string();
    let cancel_goal_hash_str = action.cancel_goal_hash.to_rihs_string();
    let status_hash_str = action.status_hash.to_rihs_string();

    Ok(quote! {
        pub struct #name;

        impl ::hiroz::action::ZAction for #name {
            type Goal = super::#goal_type;
            type Result = super::#result_type;
            type Feedback = super::#feedback_type;

            fn name() -> &'static str {
                #action_type_name
            }

            fn send_goal_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    #send_goal_type_name,
                    ::hiroz::entity::TypeHash::from_rihs_string(#send_goal_hash_str)
                        .expect("Invalid RIHS hash")
                )
            }

            fn get_result_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    #get_result_type_name,
                    ::hiroz::entity::TypeHash::from_rihs_string(#get_result_hash_str)
                        .expect("Invalid RIHS hash")
                )
            }

            fn cancel_goal_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    "action_msgs::srv::dds_::CancelGoal_",
                    ::hiroz::entity::TypeHash::from_rihs_string(#cancel_goal_hash_str)
                        .expect("Invalid RIHS hash")
                )
            }

            fn feedback_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    #feedback_msg_type_name,
                    ::hiroz::entity::TypeHash::from_rihs_string(#feedback_hash_str)
                        .expect("Invalid RIHS hash")
                )
            }

            fn status_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    "action_msgs::msg::dds_::GoalStatusArray_",
                    ::hiroz::entity::TypeHash::from_rihs_string(#status_hash_str)
                        .expect("Invalid RIHS hash")
                )
            }
        }

        impl ::hiroz::ActionTypeInfo for #name {
            fn action_type_info() -> ::hiroz::entity::TypeInfo {
                ::hiroz::entity::TypeInfo::new(
                    #action_type_name,
                    ::hiroz::entity::TypeHash::from_rihs_string(#hash_str)
                        .expect("Invalid RIHS hash")
                )
            }
        }
    })
}

/// Check if a field uses ZBuf (uint8[] or byte[])
fn is_zbuf_field(field: &Field) -> bool {
    matches!(
        (field.field_type.base_type.as_str(), &field.field_type.array),
        ("uint8" | "byte", ArrayType::Unbounded)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::{ParsedMessage, TypeHash};

    #[test]
    fn test_is_zbuf_field() {
        let field = Field {
            name: "data".to_string(),
            field_type: FieldType {
                base_type: "uint8".to_string(),
                package: None,
                array: ArrayType::Unbounded,
                string_bound: None,
            },
            default: None,
        };
        assert!(is_zbuf_field(&field));

        let field = Field {
            name: "data".to_string(),
            field_type: FieldType {
                base_type: "byte".to_string(),
                package: None,
                array: ArrayType::Unbounded,
                string_bound: None,
            },
            default: None,
        };
        assert!(is_zbuf_field(&field));

        let field = Field {
            name: "data".to_string(),
            field_type: FieldType {
                base_type: "int32".to_string(),
                package: None,
                array: ArrayType::Unbounded,
                string_bound: None,
            },
            default: None,
        };
        assert!(!is_zbuf_field(&field));
    }

    #[test]
    fn test_generate_simple_message() {
        let msg = ResolvedMessage {
            parsed: ParsedMessage {
                name: "Simple".to_string(),
                package: "test_msgs".to_string(),
                fields: vec![Field {
                    name: "value".to_string(),
                    field_type: FieldType {
                        base_type: "int32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                }],
                constants: vec![],
                source: String::new(),
                path: PathBuf::new(),
            },
            type_hash: TypeHash([0u8; 32]),
            definition: String::new(),
        };

        let result = generate_message_impl(&msg);
        assert!(result.is_ok());

        let tokens = result.unwrap();
        let code = tokens.to_string();

        // Should contain struct definition
        assert!(code.contains("struct Simple"));
        // Should contain field
        assert!(code.contains("pub value"));
        // Should derive Serialize/Deserialize (no ZBuf)
        assert!(code.contains("Serialize"));
        assert!(code.contains("Deserialize"));
        // Should have MessageTypeInfo
        assert!(code.contains("MessageTypeInfo"));
        // Should provide optional runtime schema hook
        assert!(code.contains("message_schema"));
        assert!(code.contains("MessageSchema :: builder"));
        assert!(code.contains("FieldType :: Int32"));
    }

    #[test]
    fn test_generate_byte_array_message() {
        let msg = ResolvedMessage {
            parsed: ParsedMessage {
                name: "Image".to_string(),
                package: "test_msgs".to_string(),
                fields: vec![
                    Field {
                        name: "width".to_string(),
                        field_type: FieldType {
                            base_type: "uint32".to_string(),
                            package: None,
                            array: ArrayType::Single,
                            string_bound: None,
                        },
                        default: None,
                    },
                    Field {
                        name: "data".to_string(),
                        field_type: FieldType {
                            base_type: "uint8".to_string(),
                            package: None,
                            array: ArrayType::Unbounded,
                            string_bound: None,
                        },
                        default: None,
                    },
                ],
                constants: vec![],
                source: String::new(),
                path: PathBuf::new(),
            },
            type_hash: TypeHash([0u8; 32]),
            definition: String::new(),
        };

        let result = generate_message_impl(&msg);
        assert!(result.is_ok());

        let tokens = result.unwrap();
        let code = tokens.to_string();

        // Should contain hiroz::ZBuf field (wrapper with optimized serde)
        assert!(code.contains("hiroz :: ZBuf"));
        // Should use derived Serialize/Deserialize (ZBuf wrapper implements these traits)
        assert!(code.contains("derive"));
    }

    #[test]
    fn test_generate_service() {
        let request = ParsedMessage {
            name: "AddTwoIntsRequest".to_string(),
            package: "example_interfaces".to_string(),
            fields: vec![
                Field {
                    name: "a".to_string(),
                    field_type: FieldType {
                        base_type: "int64".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
                Field {
                    name: "b".to_string(),
                    field_type: FieldType {
                        base_type: "int64".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
            ],
            constants: vec![],
            source: String::new(),
            path: PathBuf::new(),
        };

        let response = ParsedMessage {
            name: "AddTwoIntsResponse".to_string(),
            package: "example_interfaces".to_string(),
            fields: vec![Field {
                name: "sum".to_string(),
                field_type: FieldType {
                    base_type: "int64".to_string(),
                    package: None,
                    array: ArrayType::Single,
                    string_bound: None,
                },
                default: None,
            }],
            constants: vec![],
            source: String::new(),
            path: PathBuf::new(),
        };

        let srv = ResolvedService {
            parsed: crate::types::ParsedService {
                name: "AddTwoInts".to_string(),
                package: "example_interfaces".to_string(),
                request: request.clone(),
                response: response.clone(),
                source: String::new(),
                path: PathBuf::new(),
            },
            request: ResolvedMessage {
                parsed: request,
                type_hash: TypeHash([0u8; 32]),
                definition: String::new(),
            },
            response: ResolvedMessage {
                parsed: response,
                type_hash: TypeHash([0u8; 32]),
                definition: String::new(),
            },
            type_hash: TypeHash([0u8; 32]),
        };

        let result = generate_service_impl(&srv);
        assert!(result.is_ok());

        let tokens = result.unwrap();
        let code = tokens.to_string();

        assert!(code.contains("struct AddTwoInts"));
        assert!(code.contains("ServiceTypeInfo"));
        assert!(code.contains("AddTwoIntsRequest"));
        assert!(code.contains("AddTwoIntsResponse"));
    }

    #[test]
    fn test_generate_schema_for_nested_message_uses_message_schema_hook() {
        let msg = ResolvedMessage {
            parsed: ParsedMessage {
                name: "StampedPoint".to_string(),
                package: "test_msgs".to_string(),
                fields: vec![Field {
                    name: "point".to_string(),
                    field_type: FieldType {
                        base_type: "Point".to_string(),
                        package: Some("geometry_msgs".to_string()),
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                }],
                constants: vec![],
                source: String::new(),
                path: PathBuf::new(),
            },
            type_hash: TypeHash([0u8; 32]),
            definition: String::new(),
        };

        let ctx = GenerationContext::new(
            Some("hiroz_msgs".to_string()),
            std::iter::once("test_msgs".to_string()).collect(),
        );

        let tokens = generate_message_impl_with_context(&msg, &ctx).unwrap();
        let code = tokens.to_string();

        assert!(code.contains("FieldType :: Message"));
        assert!(code.contains("MessageTypeInfo"));
        assert!(code.contains("message_schema"));
        assert!(code.contains("geometry_msgs :: Point"));
    }
}
