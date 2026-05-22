//! Type Description Service implementation.
//!
//! This module provides the `TypeDescriptionService` which allows nodes to expose
//! their registered type descriptions to other nodes. This enables dynamic schema
//! discovery as specified in REP-2016.
//!
//! # Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────┐
//! │                      ZNode                                  │
//! ├────────────────────────────────────────────────────────────┤
//! │  TypeDescriptionService                                     │
//! │  ├── key_expr: "{node}/get_type_description"               │
//! │  ├── server: ZServer<GetTypeDescription>                    │
//! │  └── schemas: HashMap<type_name, RegisteredSchema>         │
//! └────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use hiroz::dynamic::{TypeDescriptionService, MessageSchema, FieldType};
//!
//! // Create a schema
//! let schema = MessageSchema::builder("geometry_msgs/msg/Point")
//!     .field("x", FieldType::Float64)
//!     .field("y", FieldType::Float64)
//!     .field("z", FieldType::Float64)
//!     .build()?;
//!
//! // Register with service
//! type_desc_service.register_schema(schema);
//!
//! // Now remote nodes can query for "geometry_msgs/msg/Point"
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use hiroz_cdr::{CdrBuffer, CdrDeserialize, CdrReader, CdrSerialize, CdrSerializedSize, CdrWriter};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace, warn};
use zenoh::query::Query;
use zenoh::{Result as ZResult, Session};

use crate::ServiceTypeInfo;
use crate::attachment::Attachment;
use crate::entity::{TypeHash, TypeInfo};
use crate::msg::ZService;
use crate::service::{ZServer, ZServerBuilder};

use super::error::DynamicError;
use super::schema::MessageSchema;
use super::type_description::MessageSchemaTypeDescription;

// ============================================================================
// Wire format types for type_description_interfaces
// These match the ROS 2 type_description_interfaces package exactly
// ============================================================================

/// Wire format for FieldType message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireFieldType {
    pub type_id: u8,
    pub capacity: u64,
    pub string_capacity: u64,
    pub nested_type_name: String,
}

/// Wire format for Field message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: WireFieldType,
    pub default_value: String,
}

/// Wire format for IndividualTypeDescription message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireIndividualTypeDescription {
    pub type_name: String,
    pub fields: Vec<WireField>,
}

/// Wire format for TypeDescription message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireTypeDescription {
    pub type_description: WireIndividualTypeDescription,
    pub referenced_type_descriptions: Vec<WireIndividualTypeDescription>,
}

/// Wire format for TypeSource message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireTypeSource {
    pub type_name: String,
    pub encoding: String,
    pub raw_file_contents: String,
}

/// Wire format for KeyValue message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireKeyValue {
    pub key: String,
    pub value: String,
}

/// GetTypeDescription service request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetTypeDescriptionRequest {
    pub type_name: String,
    pub type_hash: String,
    pub include_type_sources: bool,
}

// ── CDR impls for wire types ──────────────────────────────────────────────────

impl CdrSerialize for WireFieldType {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.type_id.cdr_serialize(w);
        self.capacity.cdr_serialize(w);
        self.string_capacity.cdr_serialize(w);
        self.nested_type_name.cdr_serialize(w);
    }
}
impl CdrDeserialize for WireFieldType {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(WireFieldType {
            type_id: u8::cdr_deserialize(r)?,
            capacity: u64::cdr_deserialize(r)?,
            string_capacity: u64::cdr_deserialize(r)?,
            nested_type_name: String::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for WireFieldType {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.type_id.cdr_serialized_size(pos);
        let p = self.capacity.cdr_serialized_size(p);
        let p = self.string_capacity.cdr_serialized_size(p);
        self.nested_type_name.cdr_serialized_size(p)
    }
}

impl CdrSerialize for WireField {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.name.cdr_serialize(w);
        self.field_type.cdr_serialize(w);
        self.default_value.cdr_serialize(w);
    }
}
impl CdrDeserialize for WireField {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(WireField {
            name: String::cdr_deserialize(r)?,
            field_type: WireFieldType::cdr_deserialize(r)?,
            default_value: String::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for WireField {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.name.cdr_serialized_size(pos);
        let p = self.field_type.cdr_serialized_size(p);
        self.default_value.cdr_serialized_size(p)
    }
}

impl CdrSerialize for WireIndividualTypeDescription {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.type_name.cdr_serialize(w);
        self.fields.cdr_serialize(w);
    }
}
impl CdrDeserialize for WireIndividualTypeDescription {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(WireIndividualTypeDescription {
            type_name: String::cdr_deserialize(r)?,
            fields: Vec::<WireField>::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for WireIndividualTypeDescription {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.type_name.cdr_serialized_size(pos);
        self.fields.cdr_serialized_size(p)
    }
}

impl CdrSerialize for WireTypeDescription {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.type_description.cdr_serialize(w);
        self.referenced_type_descriptions.cdr_serialize(w);
    }
}
impl CdrDeserialize for WireTypeDescription {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(WireTypeDescription {
            type_description: WireIndividualTypeDescription::cdr_deserialize(r)?,
            referenced_type_descriptions: Vec::<WireIndividualTypeDescription>::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for WireTypeDescription {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.type_description.cdr_serialized_size(pos);
        self.referenced_type_descriptions.cdr_serialized_size(p)
    }
}

impl CdrSerialize for WireTypeSource {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.type_name.cdr_serialize(w);
        self.encoding.cdr_serialize(w);
        self.raw_file_contents.cdr_serialize(w);
    }
}
impl CdrDeserialize for WireTypeSource {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(WireTypeSource {
            type_name: String::cdr_deserialize(r)?,
            encoding: String::cdr_deserialize(r)?,
            raw_file_contents: String::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for WireTypeSource {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.type_name.cdr_serialized_size(pos);
        let p = self.encoding.cdr_serialized_size(p);
        self.raw_file_contents.cdr_serialized_size(p)
    }
}

impl CdrSerialize for WireKeyValue {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.key.cdr_serialize(w);
        self.value.cdr_serialize(w);
    }
}
impl CdrDeserialize for WireKeyValue {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(WireKeyValue {
            key: String::cdr_deserialize(r)?,
            value: String::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for WireKeyValue {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.key.cdr_serialized_size(pos);
        self.value.cdr_serialized_size(p)
    }
}

impl CdrSerialize for GetTypeDescriptionRequest {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.type_name.cdr_serialize(w);
        self.type_hash.cdr_serialize(w);
        self.include_type_sources.cdr_serialize(w);
    }
}
impl CdrDeserialize for GetTypeDescriptionRequest {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GetTypeDescriptionRequest {
            type_name: String::cdr_deserialize(r)?,
            type_hash: String::cdr_deserialize(r)?,
            include_type_sources: bool::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for GetTypeDescriptionRequest {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.type_name.cdr_serialized_size(pos);
        let p = self.type_hash.cdr_serialized_size(p);
        self.include_type_sources.cdr_serialized_size(p)
    }
}

/// GetTypeDescription service response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetTypeDescriptionResponse {
    pub successful: bool,
    pub failure_reason: String,
    pub type_description: WireTypeDescription,
    pub type_sources: Vec<WireTypeSource>,
    pub extra_information: Vec<WireKeyValue>,
}

impl CdrSerialize for GetTypeDescriptionResponse {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        self.successful.cdr_serialize(w);
        self.failure_reason.cdr_serialize(w);
        self.type_description.cdr_serialize(w);
        self.type_sources.cdr_serialize(w);
        self.extra_information.cdr_serialize(w);
    }
}
impl CdrDeserialize for GetTypeDescriptionResponse {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(GetTypeDescriptionResponse {
            successful: bool::cdr_deserialize(r)?,
            failure_reason: String::cdr_deserialize(r)?,
            type_description: WireTypeDescription::cdr_deserialize(r)?,
            type_sources: Vec::<WireTypeSource>::cdr_deserialize(r)?,
            extra_information: Vec::<WireKeyValue>::cdr_deserialize(r)?,
        })
    }
}
impl CdrSerializedSize for GetTypeDescriptionResponse {
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        let p = self.successful.cdr_serialized_size(pos);
        let p = self.failure_reason.cdr_serialized_size(p);
        let p = self.type_description.cdr_serialized_size(p);
        let p = self.type_sources.cdr_serialized_size(p);
        self.extra_information.cdr_serialized_size(p)
    }
}

/// Marker type for GetTypeDescription service.
pub struct GetTypeDescription;

impl ZService for GetTypeDescription {
    type Request = GetTypeDescriptionRequest;
    type Response = GetTypeDescriptionResponse;
}

impl ServiceTypeInfo for GetTypeDescription {
    fn service_type_info() -> TypeInfo {
        // Hash for GetTypeDescription service (matches ROS 2 Jazzy)
        TypeInfo::new(
            "type_description_interfaces::srv::dds_::GetTypeDescription_",
            TypeHash::from_rihs_string(
                "RIHS01_69b9c19c1021405984cc60dbbb1edceb147a6538b411d812ba6afabeed962cd5",
            )
            .expect("Invalid RIHS hash"),
        )
    }
}

// ============================================================================
// TypeDescriptionService implementation
// ============================================================================

/// Schema registered with the TypeDescriptionService.
#[derive(Clone)]
pub struct RegisteredSchema {
    /// The runtime schema
    pub schema: Arc<MessageSchema>,
    /// Precomputed type hash (RIHS01 format)
    pub type_hash: String,
    /// Optional source file content
    pub source: Option<TypeSource>,
}

/// Source file for a type definition.
#[derive(Clone)]
pub struct TypeSource {
    /// Source encoding (e.g., "msg", "srv", "action", "idl")
    pub encoding: String,
    /// Raw file contents
    pub raw_content: String,
}

impl RegisteredSchema {
    /// Create a new registered schema, computing the type hash.
    pub fn new(schema: Arc<MessageSchema>) -> std::result::Result<Self, DynamicError> {
        let type_hash = schema.compute_type_hash()?.to_rihs_string();
        Ok(Self {
            schema,
            type_hash,
            source: None,
        })
    }

    /// Create a registered schema with source.
    pub fn with_source(
        schema: Arc<MessageSchema>,
        encoding: &str,
        raw_content: &str,
    ) -> std::result::Result<Self, DynamicError> {
        let type_hash = schema.compute_type_hash()?.to_rihs_string();
        Ok(Self {
            schema,
            type_hash,
            source: Some(TypeSource {
                encoding: encoding.to_string(),
                raw_content: raw_content.to_string(),
            }),
        })
    }
}

/// Service that provides type descriptions to remote nodes.
///
/// This service implements the `GetTypeDescription` service as defined in
/// `type_description_interfaces/srv/GetTypeDescription.srv`. Nodes can
/// register their message schemas with this service, and other nodes can
/// query for type descriptions by type name.
///
/// The service uses callback mode to handle requests, avoiding the need for
/// Convert a DDS-format type name to the ROS 2 canonical slash format for schema lookup.
///
/// `pkg::msg::dds_::Foo_` → `pkg/msg/Foo`
/// `pkg::msg::Foo` → `pkg/msg/Foo`
/// `pkg/msg/Foo` → `pkg/msg/Foo` (unchanged)
fn normalize_type_name_for_lookup(name: &str) -> String {
    name.replace("::msg::dds_::", "/msg/")
        .replace("::srv::dds_::", "/srv/")
        .replace("::action::dds_::", "/action/")
        .replace("::msg::", "/msg/")
        .replace("::srv::", "/srv/")
        .replace("::action::", "/action/")
        .trim_end_matches('_')
        .to_string()
}

/// a background task that would block on queue.recv().
#[derive(Clone)]
pub struct TypeDescriptionService {
    /// Registered schemas by type name
    schemas: Arc<RwLock<HashMap<String, RegisteredSchema>>>,
    /// The underlying Zenoh service server (callback mode, no queue)
    _server: Arc<ZServer<GetTypeDescription, ()>>,
}

impl TypeDescriptionService {
    /// Create a new TypeDescriptionService.
    ///
    /// # Arguments
    ///
    /// * `session` - The Zenoh session to use
    /// * `node_name` - The name of the node hosting this service
    /// * `namespace` - The namespace of the node
    /// * `node_id` - The node ID
    /// * `counter` - The global counter for entity IDs
    ///
    /// # Returns
    ///
    /// A new `TypeDescriptionService` instance.
    pub fn new(
        session: Arc<Session>,
        node_name: &str,
        namespace: &str,
        node_id: usize,
        counter: &crate::context::GlobalCounter,
        clock: &crate::time::ZClock,
    ) -> ZResult<Self> {
        let schemas: Arc<RwLock<HashMap<String, RegisteredSchema>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Build the service name according to ROS 2 convention
        // The service is exposed as ~get_type_description (private to the node)
        // which expands to /{namespace}/{node_name}/get_type_description
        let service_name = "~get_type_description";

        // Create the node entity for the service
        let node_entity = crate::entity::NodeEntity::new(
            0, // domain_id
            session.zid(),
            node_id,
            node_name.to_string(),
            namespace.to_string(),
            String::new(), // enclave (empty, normalized to "%" in liveliness token)
        );

        let entity = crate::entity::EndpointEntity {
            id: counter.increment(),
            node: Some(node_entity),
            kind: crate::entity::EndpointKind::Service,
            topic: service_name.to_string(),
            type_info: Some(GetTypeDescription::service_type_info()),
            qos: Default::default(),
        };

        // Build the service server with callback mode to avoid blocking tasks
        let server_builder: ZServerBuilder<GetTypeDescription> = ZServerBuilder {
            entity,
            session,
            clock: clock.clone(),
            keyexpr_format: hiroz_protocol::KeyExprFormat::default(),
            _phantom_data: Default::default(),
        };

        // Clone schemas for the callback closure
        let schemas_clone = schemas.clone();

        let server = server_builder.build_with_callback(move |query| {
            // Handle request in the callback
            Self::handle_query(&schemas_clone, query);
        })?;

        info!(
            "[TDS] TypeDescriptionService created for node: {}/{}",
            namespace, node_name
        );

        Ok(Self {
            schemas,
            _server: Arc::new(server),
        })
    }

    /// Register a schema with this service.
    ///
    /// Once registered, remote nodes can query for this type's description.
    pub fn register_schema(
        &self,
        schema: Arc<MessageSchema>,
    ) -> std::result::Result<(), DynamicError> {
        let registered = RegisteredSchema::new(schema.clone())?;
        let type_name = schema.type_name.clone();

        let mut schemas = self
            .schemas
            .write()
            .map_err(|_| DynamicError::RegistryLockPoisoned)?;

        debug!(
            "[TDS] Registering schema: {} (hash: {})",
            type_name, registered.type_hash
        );
        schemas.insert(type_name, registered);

        Ok(())
    }

    /// Register a schema with source file content.
    pub fn register_schema_with_source(
        &self,
        schema: Arc<MessageSchema>,
        encoding: &str,
        raw_content: &str,
    ) -> std::result::Result<(), DynamicError> {
        let registered = RegisteredSchema::with_source(schema.clone(), encoding, raw_content)?;
        let type_name = schema.type_name.clone();

        let mut schemas = self
            .schemas
            .write()
            .map_err(|_| DynamicError::RegistryLockPoisoned)?;

        debug!(
            "[TDS] Registering schema with source: {} (hash: {})",
            type_name, registered.type_hash
        );
        schemas.insert(type_name, registered);

        Ok(())
    }

    /// Unregister a schema by type name.
    pub fn unregister_schema(&self, type_name: &str) -> std::result::Result<(), DynamicError> {
        let mut schemas = self
            .schemas
            .write()
            .map_err(|_| DynamicError::RegistryLockPoisoned)?;

        if schemas.remove(type_name).is_some() {
            debug!("[TDS] Unregistered schema: {}", type_name);
        } else {
            trace!("[TDS] Schema not found for unregister: {}", type_name);
        }

        Ok(())
    }

    /// Get a registered schema by type name.
    pub fn get_schema(
        &self,
        type_name: &str,
    ) -> std::result::Result<Option<RegisteredSchema>, DynamicError> {
        let schemas = self
            .schemas
            .read()
            .map_err(|_| DynamicError::RegistryLockPoisoned)?;

        Ok(schemas.get(type_name).cloned())
    }

    /// List all registered type names.
    pub fn list_types(&self) -> std::result::Result<Vec<String>, DynamicError> {
        let schemas = self
            .schemas
            .read()
            .map_err(|_| DynamicError::RegistryLockPoisoned)?;

        Ok(schemas.keys().cloned().collect())
    }

    /// Handle a Zenoh query for type description.
    ///
    /// This is called from the Zenoh callback when a query is received.
    /// It deserializes the request, looks up the schema, and sends the response.
    fn handle_query(schemas: &Arc<RwLock<HashMap<String, RegisteredSchema>>>, query: Query) {
        use crate::msg::{SerdeCdrSerdes, ZDeserializer, ZSerializer};

        // Deserialize the request
        let request: GetTypeDescriptionRequest = match query.payload() {
            Some(payload) => match SerdeCdrSerdes::deserialize(payload.to_bytes().as_ref()) {
                Ok(req) => req,
                Err(e) => {
                    warn!("[TDS] Failed to deserialize request: {}", e);
                    return;
                }
            },
            None => {
                warn!("[TDS] Query has no payload");
                return;
            }
        };

        info!(
            "[TDS] Received request for type: {}, hash: {}",
            request.type_name,
            if request.type_hash.is_empty() {
                "(none)"
            } else {
                &request.type_hash
            }
        );

        // Build the response
        let response = Self::build_response(schemas, &request);

        info!(
            "[TDS] Sending response: successful={}, type={}",
            response.successful, response.type_description.type_description.type_name
        );

        // Serialize and send the response
        let bytes = SerdeCdrSerdes::serialize(&response);
        use zenoh::Wait;
        let mut reply = query.reply(query.key_expr().clone(), bytes);
        if let Some(att_bytes) = query.attachment()
            && let Ok(att) = Attachment::try_from(att_bytes)
        {
            reply = reply.attachment(att);
        }
        if let Err(e) = reply.wait() {
            warn!("[TDS] Failed to send response: {}", e);
        }
    }

    /// Build a GetTypeDescription response for the given request.
    fn build_response(
        schemas: &Arc<RwLock<HashMap<String, RegisteredSchema>>>,
        request: &GetTypeDescriptionRequest,
    ) -> GetTypeDescriptionResponse {
        // Look up the schema
        let schemas_guard = match schemas.read() {
            Ok(s) => s,
            Err(_) => {
                return GetTypeDescriptionResponse {
                    successful: false,
                    failure_reason: "Internal error: registry lock poisoned".to_string(),
                    type_description: Default::default(),
                    type_sources: vec![],
                    extra_information: vec![],
                };
            }
        };

        // Normalize the requested type name so queries using the DDS format
        // (e.g. pkg::msg::dds_::Foo_ from rmw_zenoh_cpp clients) resolve to
        // the canonical slash format used internally (pkg/msg/Foo).
        let lookup_name = normalize_type_name_for_lookup(&request.type_name);
        let registered = match schemas_guard
            .get(lookup_name.as_str())
            .or_else(|| schemas_guard.get(request.type_name.as_str()))
        {
            Some(r) => r,
            None => {
                debug!("[TDS] Type not found: {}", request.type_name);
                return GetTypeDescriptionResponse {
                    successful: false,
                    failure_reason: format!("Type '{}' not registered", request.type_name),
                    type_description: Default::default(),
                    type_sources: vec![],
                    extra_information: vec![],
                };
            }
        };

        // Validate type hash if provided
        if !request.type_hash.is_empty() && request.type_hash != registered.type_hash {
            warn!(
                "[TDS] Type hash mismatch for {}: expected {}, got {}",
                request.type_name, registered.type_hash, request.type_hash
            );
            return GetTypeDescriptionResponse {
                successful: false,
                failure_reason: format!(
                    "Type hash mismatch: expected {}, got {}",
                    registered.type_hash, request.type_hash
                ),
                type_description: Default::default(),
                type_sources: vec![],
                extra_information: vec![],
            };
        }

        // Convert schema to wire format
        let type_description = match schema_to_wire_type_description(&registered.schema) {
            Ok(td) => td,
            Err(e) => {
                warn!("[TDS] Failed to convert schema: {}", e);
                return GetTypeDescriptionResponse {
                    successful: false,
                    failure_reason: format!("Failed to convert schema: {}", e),
                    type_description: Default::default(),
                    type_sources: vec![],
                    extra_information: vec![],
                };
            }
        };

        // Include type sources if requested
        let type_sources = if request.include_type_sources {
            collect_type_sources(registered, &schemas_guard)
        } else {
            vec![]
        };

        debug!(
            "[TDS] Returning type description for: {}",
            request.type_name
        );

        GetTypeDescriptionResponse {
            successful: true,
            failure_reason: String::new(),
            type_description,
            type_sources,
            extra_information: vec![WireKeyValue {
                key: "hiroz_version".to_string(),
                value: env!("CARGO_PKG_VERSION").to_string(),
            }],
        }
    }
}

/// Convert a MessageSchema to wire format TypeDescription.
pub fn schema_to_wire_type_description(
    schema: &MessageSchema,
) -> std::result::Result<WireTypeDescription, DynamicError> {
    let type_desc_msg = schema.to_type_description_msg()?;

    Ok(WireTypeDescription {
        type_description: individual_to_wire(&type_desc_msg.type_description),
        referenced_type_descriptions: type_desc_msg
            .referenced_type_descriptions
            .iter()
            .map(individual_to_wire)
            .collect(),
    })
}

/// Convert an individual TypeDescription to wire format.
fn individual_to_wire(td: &hiroz_schema::TypeDescription) -> WireIndividualTypeDescription {
    WireIndividualTypeDescription {
        type_name: td.type_name.clone(),
        fields: td.fields.iter().map(field_to_wire).collect(),
    }
}

/// Convert a FieldDescription to wire format.
fn field_to_wire(fd: &hiroz_schema::FieldDescription) -> WireField {
    WireField {
        name: fd.name.clone(),
        field_type: WireFieldType {
            type_id: fd.field_type.type_id,
            capacity: fd.field_type.capacity,
            string_capacity: fd.field_type.string_capacity,
            nested_type_name: fd.field_type.nested_type_name.clone(),
        },
        default_value: fd.default_value.clone(),
    }
}

/// Collect type sources for the schema and its referenced types.
fn collect_type_sources(
    registered: &RegisteredSchema,
    all_schemas: &HashMap<String, RegisteredSchema>,
) -> Vec<WireTypeSource> {
    let mut sources = Vec::new();

    // Add source for the main type
    if let Some(source) = &registered.source {
        sources.push(WireTypeSource {
            type_name: registered.schema.type_name.clone(),
            encoding: source.encoding.clone(),
            raw_file_contents: source.raw_content.clone(),
        });
    }

    // Add sources for referenced types (if they're registered and have sources)
    if let Ok(type_desc_msg) = registered.schema.to_type_description_msg() {
        for ref_td in &type_desc_msg.referenced_type_descriptions {
            if let Some(ref_registered) = all_schemas.get(&ref_td.type_name)
                && let Some(ref_source) = &ref_registered.source
            {
                sources.push(WireTypeSource {
                    type_name: ref_td.type_name.clone(),
                    encoding: ref_source.encoding.clone(),
                    raw_file_contents: ref_source.raw_content.clone(),
                });
            }
        }
    }

    sources
}

/// Convert from wire format TypeDescription to hiroz-schema TypeDescriptionMsg.
///
/// This is useful when receiving a TypeDescription from a remote node and
/// wanting to convert it to a MessageSchema.
pub fn wire_to_schema_type_description(
    wire: &WireTypeDescription,
) -> hiroz_schema::TypeDescriptionMsg {
    hiroz_schema::TypeDescriptionMsg {
        type_description: wire_to_schema_individual(&wire.type_description),
        referenced_type_descriptions: wire
            .referenced_type_descriptions
            .iter()
            .map(wire_to_schema_individual)
            .collect(),
    }
}

/// Convert a wire format IndividualTypeDescription to hiroz-schema.
fn wire_to_schema_individual(
    wire: &WireIndividualTypeDescription,
) -> hiroz_schema::TypeDescription {
    hiroz_schema::TypeDescription {
        type_name: wire.type_name.clone(),
        fields: wire.fields.iter().map(wire_to_schema_field).collect(),
    }
}

/// Convert a wire format Field to hiroz-schema FieldDescription.
fn wire_to_schema_field(wire: &WireField) -> hiroz_schema::FieldDescription {
    hiroz_schema::FieldDescription {
        name: wire.name.clone(),
        field_type: hiroz_schema::FieldTypeDescription {
            type_id: wire.field_type.type_id,
            capacity: wire.field_type.capacity,
            string_capacity: wire.field_type.string_capacity,
            nested_type_name: wire.field_type.nested_type_name.clone(),
        },
        default_value: wire.default_value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::schema::FieldType;

    #[test]
    fn test_registered_schema_creation() {
        let schema = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let registered = RegisteredSchema::new(schema).unwrap();
        assert!(registered.type_hash.starts_with("RIHS01_"));
        assert!(registered.source.is_none());
    }

    #[test]
    fn test_registered_schema_with_source() {
        let schema = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let registered = RegisteredSchema::with_source(schema, "msg", "string data").unwrap();

        assert!(registered.source.is_some());
        let source = registered.source.as_ref().unwrap();
        assert_eq!(source.encoding, "msg");
        assert_eq!(source.raw_content, "string data");
    }

    #[test]
    fn test_schema_to_wire_conversion() {
        let schema = MessageSchema::builder("geometry_msgs/msg/Point")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let wire = schema_to_wire_type_description(&schema).unwrap();

        assert_eq!(wire.type_description.type_name, "geometry_msgs/msg/Point");
        assert_eq!(wire.type_description.fields.len(), 3);
        assert_eq!(wire.type_description.fields[0].name, "x");
        assert_eq!(wire.type_description.fields[0].field_type.type_id, 11); // FLOAT64
    }

    #[test]
    fn test_wire_to_schema_roundtrip() {
        let schema = MessageSchema::builder("std_msgs/msg/Int32")
            .field("data", FieldType::Int32)
            .build()
            .unwrap();

        // Schema -> Wire
        let wire = schema_to_wire_type_description(&schema).unwrap();

        // Wire -> Schema (TypeDescriptionMsg)
        let schema_td = wire_to_schema_type_description(&wire);

        assert_eq!(schema_td.type_description.type_name, "std_msgs/msg/Int32");
        assert_eq!(schema_td.type_description.fields.len(), 1);
        assert_eq!(schema_td.type_description.fields[0].name, "data");
    }

    #[test]
    fn test_nested_type_wire_conversion() {
        let vector3_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let twist_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
            .field("linear", FieldType::Message(vector3_schema.clone()))
            .field("angular", FieldType::Message(vector3_schema))
            .build()
            .unwrap();

        let wire = schema_to_wire_type_description(&twist_schema).unwrap();

        assert_eq!(wire.type_description.type_name, "geometry_msgs/msg/Twist");
        assert_eq!(wire.type_description.fields.len(), 2);

        // Should have one referenced type (Vector3)
        assert_eq!(wire.referenced_type_descriptions.len(), 1);
        assert_eq!(
            wire.referenced_type_descriptions[0].type_name,
            "geometry_msgs/msg/Vector3"
        );
    }

    #[test]
    fn test_request_response_default() {
        let req = GetTypeDescriptionRequest::default();
        assert!(req.type_name.is_empty());
        assert!(req.type_hash.is_empty());
        assert!(!req.include_type_sources);

        let resp = GetTypeDescriptionResponse::default();
        assert!(!resp.successful);
        assert!(resp.failure_reason.is_empty());
    }
}
