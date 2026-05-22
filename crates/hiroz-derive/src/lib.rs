//! Derive macros for hiroz traits.
//!
//! Provides:
//! - `MessageTypeInfo` for Rust-native message schema generation
//! - `FromPyMessage` and `IntoPyMessage` for Python bridge conversion

#![allow(clippy::collapsible_if)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, GenericArgument, Ident, LitStr, PathArguments,
    Type, parse_macro_input,
};

type TokenStream2 = proc_macro2::TokenStream;

/// Derive macro for implementing hiroz message metadata and dynamic schema generation.
///
/// # Example
/// ```ignore
/// #[derive(MessageTypeInfo)]
/// #[ros_msg(type_name = "custom_msgs/msg/RobotStatus")]
/// pub struct RobotStatus {
///     pub battery_percentage: f64,
///     pub is_moving: bool,
/// }
/// ```
#[proc_macro_derive(MessageTypeInfo, attributes(ros_msg))]
pub fn derive_message_type_info(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_message_type_info(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive macro for extracting Rust messages from Python objects.
///
/// # Example
/// ```ignore
/// #[derive(FromPyMessage)]
/// #[ros_msg(module = "hiroz_msgs_py.types.std_msgs")]
/// pub struct String {
///     pub data: std::string::String,
/// }
/// ```
#[proc_macro_derive(FromPyMessage, attributes(ros_msg))]
pub fn derive_from_py_message(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_from_py_message(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive macro for constructing Python objects from Rust messages.
///
/// # Example
/// ```ignore
/// #[derive(IntoPyMessage)]
/// #[ros_msg(module = "hiroz_msgs_py.types.std_msgs")]
/// pub struct String {
///     pub data: std::string::String,
/// }
/// ```
#[proc_macro_derive(IntoPyMessage, attributes(ros_msg))]
pub fn derive_into_py_message(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_into_py_message(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn impl_message_type_info(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "MessageTypeInfo derive does not support generic types in v1",
        ));
    }

    let attrs = parse_ros_msg_args(&input.attrs)?;
    let canonical_type_name = attrs.type_name.ok_or_else(|| {
        syn::Error::new_spanned(
            input,
            "MessageTypeInfo derive requires #[ros_msg(type_name = \"my_pkg/msg/MyType\")]",
        )
    })?;
    let type_name_lit = LitStr::new(&canonical_type_name, proc_macro2::Span::call_site());
    let (package, _kind, message_name) = parse_canonical_type_name(&canonical_type_name)?;
    let dds_type_name = canonical_to_dds_name(&canonical_type_name)?;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "MessageTypeInfo derive only supports named structs in v1",
        ));
    };

    let Fields::Named(fields) = &data.fields else {
        let message = match &data.fields {
            Fields::Unnamed(_) => "MessageTypeInfo derive does not support tuple structs in v1",
            Fields::Unit => "MessageTypeInfo derive does not support unit structs in v1",
            Fields::Named(_) => unreachable!(),
        };
        return Err(syn::Error::new_spanned(input, message));
    };

    let schema_fields = fields
        .named
        .iter()
        .map(generate_message_field_schema_tokens)
        .collect::<syn::Result<Vec<_>>>()?;

    let package_lit = LitStr::new(&package, proc_macro2::Span::call_site());
    let message_name_lit = LitStr::new(&message_name, proc_macro2::Span::call_site());
    let dds_type_name_lit = LitStr::new(&dds_type_name, proc_macro2::Span::call_site());

    Ok(quote! {
        impl ::hiroz::MessageTypeInfo for #name {
            fn type_name() -> &'static str {
                #dds_type_name_lit
            }

            fn type_hash() -> ::hiroz::entity::TypeHash {
                if !::hiroz::entity::TypeHash::is_supported() {
                    return ::hiroz::entity::TypeHash::zero();
                }

                Self::message_schema()
                    .and_then(|s| s.type_hash.as_deref().map(str::to_owned))
                    .and_then(|rihs| ::hiroz::entity::TypeHash::from_rihs_string(&rihs))
                    .unwrap_or_else(|| ::hiroz::entity::TypeHash::zero())
            }

            fn message_schema() -> Option<::std::sync::Arc<::hiroz::dynamic::MessageSchema>> {
                static SCHEMA: ::std::sync::OnceLock<
                    ::std::sync::Arc<::hiroz::dynamic::MessageSchema>,
                > = ::std::sync::OnceLock::new();

                Some(
                    SCHEMA
                        .get_or_init(|| {
                            let mut schema = ::hiroz::dynamic::MessageSchema {
                                type_name: #type_name_lit.to_string(),
                                package: #package_lit.to_string(),
                                name: #message_name_lit.to_string(),
                                fields: ::std::vec![#(#schema_fields),*],
                                type_hash: None,
                            };

                            if ::hiroz::entity::TypeHash::is_supported() {
                                use ::hiroz::dynamic::MessageSchemaTypeDescription;
                                if let Ok(hash) = schema.compute_type_hash() {
                                    schema.type_hash = Some(hash.to_rihs_string());
                                }
                            }

                            ::std::sync::Arc::new(schema)
                        })
                        .clone(),
                )
            }
        }

        impl ::hiroz::WithTypeInfo for #name {}
    })
}

fn impl_from_py_message(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let Data::Struct(ref data) = input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "FromPyMessage only supports structs",
        ));
    };

    let Fields::Named(ref fields) = data.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "FromPyMessage requires named fields",
        ));
    };

    let field_extractions: Vec<TokenStream2> = fields
        .named
        .iter()
        .map(|f| {
            let field_name = f.ident.as_ref().unwrap();
            let field_name_str = field_name_to_attr(field_name);
            let field_type = &f.ty;
            let use_zbuf = parse_ros_msg_args(&f.attrs)?.zbuf;
            generate_field_extraction(field_name, &field_name_str, field_type, use_zbuf)
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        impl ::hiroz::python_bridge::FromPyMessage for #name {
            fn from_py(obj: &::pyo3::Bound<'_, ::pyo3::PyAny>) -> ::pyo3::PyResult<Self> {
                use ::pyo3::types::PyAnyMethods;
                Ok(Self {
                    #(#field_extractions),*
                })
            }
        }
    })
}

fn impl_into_py_message(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let Data::Struct(ref data) = input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "IntoPyMessage only supports structs",
        ));
    };

    let Fields::Named(ref fields) = data.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "IntoPyMessage requires named fields",
        ));
    };

    let module_path = extract_module_path(&input.attrs)?;

    let field_constructions: Vec<TokenStream2> = fields
        .named
        .iter()
        .map(|f| {
            let field_name = f.ident.as_ref().unwrap();
            let field_name_str = field_name_to_attr(field_name);
            let field_type = &f.ty;
            let use_zbuf = parse_ros_msg_args(&f.attrs)?.zbuf;
            generate_field_construction(field_name, &field_name_str, field_type, use_zbuf)
        })
        .collect::<syn::Result<Vec<_>>>()?;

    let name_str = name.to_string();

    Ok(quote! {
        impl ::hiroz::python_bridge::IntoPyMessage for #name {
            fn into_py_message(&self, py: ::pyo3::Python) -> ::pyo3::PyResult<::pyo3::PyObject> {
                use ::pyo3::types::{PyAnyMethods, PyDictMethods, PyModuleMethods};
                let module = ::pyo3::types::PyModule::import_bound(py, #module_path)?;
                let class = module.getattr(#name_str)?;

                let kwargs = ::pyo3::types::PyDict::new_bound(py);
                #(#field_constructions)*

                class.call((), Some(&kwargs)).map(|obj| obj.into())
            }
        }
    })
}

fn generate_message_field_schema_tokens(field: &syn::Field) -> syn::Result<TokenStream2> {
    let field_name = field
        .ident
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(field, "named fields are required"))?;
    let field_name_str = field_name_to_attr(field_name);
    let field_type = generate_message_field_type_tokens(&field.ty)?;

    Ok(quote! {
        ::hiroz::dynamic::FieldSchema::new(#field_name_str, #field_type)
    })
}

fn generate_message_field_type_tokens(ty: &Type) -> syn::Result<TokenStream2> {
    match ty {
        Type::Path(type_path) => {
            if type_path.qself.is_some() {
                return unsupported_message_type(
                    ty,
                    "qualified self types are not supported in v1",
                );
            }

            let last_segment = type_path.path.segments.last().ok_or_else(|| {
                syn::Error::new_spanned(ty, "unsupported field type for MessageTypeInfo derive")
            })?;
            let ident_str = last_segment.ident.to_string();

            match ident_str.as_str() {
                "bool" => Ok(quote! { ::hiroz::dynamic::FieldType::Bool }),
                "i8" => Ok(quote! { ::hiroz::dynamic::FieldType::Int8 }),
                "u8" => Ok(quote! { ::hiroz::dynamic::FieldType::Uint8 }),
                "i16" => Ok(quote! { ::hiroz::dynamic::FieldType::Int16 }),
                "u16" => Ok(quote! { ::hiroz::dynamic::FieldType::Uint16 }),
                "i32" => Ok(quote! { ::hiroz::dynamic::FieldType::Int32 }),
                "u32" => Ok(quote! { ::hiroz::dynamic::FieldType::Uint32 }),
                "i64" => Ok(quote! { ::hiroz::dynamic::FieldType::Int64 }),
                "u64" => Ok(quote! { ::hiroz::dynamic::FieldType::Uint64 }),
                "f32" => Ok(quote! { ::hiroz::dynamic::FieldType::Float32 }),
                "f64" => Ok(quote! { ::hiroz::dynamic::FieldType::Float64 }),
                "String" => Ok(quote! { ::hiroz::dynamic::FieldType::String }),
                "usize" | "isize" => unsupported_message_type(
                    ty,
                    "usize and isize are not supported by MessageTypeInfo derive in v1",
                ),
                "Option" => unsupported_message_type(
                    ty,
                    "Option fields are not supported by MessageTypeInfo derive in v1",
                ),
                "HashMap" | "BTreeMap" => unsupported_message_type(
                    ty,
                    "map fields are not supported by MessageTypeInfo derive in v1",
                ),
                "Vec" => {
                    let PathArguments::AngleBracketed(args) = &last_segment.arguments else {
                        return unsupported_message_type(
                            ty,
                            "Vec fields must specify an element type",
                        );
                    };
                    let Some(GenericArgument::Type(inner)) = args.args.first() else {
                        return unsupported_message_type(
                            ty,
                            "Vec fields must specify an element type",
                        );
                    };
                    let inner_tokens = generate_message_field_type_tokens(inner)?;
                    Ok(quote! {
                        ::hiroz::dynamic::FieldType::Sequence(::std::boxed::Box::new(#inner_tokens))
                    })
                }
                _ => Ok(quote! {
                    ::hiroz::dynamic::FieldType::Message(
                        <#ty as ::hiroz::MessageTypeInfo>::message_schema()
                            .expect("derived nested message schema must be available")
                    )
                }),
            }
        }
        Type::Array(array) => {
            let len = match &array.len {
                Expr::Lit(expr_lit) => match &expr_lit.lit {
                    syn::Lit::Int(value) => value.base10_parse::<usize>()?,
                    _ => {
                        return unsupported_message_type(
                            ty,
                            "array lengths must be integer literals for MessageTypeInfo derive",
                        );
                    }
                },
                _ => {
                    return unsupported_message_type(
                        ty,
                        "array lengths must be integer literals for MessageTypeInfo derive",
                    );
                }
            };

            let inner_tokens = generate_message_field_type_tokens(&array.elem)?;
            Ok(quote! {
                ::hiroz::dynamic::FieldType::Array(::std::boxed::Box::new(#inner_tokens), #len)
            })
        }
        Type::Tuple(_) => unsupported_message_type(
            ty,
            "tuple fields are not supported by MessageTypeInfo derive in v1",
        ),
        _ => unsupported_message_type(
            ty,
            "unsupported field type for MessageTypeInfo derive in v1",
        ),
    }
}

fn unsupported_message_type<T>(node: &T, message: &str) -> syn::Result<TokenStream2>
where
    T: quote::ToTokens,
{
    Err(syn::Error::new_spanned(node, message))
}

fn parse_canonical_type_name(type_name: &str) -> syn::Result<(String, String, String)> {
    let parts: Vec<_> = type_name.split('/').collect();
    if parts.len() != 3 {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "ros_msg type_name must look like \"my_pkg/msg/MyType\"",
        ));
    }

    match parts[1] {
        "msg" | "srv" => Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        )),
        "action" => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "ros_msg type_name does not support \"action\" — derive each constituent message type \
             directly (e.g. \"my_pkg/msg/MyAction_Goal\", \"my_pkg/msg/MyAction_Result\")",
        )),
        _ => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "ros_msg type_name kind must be one of: msg, srv",
        )),
    }
}

fn canonical_to_dds_name(type_name: &str) -> syn::Result<String> {
    let (package, kind, name) = parse_canonical_type_name(type_name)?;
    Ok(format!("{package}::{kind}::dds_::{name}_"))
}

/// Generate extraction code for a single field.
fn generate_field_extraction(
    field_name: &Ident,
    field_name_str: &str,
    field_type: &Type,
    use_zbuf: bool,
) -> syn::Result<TokenStream2> {
    if use_zbuf {
        return Ok(quote! {
            #field_name: {
                use ::pyo3::types::{PyByteArrayMethods, PyBytesMethods};
                let py_attr = obj.getattr(#field_name_str)?;
                if let Ok(view) = py_attr.downcast::<::hiroz::zbuf_view::ZBufView>() {
                    view.borrow().zbuf().clone()
                } else if let Ok(bytes) = py_attr.downcast::<::pyo3::types::PyBytes>() {
                    ::hiroz::ZBuf::from(bytes.as_bytes().to_vec())
                } else if let Ok(bytearray) = py_attr.downcast::<::pyo3::types::PyByteArray>() {
                    ::hiroz::ZBuf::from(unsafe { bytearray.as_bytes() }.to_vec())
                } else {
                    let bytes: Vec<u8> = py_attr.extract()?;
                    ::hiroz::ZBuf::from(bytes)
                }
            }
        });
    }

    match classify_type(field_type) {
        TypeClass::Primitive | TypeClass::String => Ok(quote! {
            #field_name: obj.getattr(#field_name_str)?.extract()?
        }),
        TypeClass::Vec(inner) => {
            let inner_class = classify_type(&inner);
            match inner_class {
                TypeClass::Primitive if is_u8_type(&inner) => Ok(quote! {
                    #field_name: {
                        use ::pyo3::types::{PyByteArrayMethods, PyBytesMethods};
                        let py_attr = obj.getattr(#field_name_str)?;
                        if let Ok(bytes) = py_attr.downcast::<::pyo3::types::PyBytes>() {
                            bytes.as_bytes().to_vec()
                        } else if let Ok(bytearray) = py_attr.downcast::<::pyo3::types::PyByteArray>() {
                            unsafe { bytearray.as_bytes() }.to_vec()
                        } else {
                            py_attr.extract()?
                        }
                    }
                }),
                TypeClass::Primitive | TypeClass::String => Ok(quote! {
                    #field_name: obj.getattr(#field_name_str)?.extract()?
                }),
                _ => Ok(quote! {
                    #field_name: {
                        use ::pyo3::types::PyListMethods;
                        let py_list = obj.getattr(#field_name_str)?;
                        let mut vec = Vec::new();
                        for item in py_list.iter()? {
                            vec.push(<#inner as ::hiroz::python_bridge::FromPyMessage>::from_py(&item?)?);
                        }
                        vec
                    }
                }),
            }
        }
        TypeClass::Array(inner, size) => {
            let inner_class = classify_type(&inner);
            match inner_class {
                TypeClass::Primitive | TypeClass::String => Ok(quote! {
                    #field_name: {
                        let v: Vec<_> = obj.getattr(#field_name_str)?.extract()?;
                        let mut arr: #field_type = unsafe { ::std::mem::zeroed() };
                        let len = ::std::cmp::min(v.len(), #size);
                        arr[..len].copy_from_slice(&v[..len]);
                        arr
                    }
                }),
                _ => Ok(quote! {
                    #field_name: {
                        use ::pyo3::types::PyListMethods;
                        let py_list = obj.getattr(#field_name_str)?;
                        let mut arr: #field_type = ::std::array::from_fn(|_| Default::default());
                        for (i, item) in py_list.iter()?.enumerate().take(#size) {
                            arr[i] = <#inner as ::hiroz::python_bridge::FromPyMessage>::from_py(&item?)?;
                        }
                        arr
                    }
                }),
            }
        }
        TypeClass::Nested => Ok(quote! {
            #field_name: {
                let py_attr = obj.getattr(#field_name_str)?;
                if py_attr.is_none() {
                    Default::default()
                } else {
                    <#field_type as ::hiroz::python_bridge::FromPyMessage>::from_py(&py_attr)?
                }
            }
        }),
        TypeClass::ZBuf => Ok(quote! {
            #field_name: {
                use ::pyo3::types::{PyByteArrayMethods, PyBytesMethods};
                let py_attr = obj.getattr(#field_name_str)?;
                let bytes: Vec<u8> = if let Ok(bytes) = py_attr.downcast::<::pyo3::types::PyBytes>() {
                    bytes.as_bytes().to_vec()
                } else if let Ok(bytearray) = py_attr.downcast::<::pyo3::types::PyByteArray>() {
                    unsafe { bytearray.as_bytes() }.to_vec()
                } else {
                    py_attr.extract()?
                };
                ::hiroz::ZBuf::from(bytes)
            }
        }),
    }
}

/// Generate construction code for a single field (Rust -> Python).
fn generate_field_construction(
    field_name: &Ident,
    field_name_str: &str,
    field_type: &Type,
    use_zbuf: bool,
) -> syn::Result<TokenStream2> {
    if use_zbuf {
        return Ok(quote! {
            {
                let zbuf_view = ::hiroz::zbuf_view::ZBufView::new(self.#field_name.clone());
                let py_view = ::pyo3::Py::new(py, zbuf_view)?;
                kwargs.set_item(#field_name_str, py_view)?;
            }
        });
    }

    match classify_type(field_type) {
        TypeClass::Primitive => Ok(quote! {
            kwargs.set_item(#field_name_str, self.#field_name)?;
        }),
        TypeClass::String => Ok(quote! {
            kwargs.set_item(#field_name_str, &self.#field_name)?;
        }),
        TypeClass::Vec(inner) => {
            let inner_class = classify_type(&inner);
            match inner_class {
                TypeClass::Primitive if is_u8_type(&inner) => Ok(quote! {
                    {
                        let py_bytes = ::pyo3::types::PyBytes::new_bound(py, &self.#field_name);
                        kwargs.set_item(#field_name_str, py_bytes)?;
                    }
                }),
                TypeClass::Primitive | TypeClass::String => Ok(quote! {
                    kwargs.set_item(#field_name_str, &self.#field_name)?;
                }),
                _ => Ok(quote! {
                    {
                        use ::pyo3::types::PyListMethods;
                        let py_list = ::pyo3::types::PyList::empty_bound(py);
                        for item in &self.#field_name {
                            py_list.append(
                                <#inner as ::hiroz::python_bridge::IntoPyMessage>::into_py_message(item, py)?
                            )?;
                        }
                        kwargs.set_item(#field_name_str, py_list)?;
                    }
                }),
            }
        }
        TypeClass::Array(inner, _) => {
            let inner_class = classify_type(&inner);
            match inner_class {
                TypeClass::Primitive | TypeClass::String => Ok(quote! {
                    kwargs.set_item(#field_name_str, self.#field_name.to_vec())?;
                }),
                _ => Ok(quote! {
                    {
                        use ::pyo3::types::PyListMethods;
                        let py_list = ::pyo3::types::PyList::empty_bound(py);
                        for item in &self.#field_name {
                            py_list.append(
                                <#inner as ::hiroz::python_bridge::IntoPyMessage>::into_py_message(item, py)?
                            )?;
                        }
                        kwargs.set_item(#field_name_str, py_list)?;
                    }
                }),
            }
        }
        TypeClass::Nested => Ok(quote! {
            kwargs.set_item(
                #field_name_str,
                <#field_type as ::hiroz::python_bridge::IntoPyMessage>::into_py_message(&self.#field_name, py)?
            )?;
        }),
        TypeClass::ZBuf => Ok(quote! {
            {
                use ::zenoh_buffers::buffer::SplitBuffer;
                let bytes = self.#field_name.contiguous();
                let py_bytes = ::pyo3::types::PyBytes::new_bound(py, bytes.as_ref());
                kwargs.set_item(#field_name_str, py_bytes)?;
            }
        }),
    }
}

/// Type classification for Python conversion code generation.
#[derive(Debug)]
enum TypeClass {
    Primitive,
    String,
    Vec(Box<Type>),
    Array(Box<Type>, usize),
    Nested,
    ZBuf,
}

/// Classify a type for Python conversion code generation purposes.
fn classify_type(ty: &Type) -> TypeClass {
    if let Type::Path(type_path) = ty {
        let segments = &type_path.path.segments;
        if let Some(last_segment) = segments.last() {
            let ident_str = last_segment.ident.to_string();

            if matches!(
                ident_str.as_str(),
                "bool"
                    | "i8"
                    | "u8"
                    | "i16"
                    | "u16"
                    | "i32"
                    | "u32"
                    | "i64"
                    | "u64"
                    | "f32"
                    | "f64"
            ) {
                return TypeClass::Primitive;
            }

            if ident_str == "String" {
                return TypeClass::String;
            }

            if ident_str == "ZBuf" {
                return TypeClass::ZBuf;
            }

            if ident_str == "Vec" {
                if let PathArguments::AngleBracketed(args) = &last_segment.arguments {
                    if let Some(GenericArgument::Type(inner)) = args.args.first() {
                        return TypeClass::Vec(Box::new(inner.clone()));
                    }
                }
            }
        }
    }

    if let Type::Array(arr) = ty {
        if let Expr::Lit(lit) = &arr.len {
            if let syn::Lit::Int(int_lit) = &lit.lit {
                if let Ok(size) = int_lit.base10_parse::<usize>() {
                    return TypeClass::Array(Box::new((*arr.elem).clone()), size);
                }
            }
        }
    }

    TypeClass::Nested
}

#[derive(Default)]
struct RosMsgArgs {
    module: Option<String>,
    type_name: Option<String>,
    zbuf: bool,
}

fn parse_ros_msg_args(attrs: &[Attribute]) -> syn::Result<RosMsgArgs> {
    let mut parsed = RosMsgArgs::default();

    for attr in attrs {
        if !attr.path().is_ident("ros_msg") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("module") {
                let value = meta.value()?.parse::<LitStr>()?;
                parsed.module = Some(value.value());
                return Ok(());
            }

            if meta.path.is_ident("type_name") {
                let value = meta.value()?.parse::<LitStr>()?;
                parsed.type_name = Some(value.value());
                return Ok(());
            }

            if meta.path.is_ident("zbuf") {
                parsed.zbuf = true;
                return Ok(());
            }

            Err(meta
                .error("unsupported ros_msg attribute, expected one of: module, type_name, zbuf"))
        })?;
    }

    Ok(parsed)
}

fn extract_module_path(attrs: &[Attribute]) -> syn::Result<String> {
    Ok(parse_ros_msg_args(attrs)?
        .module
        .unwrap_or_else(|| "hiroz_msgs_py.types".to_string()))
}

fn is_u8_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(last_segment) = type_path.path.segments.last() {
            return last_segment.ident == "u8";
        }
    }
    false
}

fn field_name_to_attr(ident: &Ident) -> String {
    let name = ident.to_string();
    if let Some(stripped) = name.strip_prefix("r#") {
        stripped.to_string()
    } else {
        name
    }
}
