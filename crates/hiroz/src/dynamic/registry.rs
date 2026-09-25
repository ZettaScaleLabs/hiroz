//! Schema registry for dynamic message types.
//!
//! Provides a global cache of message schemas with lazy initialization
//! and pre-registration of bundled schemas.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

#[cfg(feature = "dynamic-schema-loader")]
use super::error::DynamicError;
use super::schema::MessageSchema;
#[cfg(feature = "dynamic-schema-loader")]
use super::schema::{FieldSchema, FieldType};

/// Global registry of message schemas.
///
/// Provides fast O(1) lookup by type name and ensures schema sharing
/// via `Arc<MessageSchema>`. Can be pre-populated with bundled schemas.
pub struct SchemaRegistry {
    schemas: HashMap<String, Arc<MessageSchema>>,
}

impl SchemaRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Get the global registry (lazy initialized).
    pub fn global() -> &'static RwLock<SchemaRegistry> {
        static REGISTRY: OnceLock<RwLock<SchemaRegistry>> = OnceLock::new();
        REGISTRY.get_or_init(|| RwLock::new(SchemaRegistry::new()))
    }

    /// Get schema by full type name (e.g., "geometry_msgs/msg/Twist").
    pub fn get(&self, type_name: &str) -> Option<Arc<MessageSchema>> {
        self.schemas.get(type_name).cloned()
    }

    /// Register a schema and return the Arc for sharing.
    pub fn register(&mut self, schema: Arc<MessageSchema>) -> Arc<MessageSchema> {
        let type_name = schema.type_name.clone();
        self.schemas.insert(type_name, schema.clone());
        schema
    }

    /// Check if a type is registered.
    pub fn contains(&self, type_name: &str) -> bool {
        self.schemas.contains_key(type_name)
    }

    /// List all registered type names.
    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.schemas.keys().map(|s| s.as_str())
    }

    /// Number of registered schemas.
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    /// Clear all registered schemas.
    pub fn clear(&mut self) {
        self.schemas.clear();
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Convenience functions for working with the global registry

/// Get a schema from the global registry (read-only, fast path).
pub fn get_schema(type_name: &str) -> Option<Arc<MessageSchema>> {
    SchemaRegistry::global().read().ok()?.get(type_name)
}

/// Register a schema in the global registry.
pub fn register_schema(schema: Arc<MessageSchema>) -> Arc<MessageSchema> {
    SchemaRegistry::global()
        .write()
        .expect("Registry lock poisoned")
        .register(schema)
}

/// Check if a schema is registered.
pub fn has_schema(type_name: &str) -> bool {
    SchemaRegistry::global()
        .read()
        .map(|r| r.contains(type_name))
        .unwrap_or(false)
}

/// Convert a hiroz-codegen ParsedMessage to a dynamic MessageSchema.
///
/// This function handles the conversion of field types from the codegen
/// representation to the dynamic schema representation.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_message_to_schema(
    msg: &hiroz_codegen::types::ParsedMessage,
    resolver: &impl Fn(&str, &str) -> Option<Arc<MessageSchema>>,
) -> Result<Arc<MessageSchema>, DynamicError> {
    parsed_message_to_schema_named(
        msg,
        &format!("{}/msg/{}", msg.package, msg.name),
        &|canonical| {
            let (package, kind, name) = hiroz_schema::split_canonical(canonical)?;
            (kind == "msg").then(|| resolver(package, name)).flatten()
        },
    )
}

/// Convert a parsed message using an explicit canonical ROS type name.
///
/// Service and action submessages use names such as
/// `example_interfaces/srv/AddTwoInts_Request`, which cannot be inferred from
/// a standalone [`hiroz_codegen::types::ParsedMessage`]. Nested types in the
/// `.msg`, `.srv`, and `.action` source formats belong to the parent's package
/// `msg` namespace. Use [`parsed_idl_message_to_schema`] for IDL references,
/// which retain their declared interface namespace.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_message_to_schema_named(
    msg: &hiroz_codegen::types::ParsedMessage,
    type_name: &str,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<Arc<MessageSchema>, DynamicError> {
    parsed_message_to_schema_named_with_references(msg, type_name, resolver, &|_| None)
}

#[cfg(feature = "dynamic-schema-loader")]
fn parsed_message_to_schema_named_with_references<'a>(
    msg: &hiroz_codegen::types::ParsedMessage,
    type_name: &str,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
    reference: &impl Fn(&str) -> Option<&'a str>,
) -> Result<Arc<MessageSchema>, DynamicError> {
    let (package, _, name) = hiroz_schema::split_canonical(type_name)
        .ok_or_else(|| DynamicError::InvalidTypeName(type_name.to_string()))?;
    let fields: Result<Vec<FieldSchema>, DynamicError> = msg
        .fields
        .iter()
        .map(|f| {
            let field_type = convert_field_type(f, package, reference(&f.name), resolver)?;
            Ok(FieldSchema::new(&f.name, field_type))
        })
        .collect();

    Ok(Arc::new(MessageSchema {
        type_name: type_name.to_string(),
        package: package.to_string(),
        name: name.to_string(),
        fields: fields?,
        type_hash: None,
    }))
}

/// Convert both wire messages from a parsed service definition.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_service_to_schemas(
    service: &hiroz_codegen::types::ParsedService,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<(Arc<MessageSchema>, Arc<MessageSchema>), DynamicError> {
    let prefix = format!("{}/srv/{}", service.package, service.name);
    Ok((
        parsed_message_to_schema_named(&service.request, &format!("{prefix}_Request"), resolver)?,
        parsed_message_to_schema_named(&service.response, &format!("{prefix}_Response"), resolver)?,
    ))
}

/// Convert the goal, result, and feedback messages from a parsed action.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_action_to_schemas(
    action: &hiroz_codegen::types::ParsedAction,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<Vec<Arc<MessageSchema>>, DynamicError> {
    let prefix = format!("{}/action/{}", action.package, action.name);
    let mut schemas = vec![parsed_message_to_schema_named(
        &action.goal,
        &format!("{prefix}_Goal"),
        resolver,
    )?];
    if let Some(result) = &action.result {
        schemas.push(parsed_message_to_schema_named(
            result,
            &format!("{prefix}_Result"),
            resolver,
        )?);
    }
    if let Some(feedback) = &action.feedback {
        schemas.push(parsed_message_to_schema_named(
            feedback,
            &format!("{prefix}_Feedback"),
            resolver,
        )?);
    }
    Ok(schemas)
}

/// Convert every struct from a parsed ROS IDL file.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_idl_to_schemas(
    idl: &hiroz_codegen::parser::idl::ParsedIdl,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<Vec<Arc<MessageSchema>>, DynamicError> {
    idl.messages
        .iter()
        .map(|message| parsed_idl_message_to_schema(idl, message, resolver))
        .collect()
}

/// Convert one struct from a parsed ROS IDL file.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_idl_message_to_schema(
    idl: &hiroz_codegen::parser::idl::ParsedIdl,
    message: &hiroz_codegen::parser::idl::ParsedIdlMessage,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<Arc<MessageSchema>, DynamicError> {
    parsed_message_to_schema_named_with_references(
        &message.message,
        &format!("{}/{}/{}", idl.package, idl.kind, message.message.name),
        resolver,
        &|field| message.references.get(field).map(String::as_str),
    )
}

#[cfg(feature = "dynamic-schema-loader")]
fn convert_field_type(
    field: &hiroz_codegen::types::Field,
    declaring_package: &str,
    reference: Option<&str>,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    use hiroz_codegen::types::ArrayType;

    let base_type = match (
        field.field_type.base_type.as_str(),
        field.field_type.string_bound,
    ) {
        ("string", Some(bound)) => FieldType::BoundedString(bound),
        ("wstring", Some(bound)) => FieldType::BoundedWString(bound),
        _ => convert_base_type(
            &field.field_type.base_type,
            &field.field_type.package,
            declaring_package,
            reference,
            resolver,
        )?,
    };

    match &field.field_type.array {
        ArrayType::Single => Ok(base_type),
        ArrayType::Fixed(n) => Ok(FieldType::Array(Box::new(base_type), *n)),
        ArrayType::Bounded(n) => Ok(FieldType::BoundedSequence(Box::new(base_type), *n)),
        ArrayType::Unbounded => Ok(FieldType::Sequence(Box::new(base_type))),
    }
}

#[cfg(feature = "dynamic-schema-loader")]
fn convert_base_type(
    base_type: &str,
    package: &Option<String>,
    declaring_package: &str,
    reference: Option<&str>,
    resolver: &impl Fn(&str) -> Option<Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    // Check if it's a primitive type
    match base_type {
        "bool" => return Ok(FieldType::Bool),
        "int8" => return Ok(FieldType::Int8),
        "byte" => return Ok(FieldType::Byte),
        "int16" => return Ok(FieldType::Int16),
        "int32" => return Ok(FieldType::Int32),
        "int64" => return Ok(FieldType::Int64),
        "uint8" | "char" => return Ok(FieldType::Uint8),
        "idl_char" => return Ok(FieldType::Char),
        "uint16" => return Ok(FieldType::Uint16),
        "wchar" => return Ok(FieldType::WChar),
        "uint32" => return Ok(FieldType::Uint32),
        "uint64" => return Ok(FieldType::Uint64),
        "float32" => return Ok(FieldType::Float32),
        "float64" => return Ok(FieldType::Float64),
        "string" => return Ok(FieldType::String),
        "wstring" => return Ok(FieldType::WString),
        _ => {}
    }

    // Check for bounded string
    if let Some(rest) = base_type.strip_prefix("string<=")
        && let Ok(max_len) = rest.parse::<usize>()
    {
        return Ok(FieldType::BoundedString(max_len));
    }

    if let Some(rest) = base_type.strip_prefix("wstring<=")
        && let Ok(max_len) = rest.parse::<usize>()
    {
        return Ok(FieldType::BoundedWString(max_len));
    }

    // It's a message type - resolve it
    let canonical = match (reference, package) {
        (Some(reference), _) => reference.to_string(),
        (None, Some(package)) => format!("{package}/msg/{base_type}"),
        (None, None) => format!("{declaring_package}/msg/{base_type}"),
    };
    let schema =
        resolver(&canonical).ok_or_else(|| DynamicError::SchemaNotFound(canonical.clone()))?;

    Ok(FieldType::Message(schema))
}

/// Load a message schema for `type_name` (`pkg/msg/Name`) from `.msg` files on
/// disk at runtime and register it in the global registry, resolving nested
/// message fields recursively. Unlike live discovery, this needs no node on the
/// topic — it is what lets `hu meter pub` publish to an empty topic, like
/// `ros2 topic pub`. `.msg` files are located via `HIROZ_MSG_PATH` (see
/// [`find_msg_file`]). Returns the cached schema if already registered, or
/// `None` if the type cannot be found or parsed.
#[cfg(feature = "dynamic-schema-loader")]
pub fn load_schema(type_name: &str) -> Option<Arc<MessageSchema>> {
    let in_progress = std::cell::RefCell::new(std::collections::HashSet::new());
    load_schema_inner(type_name, &in_progress)
}

/// Recursive worker for [`load_schema`]. `in_progress` tracks the types whose
/// resolution is on the current stack so a self-referential or mutually
/// recursive `.msg` (malformed — well-formed ROS messages form a DAG) bails
/// with a warning instead of recursing until the stack overflows: a cycle would
/// otherwise re-enter here for a type that isn't registered yet, so the
/// `get_schema` memo never hits.
#[cfg(feature = "dynamic-schema-loader")]
fn load_schema_inner(
    type_name: &str,
    in_progress: &std::cell::RefCell<std::collections::HashSet<String>>,
) -> Option<Arc<MessageSchema>> {
    if let Some(schema) = get_schema(type_name) {
        return Some(schema);
    }
    let (package, name) = split_msg_type(type_name)?;
    // File not on disk is a legitimate "try the next source" (live discovery),
    // so return None quietly. Errors *after* a file is found are logged below,
    // since a broken `.msg` masquerading as "not found" would be misleading.
    // Disk first, so a user pointing HIROZ_MSG_PATH at their own definitions
    // always wins over what this binary happens to have been built with.
    let source = match find_msg_file(&package, &name) {
        Some(path) => MsgSource::File(path),
        None => MsgSource::Embedded(embedded_msg_source(&package, &name)?),
    };
    if !in_progress.borrow_mut().insert(type_name.to_string()) {
        tracing::warn!("cyclic .msg definition for {type_name}; skipping schema load");
        return None;
    }
    let parse_result = match &source {
        MsgSource::File(path) => hiroz_codegen::parser::msg::parse_msg_file(path, &package),
        // The path argument is only used for diagnostics; the bytes come from
        // the embedded table.
        MsgSource::Embedded(text) => hiroz_codegen::parser::msg::parse_msg_string(
            text,
            &package,
            std::path::Path::new(&format!("<embedded>/{package}/msg/{name}.msg")),
        ),
    };
    let mut parsed = match parse_result {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!("failed to parse .msg for {type_name} from {source}: {e}");
            in_progress.borrow_mut().remove(type_name);
            return None;
        }
    };
    // A nested field written unqualified (e.g. `Vector3` in geometry_msgs/Twist)
    // is recorded by the parser with no package but refers to the *same* package;
    // without this, `convert_base_type` rejects it and the whole load fails. The
    // parser already qualifies the historical `Header` alias to std_msgs, and
    // primitives short-circuit before the package is consulted, so defaulting
    // every remaining unqualified field to the parent package is safe.
    for field in &mut parsed.fields {
        if field.field_type.package.is_none() {
            field.field_type.package = Some(package.clone());
        }
    }
    // Resolve nested message-typed fields by loading them the same way; each
    // recursive load registers itself, so the outer conversion sees them.
    let resolver = |field_pkg: &str, field_type: &str| -> Option<Arc<MessageSchema>> {
        load_schema_inner(&format!("{field_pkg}/msg/{field_type}"), in_progress)
    };
    let schema = match parsed_message_to_schema(&parsed, &resolver) {
        Ok(schema) => schema,
        Err(e) => {
            tracing::warn!("failed to build schema for {type_name}: {e}");
            in_progress.borrow_mut().remove(type_name);
            return None;
        }
    };
    in_progress.borrow_mut().remove(type_name);
    Some(register_schema(schema))
}

/// Where a `.msg` definition came from, for diagnostics.
#[cfg(feature = "dynamic-schema-loader")]
enum MsgSource {
    File(std::path::PathBuf),
    Embedded(&'static str),
}

#[cfg(feature = "dynamic-schema-loader")]
impl std::fmt::Display for MsgSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsgSource::File(p) => write!(f, "{}", p.display()),
            MsgSource::Embedded(_) => write!(f, "the definitions built into this binary"),
        }
    }
}

/// The bundled `.msg` definitions, embedded as source text at build time.
///
/// Sorted by `pkg/msg/Name`, so the lookup below can binary-search.
#[cfg(feature = "dynamic-schema-loader")]
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_msgs.rs"));
}

/// Look up `<pkg>/msg/<Name>` among the definitions built into this binary.
///
/// This is what lets a downloaded `hu` decode a topic with no `HIROZ_MSG_PATH`
/// set and no reachable type-description service — the case where discovery
/// yields a type name and the disk has nothing to resolve it with.
///
/// Consulted **after** `HIROZ_MSG_PATH`, never before: a user who points that
/// variable at their own definitions means it, and a stale embedded copy must
/// not silently win over the messages their publisher was actually built from.
#[cfg(feature = "dynamic-schema-loader")]
fn embedded_msg_source(package: &str, name: &str) -> Option<&'static str> {
    let key = format!("{package}/msg/{name}");
    embedded::EMBEDDED_MSGS
        .binary_search_by(|(k, _)| (*k).cmp(key.as_str()))
        .ok()
        .map(|i| embedded::EMBEDDED_MSGS[i].1)
}

/// Split `pkg/msg/Name` (or the shorthand `pkg/Name`) into `(package, name)`.
#[cfg(feature = "dynamic-schema-loader")]
fn split_msg_type(type_name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = type_name.split('/').collect();
    match parts.as_slice() {
        [pkg, "msg", name] => Some((pkg.to_string(), name.to_string())),
        [pkg, name] => Some((pkg.to_string(), name.to_string())),
        _ => None,
    }
}

/// Find `<pkg>/msg/<Name>.msg` under `HIROZ_MSG_PATH`. Each colon-separated
/// entry is tried as a prefix that contains packages
/// (`<entry>/<pkg>/msg/<Name>.msg`, e.g. an ament `.../share`), and — only when
/// the entry's own basename equals `pkg` — as the package directory itself
/// (`<entry>/msg/<Name>.msg`). The basename guard is what keeps a request for
/// `pkg_a/msg/Status` from silently resolving to an unrelated
/// `pkg_b/msg/Status.msg` that happens to appear earlier in the path.
#[cfg(feature = "dynamic-schema-loader")]
fn find_msg_file(package: &str, name: &str) -> Option<std::path::PathBuf> {
    let msg_path = std::env::var("HIROZ_MSG_PATH").ok()?;
    let file = format!("{name}.msg");
    for entry in msg_path.split(':') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let base = std::path::Path::new(entry);
        // Prefix layout: `package` is part of the path, so it can't cross packages.
        let as_prefix = base.join(package).join("msg").join(&file);
        if as_prefix.is_file() {
            return Some(as_prefix);
        }
        // Package-directory layout: valid only if this entry IS the package's dir.
        if base.file_name().and_then(|n| n.to_str()) == Some(package) {
            let as_package = base.join("msg").join(&file);
            if as_package.is_file() {
                return Some(as_package);
            }
        }
    }
    None
}

#[cfg(all(test, feature = "dynamic-schema-loader"))]
mod embedded_tests {
    use super::*;

    /// The table is what makes a downloaded `hu` able to decode anything, so an
    /// empty one is a silent regression: every lookup would simply miss and the
    /// behaviour would fall back to today's "no .msg found".
    #[test]
    fn the_embedded_table_is_not_empty_and_is_sorted() {
        assert!(
            !embedded::EMBEDDED_MSGS.is_empty(),
            "no bundled .msg definitions were embedded; \
             the build script found no assets directory"
        );
        assert!(
            embedded::EMBEDDED_MSGS.windows(2).all(|w| w[0].0 < w[1].0),
            "the embedded table must be sorted and duplicate-free: binary_search relies on it"
        );
    }

    /// std_msgs/msg/String is the type the documented quick start echoes, and
    /// the one the G2 measurement showed failing on a default install.
    #[test]
    fn a_common_type_resolves_from_the_embedded_definitions() {
        let src = embedded_msg_source("std_msgs", "String")
            .expect("std_msgs/msg/String must be embedded");
        assert!(
            src.contains("string data"),
            "embedded source does not look like the real definition: {src:?}"
        );
    }

    #[test]
    fn legacy_char_loads_as_uint8_schema() {
        let schema = load_schema("std_msgs/msg/Char").expect("std_msgs/msg/Char must resolve");
        assert!(matches!(
            schema.field("data").expect("Char.data").field_type,
            FieldType::Uint8
        ));
    }

    #[test]
    fn parsed_bounded_strings_preserve_bounds_in_all_collection_forms() {
        let parsed = hiroz_codegen::parser::msg::parse_msg_string(
            "string<=5 narrow\n\
             wstring<=6 wide\n\
             wstring<=6[2] fixed\n\
             wstring<=6[<=3] bounded\n\
             wstring<=6[] unbounded\n",
            "test_msgs",
            std::path::Path::new("Wide.msg"),
        )
        .unwrap();
        let schema = parsed_message_to_schema(&parsed, &|_, _| None).unwrap();

        assert_eq!(
            schema.field("narrow").unwrap().field_type,
            FieldType::BoundedString(5)
        );
        assert_eq!(
            schema.field("wide").unwrap().field_type,
            FieldType::BoundedWString(6)
        );
        assert_eq!(
            schema.field("fixed").unwrap().field_type,
            FieldType::Array(Box::new(FieldType::BoundedWString(6)), 2)
        );
        assert_eq!(
            schema.field("bounded").unwrap().field_type,
            FieldType::BoundedSequence(Box::new(FieldType::BoundedWString(6)), 3)
        );
        assert_eq!(
            schema.field("unbounded").unwrap().field_type,
            FieldType::Sequence(Box::new(FieldType::BoundedWString(6)))
        );
    }

    #[test]
    fn named_conversion_keeps_wire_identity_and_uses_the_parent_package() {
        let parsed = hiroz_codegen::parser::msg::parse_msg_string(
            "Nested child\n",
            "demo_interfaces",
            std::path::Path::new("Container.msg"),
        )
        .unwrap();
        let nested = Arc::new(MessageSchema {
            type_name: "demo_interfaces/msg/Nested".to_string(),
            package: "demo_interfaces".to_string(),
            name: "Nested".to_string(),
            fields: Vec::new(),
            type_hash: None,
        });
        let schema = parsed_message_to_schema_named(
            &parsed,
            "demo_interfaces/srv/DoThing_Request",
            &|canonical| (canonical == "demo_interfaces/msg/Nested").then(|| nested.clone()),
        )
        .unwrap();

        assert_eq!(schema.type_name, "demo_interfaces/srv/DoThing_Request");
        assert_eq!(schema.package, "demo_interfaces");
        assert_eq!(schema.name, "DoThing_Request");
        assert!(matches!(
            &schema.field("child").unwrap().field_type,
            FieldType::Message(child) if child.type_name == "demo_interfaces/msg/Nested"
        ));
    }

    #[test]
    fn service_and_action_helpers_use_ros_wire_names() {
        let service = hiroz_codegen::parser::srv::parse_srv_string(
            "int64 a\n---\nint64 sum\n",
            "demo_interfaces",
            std::path::Path::new("Add.srv"),
        )
        .unwrap();
        let (request, response) = parsed_service_to_schemas(&service, &|_| None).unwrap();
        assert_eq!(request.type_name, "demo_interfaces/srv/Add_Request");
        assert_eq!(response.type_name, "demo_interfaces/srv/Add_Response");

        let action = hiroz_codegen::parser::action::parse_action(
            "int32 order\n---\nint32 result\n---\nint32 progress\n",
            "Count",
            "demo_interfaces",
            std::path::Path::new("Count.action"),
        )
        .unwrap();
        let names = parsed_action_to_schemas(&action, &|_| None)
            .unwrap()
            .into_iter()
            .map(|schema| schema.type_name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "demo_interfaces/action/Count_Goal",
                "demo_interfaces/action/Count_Result",
                "demo_interfaces/action/Count_Feedback",
            ]
        );
    }

    #[test]
    fn idl_native_char_is_distinct_from_legacy_msg_char() {
        let parsed = hiroz_codegen::parser::idl::parse_idl_string(
            "module demo_interfaces { module msg { struct Native { char code; octet raw; }; }; };",
            std::path::Path::new("Native.idl"),
        )
        .unwrap();
        let schema = parsed_idl_to_schemas(&parsed, &|_| None)
            .unwrap()
            .pop()
            .unwrap();

        assert!(matches!(
            schema.field("code").unwrap().field_type,
            FieldType::Char
        ));
        assert!(matches!(
            schema.field("raw").unwrap().field_type,
            FieldType::Byte
        ));
        let description =
            crate::dynamic::MessageSchemaTypeDescription::to_type_description(schema.as_ref())
                .unwrap();
        assert_eq!(
            description.fields[0].field_type.type_id,
            hiroz_schema::TypeId::CHAR
        );
    }

    #[test]
    fn idl_conversion_retains_local_and_cross_namespace_references() {
        let parsed = hiroz_codegen::parser::idl::parse_idl_string(
            "module demo_interfaces { module action { \
               struct Count_Goal { long order; }; \
               struct Count_SendGoal_Request { \
                 Count_Goal goal; \
                 demo_interfaces::srv::Control_Request control; \
               }; \
             }; };",
            std::path::Path::new("Count.idl"),
        )
        .unwrap();
        let goal = parsed
            .messages
            .iter()
            .find(|message| message.message.name == "Count_Goal")
            .unwrap();
        let goal = parsed_idl_message_to_schema(&parsed, goal, &|_| None).unwrap();
        let control = Arc::new(MessageSchema {
            type_name: "demo_interfaces/srv/Control_Request".to_string(),
            package: "demo_interfaces".to_string(),
            name: "Control_Request".to_string(),
            fields: Vec::new(),
            type_hash: None,
        });
        let request = parsed
            .messages
            .iter()
            .find(|message| message.message.name == "Count_SendGoal_Request")
            .unwrap();
        let request =
            parsed_idl_message_to_schema(&parsed, request, &|canonical| match canonical {
                "demo_interfaces/action/Count_Goal" => Some(goal.clone()),
                "demo_interfaces/srv/Control_Request" => Some(control.clone()),
                _ => None,
            })
            .unwrap();

        assert!(matches!(
            &request.field("goal").unwrap().field_type,
            FieldType::Message(schema)
                if schema.type_name == "demo_interfaces/action/Count_Goal"
        ));
        assert!(matches!(
            &request.field("control").unwrap().field_type,
            FieldType::Message(schema)
                if schema.type_name == "demo_interfaces/srv/Control_Request"
        ));
    }

    #[test]
    fn an_unknown_type_is_a_miss_not_a_panic() {
        assert!(embedded_msg_source("no_such_pkg", "Nope").is_none());
    }

    /// Disk must win over the embedded copy. A user who sets HIROZ_MSG_PATH
    /// means it, and their publisher may have been built from definitions that
    /// differ from the ones this binary was compiled with.
    #[test]
    #[serial_test::serial]
    fn a_definition_on_disk_wins_over_the_embedded_one() {
        let dir = std::env::temp_dir().join(format!("hiroz-embed-{}", std::process::id()));
        let msg_dir = dir.join("std_msgs").join("msg");
        std::fs::create_dir_all(&msg_dir).unwrap();
        // Deliberately NOT the real definition, so resolving it proves the disk
        // copy was used rather than the embedded one.
        std::fs::write(
            msg_dir.join("String.msg"),
            "string data\nint32 sentinel_field\n",
        )
        .unwrap();

        let found = find_msg_file("std_msgs", "String");
        let restore = std::env::var("HIROZ_MSG_PATH").ok();
        assert!(
            found.is_none() || restore.is_some(),
            "test environment already has HIROZ_MSG_PATH pointing somewhere"
        );

        unsafe { std::env::set_var("HIROZ_MSG_PATH", &dir) };
        let path = find_msg_file("std_msgs", "String")
            .expect("the on-disk definition must be found first");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("sentinel_field"),
            "HIROZ_MSG_PATH did not take precedence over the embedded table"
        );

        match restore {
            Some(v) => unsafe { std::env::set_var("HIROZ_MSG_PATH", v) },
            None => unsafe { std::env::remove_var("HIROZ_MSG_PATH") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
