//! zenoh-plugin-ros2dds compatible backend.
//!
//! This backend generates key expressions compatible with zenoh-plugin-ros2dds.
//!
//! Key expression formats:
//! - Topic: `<topic_name>` (simple, no type/hash)
//! - Liveliness: `@/<zenoh_id>/@ros2_lv/<kind>/<escaped_ke>/<type>[/<qos>]`
//!
//! Reference: zenoh-plugin-ros2dds/src/liveliness_mgt.rs

use zenoh::{key_expr::KeyExpr, session::ZenohId, Result};

use crate::{
    entity::{
        EndpointEntity, EndpointKind, Entity, EntityConversionError, LivelinessKE, NodeEntity,
        TopicKE, TypeHash, TypeInfo,
    },
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
};

use super::KeyExprFormatter;

/// Escape character for slashes in ros2dds format (U+00A7 Section Sign).
pub const SLASH_REPLACEMENT_CHAR: char = '§';

/// zenoh-plugin-ros2dds compatible formatter.
pub struct Ros2DdsFormatter;

impl KeyExprFormatter for Ros2DdsFormatter {
    /// ros2dds uses '§' (U+00A7) to escape slashes
    const ESCAPE_CHAR: char = SLASH_REPLACEMENT_CHAR;

    /// Admin space prefix for ros2dds liveliness tokens
    const ADMIN_SPACE: &'static str = "@ros2_lv";

    fn topic_key_expr(entity: &EndpointEntity) -> Result<TopicKE> {
        // ros2dds format: just the topic name (no domain, type, or hash)
        let topic = {
            let s = &entity.topic;
            let s = s.strip_prefix('/').unwrap_or(s);
            let s = s.strip_suffix('/').unwrap_or(s);
            s.to_string()
        };

        Ok(TopicKE::new(topic.try_into()?))
    }

    fn liveliness_key_expr(entity: &EndpointEntity, zid: &ZenohId) -> Result<LivelinessKE> {
        // ros2dds format: @/<zenoh_id>/@ros2_lv/<kind>/<escaped_ke>/<type>[/<qos>]
        let kind = match entity.kind {
            EndpointKind::Publisher => "MP",
            EndpointKind::Subscription => "MS",
            EndpointKind::Service => "SS",
            EndpointKind::Client => "SC",
        };

        // Escape slashes in topic name
        let topic = {
            let s = &entity.topic;
            let s = s.strip_prefix('/').unwrap_or(s);
            let s = s.strip_suffix('/').unwrap_or(s);
            Self::mangle_name(s)
        };

        // Escape slashes in type name
        let type_name = entity
            .type_info
            .as_ref()
            .map(|ti| Self::mangle_name(&ti.name))
            .unwrap_or_else(|| "unknown".to_string());

        // QoS encoding for pub/sub only
        let qos_str = match entity.kind {
            EndpointKind::Publisher | EndpointKind::Subscription => {
                format!("/{}", Self::encode_qos(&entity.qos, false))
            }
            _ => String::new(),
        };

        let ke = format!(
            "@/{zid}/{}/{kind}/{topic}/{type_name}{qos_str}",
            Self::ADMIN_SPACE
        );

        Ok(LivelinessKE::new(ke.try_into()?))
    }

    fn node_liveliness_key_expr(_entity: &NodeEntity) -> Result<LivelinessKE> {
        // ros2dds does not expose node entities as liveliness tokens
        // Return an empty/placeholder token that won't match anything
        Err(zenoh::Error::from(
            "ros2dds backend does not support node liveliness tokens",
        ))
    }

    fn parse_liveliness(ke: &KeyExpr) -> Result<Entity> {
        // ros2dds format: @/<zenoh_id>/@ros2_lv/<kind>/<escaped_ke>/<type>[/<qos>]
        use EntityConversionError::*;

        let mut iter = ke.split('/');

        // First element should be '@'
        let first = iter.next().ok_or(MissingAdminSpace)?;
        if first != "@" {
            return Err(zenoh::Error::from(MissingAdminSpace));
        }

        // Zenoh ID: present in the key expression but not used in Ros2Dds format
        // (endpoints have no node identity, so the z_id cannot be stored)
        let _z_id = iter.next().ok_or(MissingZId)?;

        // @ros2_lv
        let admin = iter.next().ok_or(MissingAdminSpace)?;
        if admin != Self::ADMIN_SPACE {
            return Err(zenoh::Error::from(MissingAdminSpace));
        }

        // Entity kind
        let kind_str = iter.next().ok_or(MissingEntityKind)?;
        let kind = match kind_str {
            "MP" => EndpointKind::Publisher,
            "MS" => EndpointKind::Subscription,
            "SS" => EndpointKind::Service,
            "SC" => EndpointKind::Client,
            "AS" | "AC" => {
                // Action server/client - map to Service for now
                EndpointKind::Service
            }
            _ => return Err(zenoh::Error::from(ParsingError)),
        };

        // Topic key expression (escaped)
        let topic_escaped = iter.next().ok_or(MissingTopicName)?;
        let topic = match Self::demangle_name(topic_escaped) {
            topic if topic.is_empty() => "/".to_string(),
            topic if topic.starts_with('/') => topic,
            topic => format!("/{}", topic),
        };

        // Type name (escaped)
        let type_escaped = iter.next().ok_or(MissingTopicType)?;
        let type_name = Self::demangle_name(type_escaped);

        // Optional QoS
        let qos = if let Some(qos_str) = iter.next() {
            let (_, qos) = Self::decode_qos(qos_str)?;
            qos
        } else {
            QosProfile::default()
        };

        let type_info = if type_name.is_empty() || type_name == "unknown" {
            None
        } else {
            Some(TypeInfo {
                name: type_name,
                hash: TypeHash::zero(),
            })
        };

        Ok(Entity::Endpoint(EndpointEntity {
            id: 0,
            node: None,
            kind,
            topic,
            type_info,
            qos,
        }))
    }

    /// Encode QoS in ros2dds format.
    ///
    /// Format: `<keyless>:<reliability>:<durability>:<history_kind>,<depth>[:<user_data>]`
    /// - keyless: 'K' if not keyless, empty if keyless
    /// - reliability: 0=BEST_EFFORT, 1=RELIABLE, empty=default
    /// - durability: 0=VOLATILE, 1=TRANSIENT_LOCAL, empty=default
    /// - history: `<kind>,<depth>` where kind is 0=KEEP_LAST, 1=KEEP_ALL
    fn encode_qos(qos: &QosProfile, keyless: bool) -> String {
        let mut result = String::new();

        // Keyless flag
        if !keyless {
            result.push('K');
        }
        result.push(':');

        // Reliability
        match qos.reliability {
            QosReliability::BestEffort => result.push('0'),
            QosReliability::Reliable => result.push('1'),
        }
        result.push(':');

        // Durability
        match qos.durability {
            QosDurability::Volatile => result.push('0'),
            QosDurability::TransientLocal => result.push('1'),
        }
        result.push(':');

        // History
        match qos.history {
            QosHistory::KeepLast(depth) => {
                result.push_str(&format!("0,{}", depth));
            }
            QosHistory::KeepAll => {
                result.push_str("1,0");
            }
        }

        result
    }

    /// Decode QoS from ros2dds format.
    fn decode_qos(s: &str) -> Result<(bool, QosProfile)> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() < 4 {
            return Err(zenoh::Error::from(format!(
                "Invalid QoS format: expected at least 4 colon-separated parts, got {}",
                parts.len()
            )));
        }

        let keyless = parts[0].is_empty();

        let reliability = match parts[1] {
            "" => QosReliability::default(),
            "0" => QosReliability::BestEffort,
            "1" => QosReliability::Reliable,
            _ => QosReliability::default(),
        };

        let durability = match parts[2] {
            "" => QosDurability::default(),
            "0" => QosDurability::Volatile,
            "1" => QosDurability::TransientLocal,
            _ => QosDurability::default(),
        };

        let history = if parts[3].is_empty() {
            QosHistory::default()
        } else {
            let history_parts: Vec<&str> = parts[3].split(',').collect();
            if history_parts.len() >= 2 {
                let depth: usize = history_parts[1].parse().unwrap_or(10);
                match history_parts[0] {
                    "0" | "" => QosHistory::from_depth(depth),
                    "1" => QosHistory::KeepAll,
                    _ => QosHistory::from_depth(depth),
                }
            } else {
                QosHistory::default()
            }
        };

        Ok((
            keyless,
            QosProfile {
                reliability,
                durability,
                history,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mangle_demangle() {
        assert_eq!(Ros2DdsFormatter::mangle_name("/chatter"), "§chatter");
        assert_eq!(
            Ros2DdsFormatter::mangle_name("std_msgs/msg/String"),
            "std_msgs§msg§String"
        );
        assert_eq!(
            Ros2DdsFormatter::demangle_name("std_msgs§msg§String"),
            "std_msgs/msg/String"
        );
    }

    #[test]
    fn test_qos_encode_decode() {
        let qos = QosProfile::default();
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        assert!(encoded.starts_with("K:"));

        let (keyless, decoded) = Ros2DdsFormatter::decode_qos(&encoded).unwrap();
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
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        assert_eq!(encoded, "K:1:1:0,10");

        let (keyless, decoded) = Ros2DdsFormatter::decode_qos(&encoded).unwrap();
        assert!(!keyless);
        assert_eq!(decoded.reliability, QosReliability::Reliable);
        assert_eq!(decoded.durability, QosDurability::TransientLocal);
    }

    /// Test that QoS encoding matches zenoh-plugin-ros2dds format.
    ///
    /// Format: `<keyless>:<ReliabilityKind>:<DurabilityKind>:<HistoryKind>,<HistoryDepth>`
    /// where:
    /// - keyless: 'K' if not keyless, empty if keyless
    /// - ReliabilityKind: 0=BEST_EFFORT, 1=RELIABLE
    /// - DurabilityKind: 0=VOLATILE, 1=TRANSIENT_LOCAL
    /// - HistoryKind: 0=KEEP_LAST, 1=KEEP_ALL
    #[test]
    fn test_qos_format_compatibility_with_zenoh_plugin() {
        // Test default QoS (keyless=true) -> ":::"
        let qos = QosProfile::default();
        let encoded = Ros2DdsFormatter::encode_qos(&qos, true);
        // Keyless=true means empty first field
        assert!(
            encoded.starts_with(":"),
            "Expected ':' prefix for keyless, got: {}",
            encoded
        );

        // Test default QoS (keyless=false) -> "K:::"
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        assert!(
            encoded.starts_with("K:"),
            "Expected 'K:' prefix for non-keyless, got: {}",
            encoded
        );

        // Test RELIABLE -> kind=1
        let qos = QosProfile {
            reliability: QosReliability::Reliable,
            ..Default::default()
        };
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        let parts: Vec<&str> = encoded.split(':').collect();
        assert_eq!(parts[1], "1", "RELIABLE should be encoded as '1'");

        // Test BEST_EFFORT -> kind=0
        let qos = QosProfile {
            reliability: QosReliability::BestEffort,
            ..Default::default()
        };
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        let parts: Vec<&str> = encoded.split(':').collect();
        assert_eq!(parts[1], "0", "BEST_EFFORT should be encoded as '0'");

        // Test TRANSIENT_LOCAL -> kind=1
        let qos = QosProfile {
            durability: QosDurability::TransientLocal,
            ..Default::default()
        };
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        let parts: Vec<&str> = encoded.split(':').collect();
        assert_eq!(parts[2], "1", "TRANSIENT_LOCAL should be encoded as '1'");

        // Test VOLATILE -> kind=0
        let qos = QosProfile {
            durability: QosDurability::Volatile,
            ..Default::default()
        };
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        let parts: Vec<&str> = encoded.split(':').collect();
        assert_eq!(parts[2], "0", "VOLATILE should be encoded as '0'");

        // Test KEEP_LAST with depth -> "0,depth"
        let qos = QosProfile {
            history: QosHistory::from_depth(5),
            ..Default::default()
        };
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        let parts: Vec<&str> = encoded.split(':').collect();
        assert_eq!(parts[3], "0,5", "KEEP_LAST(5) should be encoded as '0,5'");

        // Test KEEP_ALL -> "1,0"
        let qos = QosProfile {
            history: QosHistory::KeepAll,
            ..Default::default()
        };
        let encoded = Ros2DdsFormatter::encode_qos(&qos, false);
        let parts: Vec<&str> = encoded.split(':').collect();
        assert_eq!(parts[3], "1,0", "KEEP_ALL should be encoded as '1,0'");
    }

    /// Test topic key expression format matches zenoh-plugin-ros2dds.
    ///
    /// zenoh-plugin-ros2dds uses simple topic names without type/hash:
    /// Topic: `/chatter` -> key expression: `chatter`
    #[test]
    fn test_topic_key_expr_format() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            1,
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

        let topic_ke = Ros2DdsFormatter::topic_key_expr(&entity).unwrap();
        // ros2dds uses simple topic name
        assert_eq!(topic_ke.as_str(), "chatter");
    }

    /// Test liveliness key expression format matches zenoh-plugin-ros2dds.
    ///
    /// Format: `@/<zenoh_id>/@ros2_lv/<kind>/<escaped_ke>/<type>/<qos>`
    #[test]
    fn test_liveliness_key_expr_format() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            1,
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

        let liveliness_ke = Ros2DdsFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should start with @/<zenoh_id>/@ros2_lv
        assert!(
            ke_str.starts_with("@/"),
            "Should start with '@/', got: {}",
            ke_str
        );
        assert!(
            ke_str.contains("/@ros2_lv/"),
            "Should contain '/@ros2_lv/', got: {}",
            ke_str
        );

        // Should contain MP for Publisher
        assert!(
            ke_str.contains("/MP/"),
            "Should contain '/MP/' for Publisher, got: {}",
            ke_str
        );

        // Should contain escaped topic name (chatter has no slashes, so unchanged)
        assert!(
            ke_str.contains("/chatter/"),
            "Should contain '/chatter/', got: {}",
            ke_str
        );

        // Should contain escaped type name with § instead of /
        assert!(
            ke_str.contains("std_msgs§msg§String"),
            "Should contain 'std_msgs§msg§String', got: {}",
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
            1,
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

        let liveliness_ke = Ros2DdsFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should contain MS for Subscription
        assert!(
            ke_str.contains("/MS/"),
            "Should contain '/MS/' for Subscription, got: {}",
            ke_str
        );
    }

    #[test]
    fn test_parse_liveliness_restores_absolute_topic_name() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            1,
            "test_node".to_string(),
            "/".to_string(),
            String::new(),
        );

        let entity = EndpointEntity {
            id: 1,
            node: Some(node),
            kind: EndpointKind::Publisher,
            topic: "/chatter".to_string(),
            type_info: Some(TypeInfo::new("std_msgs/msg/String", TypeHash::zero())),
            qos: QosProfile::default(),
        };

        let liveliness_ke = Ros2DdsFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let parsed = Ros2DdsFormatter::parse_liveliness(&liveliness_ke).unwrap();

        match parsed {
            Entity::Endpoint(endpoint) => {
                assert_eq!(endpoint.topic, "/chatter");
                assert!(endpoint.node.is_none());
            }
            other => panic!("expected endpoint entity, got {:?}", other),
        }
    }

    /// Test service server liveliness key expression
    #[test]
    fn test_service_liveliness_key_expr() {
        let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
        let node = NodeEntity::new(
            0,
            zid,
            1,
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

        let liveliness_ke = Ros2DdsFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should contain SS for Service
        assert!(
            ke_str.contains("/SS/"),
            "Should contain '/SS/' for Service, got: {}",
            ke_str
        );

        // Service liveliness should NOT have QoS suffix (per ros2dds format)
        // Check that it doesn't end with QoS pattern
        let parts: Vec<&str> = ke_str.split('/').collect();
        // Last part should be the escaped type, not a QoS string
        let last_part = parts.last().unwrap();
        assert!(
            !last_part.contains(':'),
            "Service liveliness should not have QoS suffix, got: {}",
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
            1,
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

        let liveliness_ke = Ros2DdsFormatter::liveliness_key_expr(&entity, &zid).unwrap();
        let ke_str = liveliness_ke.as_str();

        // Should contain SC for Client
        assert!(
            ke_str.contains("/SC/"),
            "Should contain '/SC/' for Client, got: {}",
            ke_str
        );
    }
}
