//! Entity types for ROS 2 entities in hiroz.
//!
//! This module re-exports entity types from hiroz-protocol and adds
//! hiroz-specific extensions.

// Re-export all entity types from hiroz-protocol
pub use hiroz_protocol::entity::*;

use zenoh::{Result, key_expr::KeyExpr};

// Constants for hiroz-specific functionality
pub const ADMIN_SPACE: &str = "@ros2_lv";

// Type aliases
type NodeName = String;
type NodeNamespace = String;
pub type NodeKey = (NodeNamespace, NodeName);
pub type Topic = String;

// Extension functions for NodeEntity (can't use impl due to orphan rules)

/// Normalize a node namespace for internal storage.
///
/// The root namespace (`"/"`) is stored as an empty string so local and remote
/// entities use the same key representation.
pub fn normalize_node_namespace(namespace: &str) -> String {
    if namespace == "/" {
        String::new()
    } else {
        namespace.to_owned()
    }
}

/// Get the key for this node (namespace, name)
pub fn node_key(entity: &NodeEntity) -> NodeKey {
    (
        normalize_node_namespace(&entity.namespace),
        entity.name.clone(),
    )
}

/// Get the liveliness token key expression for a node
pub fn node_lv_token_key_expr(entity: &NodeEntity) -> Result<KeyExpr<'static>> {
    let ke = node_to_liveliness_ke(entity)?;
    Ok(ke.0)
}

// Extension functions for EndpointEntity

/// Get the GID (globally unique identifier) for this endpoint.
/// Returns `None` for endpoints without node identity (e.g. Ros2Dds-format liveliness tokens).
pub fn endpoint_gid(entity: &EndpointEntity) -> Option<crate::attachment::GidArray> {
    use sha2::Digest;
    let node = entity.node.as_ref()?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(node.z_id.to_le_bytes());
    hasher.update(entity.id.to_le_bytes());
    let hash = hasher.finalize();
    let mut gid = [0u8; 16];
    gid.copy_from_slice(&hash[..16]);
    Some(gid)
}

// Helper functions for converting entities to LivelinessKE
// Note: Can't implement TryFrom due to orphan rules (both types are from hiroz-protocol)

/// Convert a NodeEntity to a LivelinessKE using the default format
pub fn node_to_liveliness_ke(entity: &NodeEntity) -> Result<LivelinessKE> {
    let format = hiroz_protocol::KeyExprFormat::default();
    format.node_liveliness_key_expr(entity)
}

/// Convert an EndpointEntity to a LivelinessKE using the default format
pub fn endpoint_to_liveliness_ke(entity: &EndpointEntity) -> Result<LivelinessKE> {
    let format = hiroz_protocol::KeyExprFormat::default();
    let Some(node) = entity.node.as_ref() else {
        return Err(zenoh::Error::from(
            "endpoint liveliness requires node identity",
        ));
    };
    format.liveliness_key_expr(entity, &node.z_id)
}

/// Convert an Entity to a LivelinessKE using the default format
pub fn entity_to_liveliness_ke(entity: &Entity) -> Result<LivelinessKE> {
    match entity {
        Entity::Node(n) => node_to_liveliness_ke(n),
        Entity::Endpoint(e) => endpoint_to_liveliness_ke(e),
    }
}

/// Get the kind of entity
pub fn entity_kind(entity: &Entity) -> EntityKind {
    match entity {
        Entity::Node(_) => EntityKind::Node,
        Entity::Endpoint(e) => e.entity_kind(),
    }
}

/// Get the endpoint entity if this is an endpoint
pub fn entity_get_endpoint(entity: &Entity) -> Option<&EndpointEntity> {
    match entity {
        Entity::Node(_) => None,
        Entity::Endpoint(e) => Some(e),
    }
}
