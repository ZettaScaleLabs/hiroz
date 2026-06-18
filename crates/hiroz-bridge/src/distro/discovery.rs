//! Discovery: subscribe to `@ros2_lv/{domain_id}/**` and classify entities.
//!
//! Entities coming from a Humble network will have `TypeHashNotSupported` as
//! their type hash (which `TypeHash::from_rihs_string` maps to `TypeHash::zero()`).
//! Entities from a Jazzy network will have a real RIHS01 hash.
//!
//! This module does not forward any data — it only surfaces discovery events
//! to the [`Bridge`][crate::bridge::Bridge] via callbacks.

use std::sync::Arc;

use anyhow::Result;
use hiroz_protocol::{Entity, TypeHash};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use zenoh::{Session, Wait, sample::SampleKind};

use super::ke::is_humble_hash;

/// Which distro was the entity announced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distro {
    /// Hash is `TypeHashNotSupported` (all-zero sentinel).
    Humble,
    /// Hash is a real RIHS01 value.
    Jazzy,
}

/// A discovery event sent over the channel.
#[derive(Debug, Clone)]
pub struct DiscoveryEvent {
    /// The parsed entity.
    pub entity: Arc<Entity>,
    /// Inferred distro based on hash variant.
    pub distro: Distro,
    /// Whether the entity appeared (`true`) or disappeared (`false`).
    pub appeared: bool,
    /// The raw liveliness key expression (used for re-announcement on the other side).
    pub raw_ke: String,
}

/// Classify a `TypeHash` as Humble or Jazzy.
pub fn classify_hash(hash: &TypeHash) -> Distro {
    if is_humble_hash(hash) {
        Distro::Humble
    } else {
        Distro::Jazzy
    }
}

/// Classify an `Entity` as Humble or Jazzy.
///
/// Nodes have no type hash, so we cannot determine their distro from the
/// liveliness key expression alone.  We return `None` for nodes.
pub fn classify_entity(entity: &Entity) -> Option<Distro> {
    match entity {
        Entity::Node(_) => None,
        Entity::Endpoint(ep) => {
            let hash = ep.type_info.as_ref().map(|ti| &ti.hash)?;
            Some(classify_hash(hash))
        }
    }
}

/// Subscribe to liveliness tokens on `session` and send `DiscoveryEvent`s on
/// the returned channel.
///
/// `domain_id` filters the subscription pattern.  The caller is responsible
/// for driving the returned `JoinHandle`.
pub fn start_discovery(
    session: Arc<Session>,
    domain_id: usize,
) -> Result<mpsc::UnboundedReceiver<DiscoveryEvent>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let pattern = format!("@ros2_lv/{domain_id}/**");

    debug!("Starting discovery on pattern: {pattern}");

    let sub = session
        .liveliness()
        .declare_subscriber(&pattern)
        .history(true)
        .callback({
            let tx = tx.clone();
            move |sample| {
                let ke = sample.key_expr().to_owned();
                let appeared = sample.kind() == SampleKind::Put;

                // Parse using the rmw_zenoh formatter.
                let format = hiroz_protocol::KeyExprFormat::default();
                let entity = match format.parse_liveliness(&ke) {
                    Ok(e) => Arc::new(e),
                    Err(err) => {
                        warn!("discovery: failed to parse liveliness KE {ke}: {err}");
                        return;
                    }
                };

                let distro = match classify_entity(&entity) {
                    Some(d) => d,
                    None => {
                        // Node-only token — no type hash, skip distro classification.
                        debug!("discovery: node token {ke}, skipping");
                        return;
                    }
                };

                debug!("discovery: {:?} entity appeared={appeared} ke={ke}", distro);

                let event = DiscoveryEvent {
                    entity,
                    distro,
                    appeared,
                    raw_ke: ke.to_string(),
                };

                // Ignore send errors (receiver dropped = bridge shutting down).
                let _ = tx.send(event);
            }
        })
        .wait();

    // Keep the subscriber alive by leaking it into a background task.
    tokio::spawn(async move {
        // Hold subscriber until the channel is closed.
        let _sub = sub;
        // Wait for channel to be dropped (receiver side gone = bridge shutdown).
        tx.closed().await;
    });

    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiroz_protocol::{
        EndpointKind, TypeHash, TypeInfo,
        entity::{EndpointEntity, NodeEntity},
    };

    fn make_endpoint(hash: TypeHash) -> Entity {
        Entity::Endpoint(EndpointEntity {
            id: 1,
            node: Some(NodeEntity {
                domain_id: 0,
                z_id: "1234567890abcdef1234567890abcdef".parse().unwrap(),
                id: 0,
                name: "test_node".to_string(),
                namespace: "/".to_string(),
                enclave: String::new(),
            }),
            kind: EndpointKind::Publisher,
            topic: "/chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs::msg::dds_::String_", hash)),
            qos: Default::default(),
        })
    }

    #[test]
    fn classify_humble_endpoint() {
        let entity = make_endpoint(TypeHash::zero());
        assert_eq!(classify_entity(&entity), Some(Distro::Humble));
    }

    #[test]
    fn classify_jazzy_endpoint() {
        let hash = TypeHash::new(1, [0xabu8; 32]);
        let entity = make_endpoint(hash);
        assert_eq!(classify_entity(&entity), Some(Distro::Jazzy));
    }

    #[test]
    fn classify_node_returns_none() {
        let entity = Entity::Node(NodeEntity {
            domain_id: 0,
            z_id: "1234567890abcdef1234567890abcdef".parse().unwrap(),
            id: 0,
            name: "some_node".to_string(),
            namespace: "/".to_string(),
            enclave: String::new(),
        });
        assert_eq!(classify_entity(&entity), None);
    }

    #[test]
    fn classify_endpoint_no_type_info_returns_none() {
        let entity = Entity::Endpoint(EndpointEntity {
            id: 2,
            node: Some(NodeEntity {
                domain_id: 0,
                z_id: "1234567890abcdef1234567890abcdef".parse().unwrap(),
                id: 0,
                name: "no_type_node".to_string(),
                namespace: "/".to_string(),
                enclave: String::new(),
            }),
            kind: EndpointKind::Publisher,
            topic: "/anon".to_string(),
            type_info: None,
            qos: Default::default(),
        });
        assert_eq!(classify_entity(&entity), None);
    }

    #[test]
    fn classify_hash_zero_is_humble() {
        assert_eq!(classify_hash(&TypeHash::zero()), Distro::Humble);
    }

    #[test]
    fn classify_hash_non_zero_is_jazzy() {
        let h = TypeHash::new(1, {
            let mut b = [0u8; 32];
            b[0] = 1;
            b
        });
        assert_eq!(classify_hash(&h), Distro::Jazzy);
    }
}
