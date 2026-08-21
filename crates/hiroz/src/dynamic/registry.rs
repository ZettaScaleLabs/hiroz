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
    let fields: Result<Vec<FieldSchema>, DynamicError> = msg
        .fields
        .iter()
        .map(|f| {
            let field_type = convert_field_type(f, resolver)?;
            Ok(FieldSchema::new(&f.name, field_type))
        })
        .collect();

    Ok(Arc::new(MessageSchema {
        type_name: format!("{}/msg/{}", msg.package, msg.name),
        package: msg.package.clone(),
        name: msg.name.clone(),
        fields: fields?,
        type_hash: None,
    }))
}

#[cfg(feature = "dynamic-schema-loader")]
fn convert_field_type(
    field: &hiroz_codegen::types::Field,
    resolver: &impl Fn(&str, &str) -> Option<Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    use hiroz_codegen::types::ArrayType;

    let base_type = convert_base_type(
        &field.field_type.base_type,
        &field.field_type.package,
        resolver,
    )?;

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
    resolver: &impl Fn(&str, &str) -> Option<Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    // Check if it's a primitive type
    match base_type {
        "bool" => return Ok(FieldType::Bool),
        "int8" | "byte" => return Ok(FieldType::Int8),
        "int16" => return Ok(FieldType::Int16),
        "int32" => return Ok(FieldType::Int32),
        "int64" => return Ok(FieldType::Int64),
        "uint8" | "char" => return Ok(FieldType::Uint8),
        "uint16" => return Ok(FieldType::Uint16),
        "uint32" => return Ok(FieldType::Uint32),
        "uint64" => return Ok(FieldType::Uint64),
        "float32" => return Ok(FieldType::Float32),
        "float64" => return Ok(FieldType::Float64),
        "string" => return Ok(FieldType::String),
        _ => {}
    }

    // Check for bounded string
    if let Some(rest) = base_type.strip_prefix("string<=")
        && let Ok(max_len) = rest.parse::<usize>()
    {
        return Ok(FieldType::BoundedString(max_len));
    }

    // It's a message type - resolve it
    let pkg = package
        .as_ref()
        .ok_or_else(|| DynamicError::InvalidTypeName(base_type.to_string()))?;
    let schema = resolver(pkg, base_type)
        .ok_or_else(|| DynamicError::SchemaNotFound(format!("{}/msg/{}", pkg, base_type)))?;

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
        std::fs::write(msg_dir.join("String.msg"), "string data\nint32 sentinel_field\n").unwrap();

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
