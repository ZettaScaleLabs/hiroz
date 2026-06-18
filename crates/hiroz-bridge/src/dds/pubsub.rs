//! ZDdsPubBridge (DDS→Zenoh) and ZDdsSubBridge (Zenoh→DDS).

use std::sync::Arc;

use anyhow::{Result, anyhow};
use hiroz::node::ZNode;
use hiroz_protocol::entity::{EndpointEntity, EndpointKind, TypeHash, TypeInfo};
use zenoh::{Wait, bytes::ZBytes, liveliness::LivelinessToken, pubsub::Publisher};
use zenoh_ext::{
    AdvancedPublisher, AdvancedPublisherBuilderExt, AdvancedSubscriber,
    AdvancedSubscriberBuilderExt, CacheConfig, HistoryConfig,
};

use super::{
    gid::Gid,
    names::{ros2_name_to_dds_pub_topic, ros2_type_to_dds_type},
    participant::{BridgeQos, DdsParticipant, DdsReader, DdsWriter, DurabilityKind, HistoryKind},
    qos::{adapt_reader_qos_for_writer, adapt_writer_qos_for_reader, bridge_qos_to_qos_profile},
};

// ─── Zenoh publisher wrapper ──────────────────────────────────────────────────

enum BridgePublisher {
    Plain(Publisher<'static>),
    Advanced(AdvancedPublisher<'static>),
}

unsafe impl Send for BridgePublisher {}
unsafe impl Sync for BridgePublisher {}

impl BridgePublisher {
    fn put(&self, bytes: ZBytes) -> Result<()> {
        match self {
            Self::Plain(p) => p.put(bytes).wait().map_err(|e| anyhow!("{e}")),
            Self::Advanced(p) => p.put(bytes).wait().map_err(|e| anyhow!("{e}")),
        }
    }
}

// ─── Zenoh subscriber wrapper ─────────────────────────────────────────────────

#[allow(dead_code)]
enum ZenohSubHandle {
    Plain(zenoh::pubsub::Subscriber<()>),
    Advanced(AdvancedSubscriber<()>),
}

unsafe impl Send for ZenohSubHandle {}
unsafe impl Sync for ZenohSubHandle {}

// ─── ZDdsPubBridge ────────────────────────────────────────────────────────────

/// Routes a DDS publication to a Zenoh publisher.
///
/// The DDS reader is created on the raw DDS topic. On every sample the raw CDR bytes are
/// forwarded as-is to a Zenoh publisher declared on the rmw_zenoh (or ros2dds) key expression.
/// A liveliness token is declared so that `ZContext::graph()` and `ros2 topic list` see the
/// bridge as a publisher on the topic.
pub struct ZDdsPubBridge<P: DdsParticipant> {
    _dds_reader: P::Reader,
    _publisher: Arc<BridgePublisher>,
    _lv_token: LivelinessToken,
}

unsafe impl<P: DdsParticipant> Send for ZDdsPubBridge<P> {}
unsafe impl<P: DdsParticipant> Sync for ZDdsPubBridge<P> {}

impl<P: DdsParticipant> ZDdsPubBridge<P> {
    /// Create a new DDS→Zenoh publisher bridge.
    ///
    /// - `ros2_name` — ROS 2 topic name (e.g. `/chatter`)
    /// - `ros2_type` — ROS 2 message type (e.g. `std_msgs/msg/String`)
    /// - `type_hash` — optional RIHS01 type hash from DDS `user_data`; uses `EMPTY_TOPIC_HASH` when `None`
    /// - `qos` — DDS QoS of the upstream publisher
    /// - `keyless` — whether the DDS topic has no key fields
    /// - `cache_multiplier` — TRANSIENT_LOCAL cache depth multiplier (pass 10 for the plugin default)
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        node: &ZNode,
        ros2_name: &str,
        ros2_type: &str,
        type_hash: Option<TypeHash>,
        participant: &P,
        qos: BridgeQos,
        keyless: bool,
        cache_multiplier: usize,
    ) -> Result<Self> {
        let entity_id = node.next_entity_id();
        let qos_profile = bridge_qos_to_qos_profile(&qos);
        let type_info = type_hash
            .as_ref()
            .map(|h| TypeInfo::new(ros2_type, h.clone()));
        let entity = EndpointEntity {
            id: entity_id,
            node: Some(node.node_entity().clone()),
            kind: EndpointKind::Publisher,
            topic: ros2_name.to_string(),
            type_info,
            qos: qos_profile,
        };
        // Liveliness entity always carries the type name (with zero hash when hash unknown)
        // so remote bridges can reconstruct DDS routes for federation.
        let lv_type_hash = type_hash.unwrap_or_else(TypeHash::zero);
        let entity_lv = EndpointEntity {
            id: entity_id,
            node: Some(node.node_entity().clone()),
            kind: EndpointKind::Publisher,
            topic: ros2_name.to_string(),
            type_info: Some(TypeInfo::new(ros2_type, lv_type_hash)),
            qos: qos_profile,
        };

        let topic_ke = node
            .keyexpr_format()
            .topic_key_expr(&entity)
            .map_err(|e| anyhow!("topic_key_expr failed: {e}"))?;
        let lv_ke = node
            .keyexpr_format()
            .liveliness_key_expr(&entity_lv, &node.session().zid())
            .map_err(|e| anyhow!("liveliness_key_expr failed: {e}"))?;

        let ke: zenoh::key_expr::OwnedKeyExpr = topic_ke
            .as_str()
            .to_owned()
            .try_into()
            .map_err(|e| anyhow!("invalid key expr: {e}"))?;

        let is_transient_local = qos
            .durability
            .as_ref()
            .is_some_and(|d| d.kind == DurabilityKind::TransientLocal);

        let publisher = if is_transient_local {
            let cache_size = compute_cache_size(&qos, keyless, cache_multiplier);
            tracing::debug!("ZDdsPubBridge: TRANSIENT_LOCAL {ros2_name}, cache_size={cache_size}");
            let adv = node
                .session()
                .declare_publisher(ke)
                .cache(CacheConfig::default().max_samples(cache_size))
                .publisher_detection()
                .await
                .map_err(|e| anyhow!("declare_publisher(advanced) failed: {e}"))?;
            Arc::new(BridgePublisher::Advanced(adv))
        } else {
            let plain = node
                .session()
                .declare_publisher(ke)
                .await
                .map_err(|e| anyhow!("declare_publisher failed: {e}"))?;
            Arc::new(BridgePublisher::Plain(plain))
        };

        let dds_topic = ros2_name_to_dds_pub_topic(ros2_name);
        let dds_type = ros2_type_to_dds_type(ros2_type);
        let dds_reader_qos = adapt_writer_qos_for_reader(&qos);

        let pub_clone = Arc::clone(&publisher);
        let ke_display = topic_ke.as_str().to_string();
        let dds_reader = participant.create_reader(
            &dds_topic,
            &dds_type,
            keyless,
            dds_reader_qos,
            Box::new(move |bytes: Vec<u8>| {
                if let Err(e) = pub_clone.put(ZBytes::from(bytes.as_slice())) {
                    tracing::warn!("ZDdsPubBridge: Zenoh put failed on {ke_display}: {e}");
                }
            }),
        )?;

        let lv_token = node
            .session()
            .liveliness()
            .declare_token((*lv_ke).clone())
            .await
            .map_err(|e| anyhow!("liveliness token failed: {e}"))?;

        tracing::info!(
            "ZDdsPubBridge: DDS {} → Zenoh {}",
            dds_topic,
            topic_ke.as_str()
        );

        Ok(Self {
            _dds_reader: dds_reader,
            _publisher: publisher,
            _lv_token: lv_token,
        })
    }

    /// Return the DDS reader GID created for this bridge route.
    pub fn reader_guid(&self) -> Option<Gid> {
        self._dds_reader.guid().ok().map(Gid::from)
    }
}

// ─── ZDdsSubBridge ────────────────────────────────────────────────────────────

/// Routes a Zenoh subscription to a DDS writer.
///
/// In rmw-zenoh mode the Zenoh subscriber is declared on a wildcard key (`{domain}/{topic}/**`)
/// so it receives from any Zenoh publisher on the topic regardless of type hash. In ros2dds mode
/// the exact key expression is used. On every sample the raw CDR bytes are forwarded as-is to a
/// DDS writer. A liveliness token is declared with `Subscription` kind so that `ros2 topic list`
/// sees the bridge as a subscriber.
pub struct ZDdsSubBridge<P: DdsParticipant> {
    _writer: Arc<P::Writer>,
    _subscriber: ZenohSubHandle,
    _lv_token: LivelinessToken,
}

unsafe impl<P: DdsParticipant> Send for ZDdsSubBridge<P> {}
unsafe impl<P: DdsParticipant> Sync for ZDdsSubBridge<P> {}

impl<P: DdsParticipant> ZDdsSubBridge<P> {
    /// Create a new Zenoh→DDS subscriber bridge.
    ///
    /// - `ros2_name` — ROS 2 topic name (e.g. `/chatter`)
    /// - `ros2_type` — ROS 2 message type (e.g. `std_msgs/msg/String`)
    /// - `type_hash` — optional RIHS01 type hash; only used in the liveliness token so remote
    ///   bridges reconstruct the correct DDS type. The subscriber key expression is always a
    ///   wildcard in rmw-zenoh mode, so messages from any type hash are forwarded.
    /// - `keyless` — whether the DDS topic has no key fields
    pub async fn new(
        node: &ZNode,
        ros2_name: &str,
        ros2_type: &str,
        type_hash: Option<TypeHash>,
        participant: &P,
        qos: BridgeQos,
        keyless: bool,
    ) -> Result<Self> {
        let entity_id = node.next_entity_id();
        let qos_profile = bridge_qos_to_qos_profile(&qos);
        let lv_type_hash = type_hash.unwrap_or_else(TypeHash::zero);
        // Routing entity: no type_info so topic_key_expr returns the EMPTY placeholder key.
        // We then widen it to a wildcard so the subscriber catches any Zenoh publisher on
        // this topic regardless of what type name / hash the publisher uses.
        let entity = EndpointEntity {
            id: entity_id,
            node: Some(node.node_entity().clone()),
            kind: EndpointKind::Subscription,
            topic: ros2_name.to_string(),
            type_info: None,
            qos: qos_profile,
        };
        let entity_lv = EndpointEntity {
            id: entity_id,
            node: Some(node.node_entity().clone()),
            kind: EndpointKind::Subscription,
            topic: ros2_name.to_string(),
            type_info: Some(TypeInfo::new(ros2_type, lv_type_hash)),
            qos: qos_profile,
        };

        let topic_ke = node
            .keyexpr_format()
            .topic_key_expr(&entity)
            .map_err(|e| anyhow!("topic_key_expr failed: {e}"))?;
        let lv_ke = node
            .keyexpr_format()
            .liveliness_key_expr(&entity_lv, &node.session().zid())
            .map_err(|e| anyhow!("liveliness_key_expr failed: {e}"))?;

        // Convert "…/EMPTY_TOPIC_TYPE/EMPTY_TOPIC_HASH" → "…/**" for rmw-zenoh keys so
        // the subscriber matches any publisher type/hash on this topic.  For ros2dds keys
        // (which have no type/hash components) the suffix is absent and the key is used as-is.
        let ke_str = topic_ke.as_str();
        let ke_str = ke_str
            .strip_suffix("/EMPTY_TOPIC_TYPE/EMPTY_TOPIC_HASH")
            .map(|prefix| format!("{prefix}/**"))
            .unwrap_or_else(|| ke_str.to_string());
        let ke: zenoh::key_expr::OwnedKeyExpr = ke_str
            .try_into()
            .map_err(|e| anyhow!("invalid key expr: {e}"))?;

        let dds_topic = ros2_name_to_dds_pub_topic(ros2_name);
        let dds_type = ros2_type_to_dds_type(ros2_type);
        let dds_writer_qos = adapt_reader_qos_for_writer(&qos);

        let writer =
            Arc::new(participant.create_writer(&dds_topic, &dds_type, keyless, dds_writer_qos)?);

        let is_transient_local = qos
            .durability
            .as_ref()
            .is_some_and(|d| d.kind == DurabilityKind::TransientLocal);

        let writer_cb = Arc::clone(&writer);
        let ke_display = ke.as_str().to_string();
        let dds_topic_log = dds_topic.clone();

        let subscriber = if is_transient_local {
            tracing::debug!("ZDdsSubBridge: TRANSIENT_LOCAL {ros2_name}, using AdvancedSubscriber");
            let adv = node
                .session()
                .declare_subscriber(ke)
                .history(HistoryConfig::default().detect_late_publishers())
                .callback(move |sample| {
                    let bytes: Vec<u8> = sample.payload().to_bytes().into_owned();
                    if let Err(e) = writer_cb.write_cdr(bytes) {
                        tracing::warn!("ZDdsSubBridge: DDS write failed on {dds_topic_log}: {e}");
                    }
                })
                .await
                .map_err(|e| anyhow!("declare_subscriber(advanced) failed: {e}"))?;
            ZenohSubHandle::Advanced(adv)
        } else {
            let sub = node
                .session()
                .declare_subscriber(ke)
                .callback(move |sample| {
                    let bytes: Vec<u8> = sample.payload().to_bytes().into_owned();
                    if let Err(e) = writer_cb.write_cdr(bytes) {
                        tracing::warn!("ZDdsSubBridge: DDS write failed on {dds_topic_log}: {e}");
                    }
                })
                .await
                .map_err(|e| anyhow!("declare_subscriber failed: {e}"))?;
            ZenohSubHandle::Plain(sub)
        };

        let lv_token = node
            .session()
            .liveliness()
            .declare_token((*lv_ke).clone())
            .await
            .map_err(|e| anyhow!("liveliness token failed: {e}"))?;

        tracing::info!("ZDdsSubBridge: Zenoh {} → DDS {}", ke_display, dds_topic);

        Ok(Self {
            _writer: writer,
            _subscriber: subscriber,
            _lv_token: lv_token,
        })
    }

    /// Return the DDS writer GID created for this bridge route.
    pub fn writer_guid(&self) -> Option<Gid> {
        self._writer.guid().ok().map(Gid::from)
    }
}

// ─── compute_cache_size ───────────────────────────────────────────────────────

/// Compute the AdvancedPublisher cache size from DDS QoS.
///
/// Mirrors the zenoh-plugin-ros2dds formula:
/// - KEEP_ALL → `usize::MAX`
/// - keyless topics → `depth × multiplier`
/// - KEEP_LAST, unlimited instances → `usize::MAX`
/// - KEEP_LAST, N instances → `depth × N × multiplier`
fn compute_cache_size(qos: &BridgeQos, keyless: bool, multiplier: usize) -> usize {
    let depth = match &qos.history {
        Some(h) => match h.kind {
            HistoryKind::KeepAll => return usize::MAX,
            HistoryKind::KeepLast => h.depth as usize,
        },
        None => 1,
    };

    if keyless {
        return depth.saturating_mul(multiplier);
    }

    match qos
        .durability_service
        .as_ref()
        .and_then(|ds| ds.max_instances)
    {
        None => usize::MAX,
        Some(n) => depth.saturating_mul(n).saturating_mul(multiplier),
    }
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::participant::{
        BridgeQos, Durability, DurabilityKind, DurabilityService, History, HistoryKind,
    };

    use super::compute_cache_size;

    fn qos_transient_local(depth: i32) -> BridgeQos {
        BridgeQos {
            durability: Some(Durability {
                kind: DurabilityKind::TransientLocal,
            }),
            history: Some(History {
                kind: HistoryKind::KeepLast,
                depth,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_cache_size_keep_all() {
        let qos = BridgeQos {
            history: Some(History {
                kind: HistoryKind::KeepAll,
                depth: 0,
            }),
            ..Default::default()
        };
        assert_eq!(compute_cache_size(&qos, true, 10), usize::MAX);
    }

    #[test]
    fn test_cache_size_keyless_keep_last() {
        let qos = qos_transient_local(5);
        assert_eq!(compute_cache_size(&qos, true, 10), 50);
    }

    #[test]
    fn test_cache_size_keyed_unlimited_instances() {
        let qos = qos_transient_local(5);
        assert_eq!(compute_cache_size(&qos, false, 10), usize::MAX);
    }

    #[test]
    fn test_cache_size_keyed_limited_instances() {
        let qos = BridgeQos {
            history: Some(History {
                kind: HistoryKind::KeepLast,
                depth: 5,
            }),
            durability_service: Some(DurabilityService {
                service_cleanup_delay: None,
                history_kind: HistoryKind::KeepLast,
                history_depth: 5,
                max_samples: None,
                max_instances: Some(3),
                max_samples_per_instance: None,
            }),
            ..Default::default()
        };
        assert_eq!(compute_cache_size(&qos, false, 10), 150);
    }

    #[test]
    fn test_cache_size_no_history_keyless() {
        let qos = BridgeQos::default();
        assert_eq!(compute_cache_size(&qos, true, 10), 10);
    }
}
