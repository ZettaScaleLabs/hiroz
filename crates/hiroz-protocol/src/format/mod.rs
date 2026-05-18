//! Key expression format trait and implementations.
//!
//! This module provides the [`KeyExprFormatter`] trait and concrete implementations
//! for different key expression formats.

#[cfg(feature = "rmw-zenoh")]
pub mod rmw_zenoh;

#[cfg(feature = "ros2dds")]
pub mod ros2dds;

use alloc::string::String;
use zenoh::{key_expr::KeyExpr, session::ZenohId, Result};

use crate::{
    entity::{EndpointEntity, Entity, LivelinessKE, NodeEntity, TopicKE},
    qos::QosProfile,
};

/// Key expression format selector.
///
/// Determines which key expression format to use for ROS 2 <-> Zenoh mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum KeyExprFormat {
    /// rmw_zenoh_cpp compatible format (default).
    ///
    /// - Topic key expressions use `strip_slashes()` (preserve internal slashes)
    /// - Liveliness tokens use mangling (replace `/` with `%`)
    /// - Format: `<domain>/<topic>/<type>/<hash>`
    #[default]
    RmwZenoh,

    /// zenoh-plugin-ros2dds compatible format.
    ///
    /// Different key expression structure for DDS bridge compatibility.
    Ros2Dds,
}

#[allow(unused_variables)]
impl KeyExprFormat {
    /// Generate topic key expression for data publication/subscription.
    pub fn topic_key_expr(&self, entity: &EndpointEntity) -> Result<TopicKE> {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    rmw_zenoh::RmwZenohFormatter::topic_key_expr(entity)
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    Err(zenoh::Error::from("rmw-zenoh format not enabled"))
                }
            }
            KeyExprFormat::Ros2Dds => {
                #[cfg(feature = "ros2dds")]
                {
                    ros2dds::Ros2DdsFormatter::topic_key_expr(entity)
                }
                #[cfg(not(feature = "ros2dds"))]
                {
                    Err(zenoh::Error::from("ros2dds format not enabled"))
                }
            }
        }
    }

    /// Generate liveliness token for endpoint entity discovery.
    pub fn liveliness_key_expr(
        &self,
        entity: &EndpointEntity,
        zid: &ZenohId,
    ) -> Result<LivelinessKE> {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    rmw_zenoh::RmwZenohFormatter::liveliness_key_expr(entity, zid)
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    Err(zenoh::Error::from("rmw-zenoh format not enabled"))
                }
            }
            KeyExprFormat::Ros2Dds => {
                #[cfg(feature = "ros2dds")]
                {
                    ros2dds::Ros2DdsFormatter::liveliness_key_expr(entity, zid)
                }
                #[cfg(not(feature = "ros2dds"))]
                {
                    Err(zenoh::Error::from("ros2dds format not enabled"))
                }
            }
        }
    }

    /// Generate liveliness token for node entity discovery.
    pub fn node_liveliness_key_expr(&self, entity: &NodeEntity) -> Result<LivelinessKE> {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    rmw_zenoh::RmwZenohFormatter::node_liveliness_key_expr(entity)
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    Err(zenoh::Error::from("rmw-zenoh format not enabled"))
                }
            }
            KeyExprFormat::Ros2Dds => {
                #[cfg(feature = "ros2dds")]
                {
                    ros2dds::Ros2DdsFormatter::node_liveliness_key_expr(entity)
                }
                #[cfg(not(feature = "ros2dds"))]
                {
                    Err(zenoh::Error::from("ros2dds format not enabled"))
                }
            }
        }
    }

    /// Parse liveliness token back to entity.
    pub fn parse_liveliness(&self, ke: &KeyExpr) -> Result<Entity> {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    rmw_zenoh::RmwZenohFormatter::parse_liveliness(ke)
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    Err(zenoh::Error::from("rmw-zenoh format not enabled"))
                }
            }
            KeyExprFormat::Ros2Dds => {
                #[cfg(feature = "ros2dds")]
                {
                    ros2dds::Ros2DdsFormatter::parse_liveliness(ke)
                }
                #[cfg(not(feature = "ros2dds"))]
                {
                    Err(zenoh::Error::from("ros2dds format not enabled"))
                }
            }
        }
    }

    /// Encode QoS for liveliness token.
    pub fn encode_qos(&self, qos: &QosProfile, keyless: bool) -> String {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    rmw_zenoh::RmwZenohFormatter::encode_qos(qos, keyless)
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    String::new()
                }
            }
            KeyExprFormat::Ros2Dds => {
                #[cfg(feature = "ros2dds")]
                {
                    ros2dds::Ros2DdsFormatter::encode_qos(qos, keyless)
                }
                #[cfg(not(feature = "ros2dds"))]
                {
                    String::new()
                }
            }
        }
    }

    /// Decode QoS from liveliness token.
    pub fn decode_qos(&self, s: &str) -> Result<(bool, QosProfile)> {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    rmw_zenoh::RmwZenohFormatter::decode_qos(s)
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    Err(zenoh::Error::from("rmw-zenoh format not enabled"))
                }
            }
            KeyExprFormat::Ros2Dds => {
                #[cfg(feature = "ros2dds")]
                {
                    ros2dds::Ros2DdsFormatter::decode_qos(s)
                }
                #[cfg(not(feature = "ros2dds"))]
                {
                    Err(zenoh::Error::from("ros2dds format not enabled"))
                }
            }
        }
    }
}

/// Trait for key expression format implementations.
///
/// This trait abstracts the differences between key expression formats
/// used by different Zenoh-ROS bridges.
pub trait KeyExprFormatter {
    /// Escape character used to replace slashes in key expressions.
    const ESCAPE_CHAR: char;

    /// Admin space prefix for liveliness tokens.
    const ADMIN_SPACE: &'static str;

    /// Generate topic key expression for data publication/subscription.
    fn topic_key_expr(entity: &EndpointEntity) -> Result<TopicKE>;

    /// Generate liveliness token for endpoint entity discovery.
    fn liveliness_key_expr(entity: &EndpointEntity, zid: &ZenohId) -> Result<LivelinessKE>;

    /// Generate liveliness token for node entity discovery.
    fn node_liveliness_key_expr(entity: &NodeEntity) -> Result<LivelinessKE>;

    /// Parse liveliness token back to entity.
    fn parse_liveliness(ke: &KeyExpr) -> Result<Entity>;

    /// Mangle a name (replace slashes with escape char).
    fn mangle_name(name: &str) -> String {
        name.replace('/', &Self::ESCAPE_CHAR.to_string())
    }

    /// Demangle a name (restore slashes from escape char).
    fn demangle_name(name: &str) -> String {
        name.replace(Self::ESCAPE_CHAR, "/")
    }

    /// Encode QoS for liveliness token.
    fn encode_qos(qos: &QosProfile, keyless: bool) -> String;

    /// Decode QoS from liveliness token.
    fn decode_qos(s: &str) -> Result<(bool, QosProfile)>;
}
