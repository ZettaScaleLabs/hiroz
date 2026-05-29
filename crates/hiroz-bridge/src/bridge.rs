//! Top-level bridge orchestrator.
//!
//! Creates two Zenoh sessions (one for Humble, one for Jazzy), starts
//! discovery on both, and wires up forwarders as topics are discovered.
//!
//! # Forwarding strategy
//!
//! When a Humble publisher is discovered:
//!   - We forward: humble KE (TypeHashNotSupported) → jazzy KE (RIHS01_<hash>)
//!   - We also forward the reverse direction so Jazzy nodes can receive Humble data
//!
//! Services use queryable-based forwarding (see [`crate::forwarder`]).
//! Actions are independent sub-entities (send_goal, cancel_goal, get_result,
//! feedback, status) and are bridged individually via their KEs.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use hiroz_protocol::{EndpointKind, TypeHash};
use parking_lot::RwLock;
use zenoh::{Wait, liveliness::LivelinessToken};

use crate::{
    discovery::{self, DiscoveryEvent, Distro},
    forwarder::{
        ForwarderHandle, ServiceForwarderHandle, start_forwarder, start_service_forwarder,
    },
    hash_registry,
    ke::HUMBLE_HASH_SENTINEL,
};

// ---------------------------------------------------------------------------
// BridgedEntry
// ---------------------------------------------------------------------------

/// Canonical entity kind for deduplication: Publisher and Subscription are treated
/// as the same "pubsub" endpoint since they use the same bidirectional forwarder pair.
///
/// Using Publisher/Subscription as separate keys would cause two separate forwarder
/// pairs to be created when both a Publisher and a Subscription are discovered for
/// the same topic (e.g., a Humble publisher AND a Jazzy subscriber for `/chatter`).
/// The second pair's subscriber declaration arrives AFTER the 1.6.2 humble talker has
/// connected, triggering a 1.9.0-style Interest that the 1.6.2 peer doesn't understand.
/// That can reset the peer's routing state and break delivery.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CanonicalKind {
    PubSub,
    Service,
}

impl From<EndpointKind> for CanonicalKind {
    fn from(k: EndpointKind) -> Self {
        match k {
            EndpointKind::Publisher | EndpointKind::Subscription => CanonicalKind::PubSub,
            EndpointKind::Service | EndpointKind::Client => CanonicalKind::Service,
            // ActionServer/ActionClient are synthetic graph kinds; the bridge sees their
            // underlying service/pubsub sub-endpoints with their original kinds.
            EndpointKind::ActionServer | EndpointKind::ActionClient => CanonicalKind::Service,
        }
    }
}

/// Key identifying a unique bridged topic/service/action sub-entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BridgeKey {
    /// Topic / service name (without domain prefix).
    topic: String,
    /// Fully-qualified ROS type name.
    type_name: String,
    /// Canonical entity kind (Publisher and Subscription share the same key).
    kind: CanonicalKind,
}

/// Forwarding handles for a bridged entity — pub/sub pair or service proxy.
///
/// Exactly one variant is active per entity; dropping tears down the forwarders.
enum ForwarderPair {
    PubSub {
        _h2j: ForwarderHandle,
        _j2h: ForwarderHandle,
    },
    /// A single unidirectional service proxy placed on the side opposite the server.
    ///
    /// Only one proxy is needed per service: it intercepts calls from clients on
    /// one side and forwards them to the actual server on the other side.
    /// Creating bidirectional proxies would cause queries to loop infinitely.
    Service { _proxy: ServiceForwarderHandle },
}

/// All handles for a single bridged entity.
///
/// Dropping this struct tears down the forwarders and withdraws the synthetic
/// liveliness tokens from both sides.
struct BridgedEntry {
    _forwarders: ForwarderPair,
    /// Re-announces a Humble entity on the Jazzy session (RIHS01 hash).
    _jazzy_lv: Option<LivelinessToken>,
    /// Re-announces a Jazzy entity on the Humble session (TypeHashNotSupported).
    _humble_lv: Option<LivelinessToken>,
}

// ---------------------------------------------------------------------------
// Bridge
// ---------------------------------------------------------------------------

/// The bridge holds the two Zenoh sessions and drives the event loop.
pub struct Bridge {
    humble_session: Arc<zenoh::Session>,
    jazzy_session: Arc<zenoh::Session>,
    domain_id: usize,
    /// Active bridged entries, keyed by (topic, type, kind).
    active: Arc<RwLock<HashMap<BridgeKey, BridgedEntry>>>,
}

/// Rewrite the hash segment of a liveliness KE.
///
/// Finds the segment matching `is_target` and replaces it with `replacement`.
/// All other segments are passed through unchanged.
fn rewrite_lv_ke(raw_ke: &str, is_target: impl Fn(&str) -> bool, replacement: &str) -> String {
    raw_ke
        .split('/')
        .map(|seg| if is_target(seg) { replacement } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

fn lv_ke_to_jazzy(raw_ke: &str, jazzy_hash: &TypeHash) -> String {
    let rihs = jazzy_hash.to_rihs_string();
    rewrite_lv_ke(raw_ke, |s| s == HUMBLE_HASH_SENTINEL, &rihs)
}

fn lv_ke_to_humble(raw_ke: &str) -> String {
    rewrite_lv_ke(raw_ke, |s| s.starts_with("RIHS01_"), HUMBLE_HASH_SENTINEL)
}

/// Build a Zenoh CLIENT-mode config for a bridge session connecting to a single endpoint.
///
/// CLIENT mode is required to avoid zenoh-c 1.6.x P2P routing breakage with zenoh 1.9.0.
///
/// With zenoh 1.9.0 default PEER mode, sessions eagerly gossip-connect to ALL discovered
/// peers, including zenoh-c 1.6.x nodes.  Two problems arise:
///
/// 1. The bridge's peer session sends its 1.9.0 interest declarations over the new P2P link.
///    zenoh-c 1.6.2 does NOT understand the 1.9.0 interest protocol → silently drops them.
///    The 1.6.2 node then prefers the (now broken) P2P route over the router route, causing
///    publications from the humble talker to never reach the bridge's subscriber.
///
/// 2. Gossip target restriction alone (`"peer": ["router"]`) is insufficient because
///    zenoh-c 1.6.2 can initiate a P2P connection TO the bridge's peer sessions.  The
///    bridge side never refuses incoming P2P connections, so the broken P2P route is
///    still established.
///
/// CLIENT mode completely avoids P2P: clients never form direct peer-to-peer connections.
/// All traffic goes through the router, where the 1.6.2 ↔ 1.9.0 interest mismatch is
/// harmless (the router handles interest on behalf of all connected sessions).
fn build_session_config(endpoint: &str) -> Result<zenoh::Config> {
    let mut config = zenoh::Config::default();
    // CLIENT mode: never form direct P2P connections, always go through the router.
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config insert mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .map_err(|e| anyhow::anyhow!("config insert connect/endpoints: {e}"))?;
    // Disable multicast — connect to router explicitly.
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .map_err(|e| anyhow::anyhow!("config insert scouting/multicast: {e}"))?;
    Ok(config)
}

impl Bridge {
    /// Create a new bridge connecting to `humble_endpoint` and `jazzy_endpoint`.
    pub async fn new(
        humble_endpoint: &str,
        jazzy_endpoint: &str,
        domain_id: usize,
    ) -> Result<Self> {
        let humble_cfg = build_session_config(humble_endpoint)?;
        let jazzy_cfg = build_session_config(jazzy_endpoint)?;

        let humble_session = Arc::new(
            zenoh::open(humble_cfg)
                .await
                .map_err(|e| anyhow::anyhow!("humble session open: {e}"))?,
        );
        let jazzy_session = Arc::new(
            zenoh::open(jazzy_cfg)
                .await
                .map_err(|e| anyhow::anyhow!("jazzy session open: {e}"))?,
        );

        Ok(Self {
            humble_session,
            jazzy_session,
            domain_id,
            active: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Run the bridge event loop until a shutdown signal is received.
    pub async fn run(&mut self) -> Result<()> {
        let mut humble_rx =
            discovery::start_discovery(Arc::clone(&self.humble_session), self.domain_id)?;
        let mut jazzy_rx =
            discovery::start_discovery(Arc::clone(&self.jazzy_session), self.domain_id)?;

        tracing::info!("Bridge running — waiting for discovery events");

        loop {
            tokio::select! {
                event = humble_rx.recv() => {
                    match event {
                        Some(e) => self.handle_event(e),
                        None => {
                            tracing::warn!("Humble discovery channel closed");
                            break;
                        }
                    }
                }
                event = jazzy_rx.recv() => {
                    match event {
                        Some(e) => self.handle_event(e),
                        None => {
                            tracing::warn!("Jazzy discovery channel closed");
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received SIGINT, shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn handle_event(&self, event: DiscoveryEvent) {
        use hiroz_protocol::Entity;

        let ep = match &*event.entity {
            Entity::Node(_) => return,
            Entity::Endpoint(ep) => ep,
        };

        let type_info = match &ep.type_info {
            Some(ti) => ti,
            None => return,
        };

        // Skip Client endpoints — they are handled when their Server is discovered
        // (creating a single bidirectional proxy).
        if ep.kind == EndpointKind::Client {
            return;
        }

        let key = BridgeKey {
            topic: ep.topic.clone(),
            type_name: type_info.name.clone(),
            kind: ep.kind.into(),
        };

        if event.appeared {
            // Skip if already bridged.  This prevents re-entrancy loops caused by
            // the bridge's own synthetic liveliness tokens triggering new bridge_entity
            // calls on both sessions (both subscribe to the same shared router).
            if self.active.read().contains_key(&key) {
                tracing::debug!(
                    "Entity already bridged, skipping: topic={} type={}",
                    ep.topic,
                    type_info.name
                );
                return;
            }
            tracing::info!(
                "Entity appeared: {:?} {:?} topic={} type={}",
                event.distro,
                ep.kind,
                ep.topic,
                type_info.name
            );
            self.bridge_entity(key, &type_info.hash, event.distro, &event.raw_ke);
        } else {
            tracing::info!(
                "Entity disappeared: topic={} type={}",
                ep.topic,
                type_info.name
            );
            self.active.write().remove(&key);
        }
    }

    /// Set up forwarders for a discovered entity and re-announce it on the opposite side.
    fn bridge_entity(
        &self,
        key: BridgeKey,
        discovered_hash: &TypeHash,
        from_distro: Distro,
        raw_lv_ke: &str,
    ) {
        // Resolve the Jazzy hash: Humble entities carry all-zero hash, so look up
        // the real RIHS01 hash from the compile-time registry.
        let jazzy_hash = match from_distro {
            Distro::Jazzy => discovered_hash.clone(),
            Distro::Humble => match hash_registry::lookup(&key.type_name) {
                Some(h) => h.clone(),
                None => {
                    tracing::warn!(
                        "Unknown type {} — cannot bridge (no hash in registry)",
                        key.type_name
                    );
                    return;
                }
            },
        };

        let d = self.domain_id;
        let topic_stripped = key.topic.strip_prefix('/').unwrap_or(&key.topic);
        let humble_ke = format!(
            "{d}/{topic_stripped}/{}/{}",
            key.type_name, HUMBLE_HASH_SENTINEL
        );
        let jazzy_ke = format!(
            "{d}/{topic_stripped}/{}/{}",
            key.type_name,
            jazzy_hash.to_rihs_string()
        );

        tracing::debug!("Bridging: humble={humble_ke}  jazzy={jazzy_ke}");

        // Re-announce the entity on the opposite side so `ros2 topic list` / graph
        // introspection sees it.
        let (jazzy_lv, humble_lv) = match from_distro {
            Distro::Humble => {
                let rewritten = lv_ke_to_jazzy(raw_lv_ke, &jazzy_hash);
                match self
                    .jazzy_session
                    .liveliness()
                    .declare_token(&rewritten)
                    .wait()
                {
                    Ok(t) => {
                        tracing::debug!("Declared synthetic Jazzy lv token: {rewritten}");
                        (Some(t), None)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to declare synthetic Jazzy lv token: {e}");
                        (None, None)
                    }
                }
            }
            Distro::Jazzy => {
                let rewritten = lv_ke_to_humble(raw_lv_ke);
                match self
                    .humble_session
                    .liveliness()
                    .declare_token(&rewritten)
                    .wait()
                {
                    Ok(t) => {
                        tracing::debug!("Declared synthetic Humble lv token: {rewritten}");
                        (None, Some(t))
                    }
                    Err(e) => {
                        tracing::warn!("Failed to declare synthetic Humble lv token: {e}");
                        (None, None)
                    }
                }
            }
        };

        let forwarders = match key.kind {
            CanonicalKind::PubSub => self.setup_pubsub_bridge(&humble_ke, &jazzy_ke),
            CanonicalKind::Service => {
                // Bridge only when the server is discovered; place the proxy on the
                // opposite side so clients can reach the server.  Bridging on Client
                // discovery would create a second proxy causing an infinite query loop.
                self.setup_service_bridge(&humble_ke, &jazzy_ke, from_distro)
            }
        };

        match forwarders {
            Ok(fp) => {
                let entry = BridgedEntry {
                    _forwarders: fp,
                    _jazzy_lv: jazzy_lv,
                    _humble_lv: humble_lv,
                };
                // Hold the lock for the entire check+insert to prevent a second
                // discovery event from bridging the same entity concurrently.
                self.active.write().entry(key).or_insert(entry);
            }
            Err(err) => {
                tracing::error!("Failed to set up bridge for {}: {err}", key.topic);
            }
        }
    }

    fn setup_pubsub_bridge(&self, humble_ke: &str, jazzy_ke: &str) -> Result<ForwarderPair> {
        let h2j = start_forwarder(
            Arc::clone(&self.humble_session),
            humble_ke.to_string(),
            Arc::clone(&self.jazzy_session),
            jazzy_ke.to_string(),
        )
        .map_err(|e| anyhow::anyhow!("humble→jazzy forwarder: {e}"))?;

        let j2h = start_forwarder(
            Arc::clone(&self.jazzy_session),
            jazzy_ke.to_string(),
            Arc::clone(&self.humble_session),
            humble_ke.to_string(),
        )
        .map_err(|e| anyhow::anyhow!("jazzy→humble forwarder: {e}"))?;

        Ok(ForwarderPair::PubSub {
            _h2j: h2j,
            _j2h: j2h,
        })
    }

    fn setup_service_bridge(
        &self,
        humble_ke: &str,
        jazzy_ke: &str,
        server_distro: Distro,
    ) -> Result<ForwarderPair> {
        // Place the proxy on the side *opposite* the server so that clients on
        // that side can reach the server.  One proxy suffices; two would loop.
        let proxy = match server_distro {
            Distro::Humble => start_service_forwarder(
                Arc::clone(&self.jazzy_session),
                jazzy_ke.to_string(),
                Arc::clone(&self.humble_session),
                humble_ke.to_string(),
            )
            .map_err(|e| anyhow::anyhow!("jazzy→humble service proxy: {e}"))?,
            Distro::Jazzy => start_service_forwarder(
                Arc::clone(&self.humble_session),
                humble_ke.to_string(),
                Arc::clone(&self.jazzy_session),
                jazzy_ke.to_string(),
            )
            .map_err(|e| anyhow::anyhow!("humble→jazzy service proxy: {e}"))?,
        };

        Ok(ForwarderPair::Service { _proxy: proxy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_key_equality() {
        let k1 = BridgeKey {
            topic: "/chatter".to_string(),
            type_name: "std_msgs::msg::dds_::String_".to_string(),
            kind: EndpointKind::Publisher.into(),
        };
        let k2 = k1.clone();
        assert_eq!(k1, k2);
    }

    const HUMBLE_LV_KE: &str = "@ros2_lv/0/aabbccdd1122334455667788aabbccdd/1/2/P//chatter/std_msgs::msg::dds_::String_/TypeHashNotSupported/1";
    const JAZZY_LV_KE: &str = "@ros2_lv/0/aabbccdd1122334455667788aabbccdd/1/2/P//chatter/std_msgs::msg::dds_::String_/RIHS01_abababababababababababababababababababababababababababababababababab/1";

    #[test]
    fn lv_ke_humble_to_jazzy() {
        let hash = TypeHash::new(1, [0xabu8; 32]);
        let rewritten = lv_ke_to_jazzy(HUMBLE_LV_KE, &hash);
        assert!(rewritten.contains("RIHS01_"));
        assert!(!rewritten.contains("TypeHashNotSupported"));
        // All other segments preserved
        assert!(rewritten.contains("@ros2_lv/0/"));
        assert!(rewritten.contains("std_msgs::msg::dds_::String_"));
    }

    #[test]
    fn lv_ke_jazzy_to_humble() {
        let rewritten = lv_ke_to_humble(JAZZY_LV_KE);
        assert!(rewritten.contains("TypeHashNotSupported"));
        assert!(!rewritten.contains("RIHS01_"));
        assert!(rewritten.contains("@ros2_lv/0/"));
        assert!(rewritten.contains("std_msgs::msg::dds_::String_"));
    }

    #[test]
    fn lv_ke_roundtrip() {
        let hash = TypeHash::new(1, [0xabu8; 32]);
        // Humble → Jazzy → Humble should restore the original (modulo the hash value).
        let jazzy = lv_ke_to_jazzy(HUMBLE_LV_KE, &hash);
        let back = lv_ke_to_humble(&jazzy);
        assert_eq!(back, HUMBLE_LV_KE);
    }
}
