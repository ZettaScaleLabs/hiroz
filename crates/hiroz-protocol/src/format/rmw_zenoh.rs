//! rmw_zenoh compatible backend.
//!
//! This backend generates key expressions compatible with rmw_zenoh.
//!
//! Key expression formats:
//! - Topic: `<domain_id>/<topic>/<type>/<hash>`
//! - Liveliness: `@ros2_lv/<domain_id>/<zid>/<nid>/<eid>/<kind>/<enclave>/<ns>/<name>[/<topic>/<type>/<hash>/<qos>]`

use zenoh::{key_expr::KeyExpr, session::ZenohId, Result};

use crate::{
    entity::{
        EndpointEntity, Entity, EntityConversionError, EntityKind, LivelinessKE, NodeEntity,
        TopicKE, TypeHash, TypeInfo,
    },
    qos::QosProfile,
};

use super::KeyExprFormatter;

/// Placeholder for empty namespace/enclave.
pub const EMPTY_PLACEHOLDER: &str = "%";
/// Placeholder for empty topic type.
pub const EMPTY_TOPIC_TYPE: &str = "EMPTY_TOPIC_TYPE";
/// Placeholder for empty topic hash.
pub const EMPTY_TOPIC_HASH: &str = "EMPTY_TOPIC_HASH";

/// rmw_zenoh compatible backend.
pub struct RmwZenohFormatter;

impl KeyExprFormatter for RmwZenohFormatter {
    const ESCAPE_CHAR: char = '%';
    const ADMIN_SPACE: &'static str = "@ros2_lv";

    fn topic_key_expr(entity: &EndpointEntity) -> Result<TopicKE> {
        let EndpointEntity {
            node: Some(node),
            topic,
            type_info,
            ..
        } = entity
        else {
            return Err(zenoh::Error::from(
                "rmw-zenoh endpoint keys require node identity",
            ));
        };
        let domain_id = node.domain_id;
        let topic = {
            let s = topic.as_str();
            let s = s.strip_prefix('/').unwrap_or(s);
            let s = s.strip_suffix('/').unwrap_or(s);

            // CRITICAL: rmw_zenoh_cpp uses strip_slashes() for ALL topic key expressions.
            // This means we preserve internal slashes for ALL entity types:
            // - Services/Clients: /talker/service → talker/service
            // - Publishers/Subscriptions: /ns/topic → ns/topic
            // - Actions: /fibonacci/_action/send_goal → fibonacci/_action/send_goal
            //
            // Mangling (replacing / with %) is ONLY used in liveliness tokens, NOT topic keys!
            // Reference: rmw_zenoh_cpp TopicInfo::TopicInfo() uses strip_slashes(name_) for all topics
            s.to_string()
        };

        let type_info =
            type_info
                .as_ref()
                .map_or(format!("{EMPTY_TOPIC_TYPE}/{EMPTY_TOPIC_HASH}"), |x| {
                    let type_name = Self::demangle_name(&x.name);
                    let type_hash = Self::demangle_name(&x.hash.to_string());
                    format!("{type_name}/{type_hash}")
                });

        Ok(TopicKE::new(
            format!("{domain_id}/{topic}/{type_info}").try_into()?,
        ))
    }

    fn liveliness_key_expr(entity: &EndpointEntity, _zid: &ZenohId) -> Result<LivelinessKE> {
        let EndpointEntity {
            id,
            node:
                Some(NodeEntity {
                    domain_id,
                    z_id,
                    id: node_id,
                    name: node_name,
                    namespace: node_namespace,
                    enclave: _,
                }),
            kind,
            topic: topic_name,
            type_info,
            qos,
        } = entity
        else {
            return Err(zenoh::Error::from(
                "rmw-zenoh liveliness requires node identity",
            ));
        };

        let node_namespace = if node_namespace.is_empty() {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            Self::mangle_name(node_namespace)
        };
        let node_name = Self::mangle_name(node_name);

        // Mangle all slashes in topic name for liveliness tokens
        let topic_name = {
            let s = topic_name.strip_suffix('/').unwrap_or(topic_name);
            Self::mangle_name(s)
        };

        let type_info_str = type_info
            .as_ref()
            .map_or(format!("{EMPTY_TOPIC_TYPE}/{EMPTY_TOPIC_HASH}"), |x| {
                format!("{}/{}", Self::mangle_name(&x.name), x.hash.to_rihs_string())
            });

        let qos_str = qos.encode();

        let ke = format!(
            "{}/{domain_id}/{z_id}/{node_id}/{id}/{kind}/{EMPTY_PLACEHOLDER}/{node_namespace}/{node_name}/{topic_name}/{type_info_str}/{qos_str}",
            Self::ADMIN_SPACE
        );

        Ok(LivelinessKE::new(ke.try_into()?))
    }

    fn node_liveliness_key_expr(entity: &NodeEntity) -> Result<LivelinessKE> {
        let NodeEntity {
            domain_id,
            z_id,
            id,
            name,
            namespace,
            enclave: _,
        } = entity;

        let namespace = if namespace.is_empty() {
            EMPTY_PLACEHOLDER
        } else {
            &Self::mangle_name(namespace)
        };
        let name = Self::mangle_name(name);

        Ok(LivelinessKE::new(
            format!(
                "{}/{domain_id}/{z_id}/{id}/{id}/NN/{EMPTY_PLACEHOLDER}/{namespace}/{name}",
                Self::ADMIN_SPACE
            )
            .try_into()?,
        ))
    }

    fn parse_liveliness(ke: &KeyExpr) -> Result<Entity> {
        use EntityConversionError::*;

        let mut iter = ke.split('/');

        // Check admin space prefix
        let admin = iter.next().ok_or(MissingAdminSpace)?;
        if admin != Self::ADMIN_SPACE {
            return Err(zenoh::Error::from(MissingAdminSpace));
        }

        let domain_id = iter
            .next()
            .ok_or(MissingDomainId)?
            .parse()
            .map_err(|_| ParsingError)?;
        let z_id = iter
            .next()
            .ok_or(MissingZId)?
            .parse()
            .map_err(|_| ParsingError)?;
        let node_id = iter
            .next()
            .ok_or(MissingNodeId)?
            .parse()
            .map_err(|_| ParsingError)?;
        let entity_id = iter
            .next()
            .ok_or(MissingEntityId)?
            .parse()
            .map_err(|_| ParsingError)?;
        let entity_kind: EntityKind = iter
            .next()
            .ok_or(MissingEntityKind)?
            .parse()
            .map_err(|_| ParsingError)?;

        // Enclave (not supported yet)
        let enclave = match iter.next().ok_or(MissingEnclave)? {
            EMPTY_PLACEHOLDER => String::new(),
            x => Self::demangle_name(x),
        };

        let namespace = match iter.next().ok_or(MissingNamespace)? {
            EMPTY_PLACEHOLDER => String::new(),
            x => Self::demangle_name(x),
        };
        let node_name = Self::demangle_name(iter.next().ok_or(MissingNodeName)?);

        let node = NodeEntity {
            id: node_id,
            domain_id,
            z_id,
            name: node_name,
            namespace,
            enclave,
        };

        Ok(match entity_kind {
            EntityKind::Node => Entity::Node(node),
            EntityKind::Endpoint(kind) => {
                let topic_name = Self::demangle_name(iter.next().ok_or(MissingTopicName)?);
                let topic_type = iter.next().ok_or(MissingTopicType)?;
                let topic_hash = iter.next().ok_or(MissingTopicHash)?;

                let type_info = match (topic_type, topic_hash) {
                    (EMPTY_TOPIC_TYPE, EMPTY_TOPIC_HASH) => None,
                    (EMPTY_TOPIC_TYPE, _) | (_, EMPTY_TOPIC_HASH) => None,
                    (topic_type, topic_hash) => {
                        let type_hash = TypeHash::from_rihs_string(topic_hash)
                            .unwrap_or(TypeHash::new(0, [0u8; 32]));
                        Some(TypeInfo {
                            name: Self::demangle_name(topic_type),
                            hash: type_hash,
                        })
                    }
                };

                let qos = QosProfile::decode(iter.next().ok_or(MissingTopicQoS)?)
                    .map_err(QosDecodeError)?;

                Entity::Endpoint(EndpointEntity {
                    id: entity_id,
                    node: Some(node),
                    kind,
                    topic: topic_name,
                    type_info,
                    qos,
                })
            }
        })
    }

    fn encode_qos(qos: &QosProfile, _keyless: bool) -> String {
        qos.encode()
    }

    fn decode_qos(s: &str) -> Result<(bool, QosProfile)> {
        let qos = QosProfile::decode(s)
            .map_err(|e| zenoh::Error::from(format!("QoS decode error: {:?}", e)))?;
        Ok((false, qos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EndpointEntity, EndpointKind, NodeEntity, TypeInfo};
    use crate::qos::{QosDurability, QosHistory, QosProfile, QosReliability};

    #[test]
    fn test_mangle_demangle() {
        assert_eq!(RmwZenohFormatter::mangle_name("/chatter"), "%chatter");
        assert_eq!(
            RmwZenohFormatter::mangle_name("std_msgs/msg/String"),
            "std_msgs%msg%String"
        );
        assert_eq!(
            RmwZenohFormatter::demangle_name("std_msgs%msg%String"),
            "std_msgs/msg/String"
        );
    }

    #[test]
    fn test_qos_encode_decode() {
        let qos = QosProfile::default();
        let encoded = RmwZenohFormatter::encode_qos(&qos, false);

        let (keyless, decoded) = RmwZenohFormatter::decode_qos(&encoded).unwrap();
        assert!(!keyless);
        assert_eq!(decoded.reliability, qos.reliability);
        assert_eq!(decoded.durability, qos.durability);
    }

    #[test]
    fn test_qos_reliable_transient() {
        let qos = QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::TransientLocal,
            history: QosHistory::from_depth(10),
        };
        let encoded = RmwZenohFormatter::encode_qos(&qos, false);

        let (keyless, decoded) = RmwZenohFormatter::decode_qos(&encoded).unwrap();
        assert!(!keyless);
        assert_eq!(decoded.reliability, QosReliability::Reliable);
        assert_eq!(decoded.durability, QosDurability::TransientLocal);
    }

    /// Test topic key expression format matches rmw_zenoh.
    ///
    /// rmw_zenoh format: `<domain_id>/<topic>/<type>/<hash>`
    #[test]
    fn test_topic_key_expr_format() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // rmw_zenoh format: <domain_id>/<topic>/<type>/<hash>
        assert!(
            ke_str.starts_with("0/"),
            "Should start with domain ID '0/', got: {}",
            ke_str
        );
        assert!(
            ke_str.contains("/chatter/"),
            "Should contain '/chatter/', got: {}",
            ke_str
        );
        // Note: rmw_zenoh does NOT escape slashes in topic key expression
        assert!(
            ke_str.contains("std_msgs/msg/String"),
            "Should contain type name, got: {}",
            ke_str
        );
    }

    /// Test liveliness key expression format matches rmw_zenoh.
    ///
    /// Format: `@ros2_lv/<domain>/<zid>/<nid>/<eid>/<kind>/<enclave>/<namespace>/<node_name>/<topic>/<type>/<hash>/<qos>`
    #[test]
    fn test_liveliness_key_expr_format() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should start with @ros2_lv
        assert!(
            ke_str.starts_with("@ros2_lv/"),
            "Should start with '@ros2_lv/', got: {}",
            ke_str
        );

        // Should contain domain ID
        assert!(
            ke_str.contains("/0/"),
            "Should contain domain '/0/', got: {}",
            ke_str
        );

        // Should contain MP for Publisher
        assert!(
            ke_str.contains("/MP/"),
            "Should contain '/MP/' for Publisher, got: {}",
            ke_str
        );

        // Should contain node name
        assert!(
            ke_str.contains("/test_node/"),
            "Should contain '/test_node/', got: {}",
            ke_str
        );

        // Should contain escaped topic name
        assert!(
            ke_str.contains("/chatter/"),
            "Should contain '/chatter/', got: {}",
            ke_str
        );

        // Should contain escaped type name
        assert!(
            ke_str.contains("std_msgs%msg%String"),
            "Should contain 'std_msgs%msg%String', got: {}",
            ke_str
        );
    }

    /// Test subscriber liveliness key expression
    #[test]
    fn test_subscriber_liveliness_key_expr() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Subscription,
            topic: "chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should contain MS for Subscription
        assert!(
            ke_str.contains("/MS/"),
            "Should contain '/MS/' for Subscription, got: {}",
            ke_str
        );
    }

    /// Test service server liveliness key expression
    #[test]
    fn test_service_liveliness_key_expr() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: "add_two_ints".to_string(),
            type_info: Some(TypeInfo::new(
                "example_interfaces/srv/AddTwoInts",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should contain SS for Service
        assert!(
            ke_str.contains("/SS/"),
            "Should contain '/SS/' for Service, got: {}",
            ke_str
        );
    }

    /// Test client liveliness key expression
    #[test]
    fn test_client_liveliness_key_expr() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Client,
            topic: "add_two_ints".to_string(),
            type_info: Some(TypeInfo::new(
                "example_interfaces/srv/AddTwoInts",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should contain SC for Client
        assert!(
            ke_str.contains("/SC/"),
            "Should contain '/SC/' for Client, got: {}",
            ke_str
        );
    }

    /// Test node liveliness key expression
    #[test]
    fn test_node_liveliness_key_expr() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/my_namespace".to_string(),
            String::new(),
        );

        let liveliness_ke = RmwZenohFormatter::node_liveliness_key_expr(&node).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should start with @ros2_lv
        assert!(
            ke_str.starts_with("@ros2_lv/"),
            "Should start with '@ros2_lv/', got: {}",
            ke_str
        );

        // Should contain NN for Node
        assert!(
            ke_str.contains("/NN/"),
            "Should contain '/NN/' for Node, got: {}",
            ke_str
        );

        // Should contain node name
        assert!(
            ke_str.contains("/test_node"),
            "Should contain '/test_node', got: {}",
            ke_str
        );
    }

    // ==================== Topic Key Expression Tests ====================
    // These tests verify the CRITICAL behavior: ALL topic key expressions use
    // strip_slashes() (preserve internal slashes), NO mangling for ANY entity type.
    // Mangling is ONLY used in liveliness tokens, NOT topic key expressions!
    // Reference: rmw_zenoh_cpp TopicInfo constructor uses strip_slashes() for all topics.

    /// Test service topic key expression preserves internal slashes (strip_slashes behavior).
    ///
    /// CRITICAL: Services must NOT mangle internal slashes.
    /// Format: `<domain>/<topic_with_slashes>/<type>/<hash>`
    #[test]
    fn test_service_topic_key_expr_preserves_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "talker".to_string(),
            "/".to_string(),
            String::new(),
        );

        // Service with slashes in name (common pattern: /node_name/service_name)
        let entity = EndpointEntity {
            id: 10,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: "/talker/get_type_description".to_string(),
            type_info: Some(TypeInfo::new(
                "type_description_interfaces::srv::dds_::GetTypeDescription_",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // MUST preserve internal slashes: 0/talker/get_type_description/...
        // NOT: 0/talker%get_type_description/...
        assert!(
            ke_str.starts_with("0/talker/get_type_description/"),
            "Service topic key expr should preserve internal slashes (strip_slashes behavior), got: {}",
            ke_str
        );
        assert!(
            !ke_str.contains("%"),
            "Service topic key expr should NOT contain mangled slashes, got: {}",
            ke_str
        );
    }

    /// Test client topic key expression preserves internal slashes (same as service).
    #[test]
    fn test_client_topic_key_expr_preserves_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_client".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 11,
            node: Some(node),
            kind: EndpointKind::Client,
            topic: "/my_service/sub_service/action".to_string(),
            type_info: Some(TypeInfo::new(
                "example_interfaces/srv/AddTwoInts",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        assert!(
            ke_str.starts_with("0/my_service/sub_service/action/"),
            "Client topic key expr should preserve internal slashes, got: {}",
            ke_str
        );
    }

    /// Test publisher topic key expression preserves internal slashes.
    ///
    /// CRITICAL: All topic key expressions use strip_slashes() behavior,
    /// which preserves internal slashes (no mangling).
    #[test]
    fn test_publisher_topic_key_expr_preserves_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_pub".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "/ns/topic".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should preserve slashes: 0/ns/topic/...
        // NOT mangle: 0/ns%topic/...
        assert!(
            ke_str.contains("ns/topic"),
            "Publisher topic key expr should preserve internal slashes (strip_slashes behavior), got: {}",
            ke_str
        );
        assert!(
            !ke_str.contains("%"),
            "Publisher topic key expr should NOT mangle slashes, got: {}",
            ke_str
        );
    }

    /// Test subscription topic key expression preserves internal slashes.
    ///
    /// CRITICAL: All topic key expressions use strip_slashes() behavior.
    #[test]
    fn test_subscription_topic_key_expr_preserves_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_sub".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 2,
            node: Some(node),
            kind: EndpointKind::Subscription,
            topic: "/robot/sensor/data".to_string(),
            type_info: Some(TypeInfo::new("sensor_msgs/msg/Image", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should preserve slashes: 0/robot/sensor/data/...
        // NOT mangle: 0/robot%sensor%data/...
        assert!(
            ke_str.contains("robot/sensor/data"),
            "Subscription topic key expr should preserve internal slashes (strip_slashes behavior), got: {}",
            ke_str
        );
        assert!(
            !ke_str.contains("%"),
            "Subscription topic key expr should NOT mangle slashes, got: {}",
            ke_str
        );
    }

    /// Test action topic key expression preserves internal slashes.
    ///
    /// Actions use strip_slashes() like all other topics.
    #[test]
    fn test_action_topic_key_expr_preserves_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "action_server".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 3,
            node: Some(node),
            kind: EndpointKind::Publisher, // Actions use pub/sub for feedback/status
            topic: "/fibonacci/_action/send_goal".to_string(),
            type_info: Some(TypeInfo::new(
                "action_tutorials_interfaces::action::dds_::Fibonacci_SendGoal_",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should preserve internal slashes: 0/fibonacci/_action/send_goal/...
        assert!(
            ke_str.contains("fibonacci/_action/send_goal"),
            "Action topic key expr should preserve internal slashes (strip_slashes behavior), got: {}",
            ke_str
        );
        assert!(
            !ke_str.contains("%"),
            "Action topic key expr should NOT mangle slashes, got: {}",
            ke_str
        );
    }

    /// Test strip_slashes behavior: leading and trailing slash removal.
    #[test]
    fn test_topic_key_expr_strips_leading_trailing_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        // Service with leading and trailing slashes
        let entity = EndpointEntity {
            id: 4,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: "/my_service/".to_string(),
            type_info: Some(TypeInfo::new(
                "example_interfaces/srv/Trigger",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should become: 0/my_service/...
        assert!(
            ke_str.starts_with("0/my_service/"),
            "Should strip leading and trailing slashes, got: {}",
            ke_str
        );
        assert!(
            !ke_str.contains("//"),
            "Should not have double slashes, got: {}",
            ke_str
        );
    }

    /// Test empty type info handling.
    #[test]
    fn test_topic_key_expr_empty_type_info() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 5,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            type_info: None, // No type info
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should use placeholders: 0/chatter/EMPTY_TOPIC_TYPE/EMPTY_TOPIC_HASH
        assert!(
            ke_str.contains(EMPTY_TOPIC_TYPE),
            "Should use EMPTY_TOPIC_TYPE placeholder, got: {}",
            ke_str
        );
        assert!(
            ke_str.contains(EMPTY_TOPIC_HASH),
            "Should use EMPTY_TOPIC_HASH placeholder, got: {}",
            ke_str
        );
    }

    /// Test type name demangling in topic key expression.
    #[test]
    fn test_topic_key_expr_type_name_demangling() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 6,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            // Type name with mangled slashes (as stored internally)
            type_info: Some(TypeInfo::new("std_msgs%msg%String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Type name should be demangled in key expression
        assert!(
            ke_str.contains("std_msgs/msg/String"),
            "Type name should be demangled in topic key expr, got: {}",
            ke_str
        );
    }

    // ==================== Liveliness Key Expression Tests ====================

    /// Test service liveliness key expression mangles topic name.
    ///
    /// CRITICAL: In liveliness tokens, ALL topic names are mangled (even services).
    /// This is different from topic key expressions!
    #[test]
    fn test_service_liveliness_mangles_topic_name() {
        let zid: zenoh::session::ZenohId = "9aed1ea85b72095f6dbc9ee90dabd56".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "talker".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 10,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: "/talker/get_type_description".to_string(),
            type_info: Some(TypeInfo::new(
                "type_description_interfaces::srv::dds_::GetTypeDescription_",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Liveliness token MUST mangle topic name: %talker%get_type_description
        assert!(
            ke_str.contains("%talker%get_type_description"),
            "Service liveliness should mangle topic name, got: {}",
            ke_str
        );
    }

    /// Test publisher liveliness with multiple namespace segments.
    #[test]
    fn test_publisher_liveliness_multi_segment_namespace() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "sensor_node".to_string(),
            "/robot/sensors".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 7,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "/data/temperature".to_string(),
            type_info: Some(TypeInfo::new(
                "sensor_msgs/msg/Temperature",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Namespace should be mangled: %robot%sensors
        assert!(
            ke_str.contains("%robot%sensors"),
            "Namespace should be mangled in liveliness, got: {}",
            ke_str
        );
        // Topic should be mangled: %data%temperature
        assert!(
            ke_str.contains("%data%temperature"),
            "Topic should be mangled in liveliness, got: {}",
            ke_str
        );
        // Should contain MP for Publisher
        assert!(
            ke_str.contains("/MP/"),
            "Should contain '/MP/', got: {}",
            ke_str
        );
    }

    /// Test empty namespace handling in liveliness.
    #[test]
    fn test_liveliness_empty_namespace() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 8,
            node: Some(node),
            kind: EndpointKind::Subscription,
            topic: "chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Empty namespace should use placeholder
        // Format: @ros2_lv/0/{zid}/0/8/MS/%/%/test_node/...
        let parts: Vec<&str> = ke_str.split('/').collect();
        assert_eq!(
            parts[6], EMPTY_PLACEHOLDER,
            "Enclave should be empty placeholder"
        );
        assert_eq!(
            parts[7], EMPTY_PLACEHOLDER,
            "Empty namespace should use placeholder, got: {}",
            ke_str
        );
    }

    /// Test root namespace (/) handling in liveliness.
    #[test]
    fn test_liveliness_root_namespace() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 9,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Root namespace should be treated as empty
        let parts: Vec<&str> = ke_str.split('/').collect();
        assert_eq!(
            parts[7], EMPTY_PLACEHOLDER,
            "Root namespace should use placeholder, got: {}",
            ke_str
        );
    }

    /// Test type info mangling in liveliness token.
    #[test]
    fn test_liveliness_type_info_mangling() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 10,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "image".to_string(),
            type_info: Some(TypeInfo::new(
                "sensor_msgs/msg/Image",
                TypeHash::new(1, [0x12; 32]),
            )),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Type name should be mangled in liveliness: sensor_msgs%msg%Image
        assert!(
            ke_str.contains("sensor_msgs%msg%Image"),
            "Type name should be mangled in liveliness, got: {}",
            ke_str
        );
        // Should contain RIHS01 hash
        assert!(
            ke_str.contains("RIHS01_"),
            "Should contain RIHS01 hash, got: {}",
            ke_str
        );
    }

    /// Test liveliness QoS encoding.
    #[test]
    fn test_liveliness_qos_encoding() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let qos = QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::TransientLocal,
            history: QosHistory::from_depth(10),
        };

        let entity = EndpointEntity {
            id: 11,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos,
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // QoS should be encoded at the end
        // Format: :reliability:,depth:durability:liveliness:deadline,lifespan
        let parts: Vec<&str> = ke_str.split('/').collect();
        let qos_part = parts.last().unwrap();
        assert!(
            qos_part.contains(":"),
            "QoS should be encoded with colons, got: {}",
            qos_part
        );
        // Should contain depth (10)
        assert!(
            qos_part.contains("10"),
            "QoS should contain history depth, got: {}",
            qos_part
        );
    }

    // ==================== Round-trip Tests ====================

    /// Test parse_liveliness round-trip for publisher.
    #[test]
    fn test_parse_liveliness_publisher_roundtrip() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test_node".to_string(),
            "/my_ns".to_string(),
            String::new(),
        );

        let original = EndpointEntity {
            id: 12,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "/topic/name".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&original, &zid).unwrap();
        let parsed = RmwZenohFormatter::parse_liveliness(&liveliness_ke).unwrap();

        if let Entity::Endpoint(parsed_entity) = parsed {
            assert_eq!(parsed_entity.id, original.id);
            assert_eq!(parsed_entity.kind, original.kind);
            assert_eq!(
                parsed_entity.node.as_ref().unwrap().name,
                original.node.as_ref().unwrap().name
            );
            assert_eq!(
                parsed_entity.node.as_ref().unwrap().namespace,
                original.node.as_ref().unwrap().namespace
            );
            // Topic name should be reconstructed (slashes demangled)
            assert_eq!(parsed_entity.topic, "/topic/name");
        } else {
            panic!("Expected Endpoint entity");
        }
    }

    /// Test parse_liveliness round-trip for service.
    #[test]
    fn test_parse_liveliness_service_roundtrip() {
        let zid: zenoh::session::ZenohId = "9aed1ea85b72095f6dbc9ee90dabd56".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "server".to_string(),
            "/".to_string(),
            String::new(),
        );

        let original = EndpointEntity {
            id: 13,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: "/my/service".to_string(),
            type_info: Some(TypeInfo::new(
                "example_interfaces::srv::dds_::AddTwoInts_",
                TypeHash::new(1, [0xab; 32]),
            )),
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&original, &zid).unwrap();
        let parsed = RmwZenohFormatter::parse_liveliness(&liveliness_ke).unwrap();

        if let Entity::Endpoint(parsed_entity) = parsed {
            assert_eq!(parsed_entity.id, original.id);
            assert_eq!(parsed_entity.kind, EndpointKind::Service);
            assert_eq!(parsed_entity.topic, "/my/service");
            assert_eq!(
                parsed_entity.type_info.as_ref().unwrap().name,
                "example_interfaces::srv::dds_::AddTwoInts_"
            );
        } else {
            panic!("Expected Endpoint entity");
        }
    }

    /// Test parse_liveliness with empty type info.
    #[test]
    fn test_parse_liveliness_empty_type_info() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let original = EndpointEntity {
            id: 14,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "test".to_string(),
            type_info: None,
            qos: QosProfile::default(),
        };

        let liveliness_ke = RmwZenohFormatter::liveliness_key_expr(&original, &zid).unwrap();
        let parsed = RmwZenohFormatter::parse_liveliness(&liveliness_ke).unwrap();

        if let Entity::Endpoint(parsed_entity) = parsed {
            assert!(
                parsed_entity.type_info.is_none(),
                "Type info should be None for empty placeholders"
            );
        } else {
            panic!("Expected Endpoint entity");
        }
    }

    /// Test parse_liveliness for node entity.
    #[test]
    fn test_parse_liveliness_node() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            15,
            "my_node".to_string(),
            "/robot/sensors".to_string(),
            String::new(),
        );

        let liveliness_ke = RmwZenohFormatter::node_liveliness_key_expr(&node).unwrap();
        let parsed = RmwZenohFormatter::parse_liveliness(&liveliness_ke).unwrap();

        if let Entity::Node(parsed_node) = parsed {
            assert_eq!(parsed_node.id, node.id);
            assert_eq!(parsed_node.name, node.name);
            assert_eq!(parsed_node.namespace, node.namespace);
            assert_eq!(parsed_node.domain_id, node.domain_id);
        } else {
            panic!("Expected Node entity");
        }
    }

    // ==================== Edge Cases and Error Handling ====================

    /// Test topic with no slashes.
    #[test]
    fn test_topic_key_expr_no_slashes() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 16,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "simple_topic".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should just be: 0/simple_topic/...
        assert!(
            ke_str.starts_with("0/simple_topic/"),
            "Simple topic should work, got: {}",
            ke_str
        );
    }

    /// Test that consecutive slashes in topic names are rejected by Zenoh.
    ///
    /// Zenoh KeyExpr does not allow empty chunks (consecutive slashes).
    /// This test verifies that such invalid inputs are caught.
    #[test]
    fn test_service_topic_consecutive_slashes_rejected() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 17,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: "/a//b".to_string(), // Consecutive slashes
            type_info: Some(TypeInfo::new("std_srvs/srv/Trigger", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        // Should return an error because Zenoh KeyExpr rejects consecutive slashes
        let result = RmwZenohFormatter::topic_key_expr(&entity);
        assert!(
            result.is_err(),
            "Consecutive slashes should be rejected by Zenoh KeyExpr"
        );
    }

    /// Test DDS type name format (with ::dds_:: namespace).
    #[test]
    fn test_topic_key_expr_dds_type_name() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 18,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "chatter".to_string(),
            // DDS type name with ::dds_:: namespace
            type_info: Some(TypeInfo::new(
                "std_msgs::msg::dds_::String_",
                TypeHash::zero(),
            )),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // DDS type name should be preserved exactly
        assert!(
            ke_str.contains("std_msgs::msg::dds_::String_"),
            "DDS type name should be preserved, got: {}",
            ke_str
        );
    }

    /// Test very long topic name.
    #[test]
    fn test_topic_key_expr_long_name() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            0,
            "test".to_string(),
            "/".to_string(),
            String::new(),
        );

        let long_topic = "/very/long/topic/name/with/many/segments/for/testing/purposes";
        let entity = EndpointEntity {
            id: 19,
            node: Some(node),
            kind: EndpointKind::Service,
            topic: long_topic.to_string(),
            type_info: Some(TypeInfo::new("std_srvs/srv/Trigger", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let topic_ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        let ke_str = topic_ke.as_str();

        // Should handle long names correctly (strip leading slash, keep internal ones)
        assert!(
            ke_str.contains("very/long/topic/name/with/many/segments/for/testing/purposes"),
            "Should handle long topic names, got: {}",
            ke_str
        );
    }

    /// Test TypeHash RIHS string format.
    #[test]
    fn test_type_hash_rihs_format() {
        let hash = TypeHash::new(1, [0xab; 32]);
        let rihs_str = hash.to_rihs_string();

        // Should be RIHS01_ followed by 64 hex characters
        assert!(
            rihs_str.starts_with("RIHS01_"),
            "Should start with RIHS01_, got: {}",
            rihs_str
        );
        assert_eq!(
            rihs_str.len(),
            7 + 64,
            "RIHS string should be 71 chars (RIHS01_ + 64 hex)"
        );

        // Should be able to parse back
        let parsed = TypeHash::from_rihs_string(&rihs_str).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.value, [0xab; 32]);
    }

    /// Test zero TypeHash.
    #[test]
    fn test_type_hash_zero() {
        let hash = TypeHash::zero();
        let rihs_str = hash.to_rihs_string();

        assert_eq!(
            rihs_str,
            "RIHS01_0000000000000000000000000000000000000000000000000000000000000000"
        );
    }
}

// ---------------------------------------------------------------------------
// Kani formal proof harnesses
// ---------------------------------------------------------------------------

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use crate::{
        entity::{EndpointEntity, EntityKind, NodeEntity, TypeHash, TypeInfo},
        qos::{QosDurability, QosHistory, QosProfile, QosReliability},
    };
    use zenoh::session::ZenohId;

    /// For any domain_id in 0..=255, formatting then parsing a publisher
    /// liveliness key expression recovers the original domain_id and entity kind.
    #[kani::proof]
    #[kani::unwind(8)]
    fn liveliness_roundtrip_domain_id() {
        // Bound the domain_id to a u8 range so Kani can enumerate all values
        let raw_domain_id: u8 = kani::any();
        let domain_id = raw_domain_id as usize;

        let node = NodeEntity {
            domain_id,
            z_id: ZenohId::default(),
            id: 1,
            name: "kani_node".to_string(),
            namespace: "/".to_string(),
            enclave: "/".to_string(),
        };
        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "/kani_topic".to_string(),
            type_info: Some(TypeInfo {
                name: "std_msgs/msg/String".to_string(),
                hash: TypeHash::zero(),
            }),
            qos: QosProfile {
                reliability: QosReliability::Reliable,
                durability: QosDurability::Volatile,
                history: QosHistory::KeepLast(10),
            },
        };

        let ke = RmwZenohFormatter::liveliness_key_expr(&entity, &ZenohId::default())
            .expect("liveliness_key_expr");
        let parsed = RmwZenohFormatter::parse_liveliness(&ke).expect("parse_liveliness");

        if let crate::entity::Entity::Endpoint(ep) = parsed {
            kani::assert(
                ep.node.as_ref().unwrap().domain_id == domain_id,
                "domain_id preserved",
            );
            kani::assert(ep.kind == EndpointKind::Publisher, "entity kind preserved");
        } else {
            kani::assert(false, "expected Endpoint entity");
        }
    }
}
