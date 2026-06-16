//! Key expression format trait and implementations.
//!
//! This module provides the [`KeyExprFormatter`] trait and concrete implementations
//! for different key expression formats, plus the [`DynKeyExprFormatter`] object-safe
//! companion for runtime extension via [`KeyExprFormat::Custom`].

#[cfg(feature = "rmw-zenoh")]
pub mod rmw_zenoh;

use alloc::{string::String, sync::Arc};
use core::{fmt, marker::PhantomData};
use zenoh::{key_expr::KeyExpr, session::ZenohId, Result};

use crate::{
    entity::{EndpointEntity, Entity, LivelinessKE, NodeEntity, TopicKE},
    qos::QosProfile,
};

/// Key expression format selector.
///
/// Determines which key expression format to use for ROS 2 <-> Zenoh mapping.
///
/// The built-in [`RmwZenoh`](KeyExprFormat::RmwZenoh) variant covers the rmw_zenoh_cpp
/// wire format. For any other format — e.g. a legacy or proprietary bridge wire format —
/// use [`Custom`](KeyExprFormat::Custom) with a [`DynKeyExprFormatter`] implementation:
///
/// ```rust,ignore
/// use hiroz_protocol::{KeyExprFormat, DynKeyExprFormatter};
/// use std::sync::Arc;
///
/// let format = KeyExprFormat::Custom(Arc::new(MyFormat));
/// ```
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum KeyExprFormat {
    /// rmw_zenoh_cpp compatible format (default).
    ///
    /// - Topic key expressions use `strip_slashes()` (preserve internal slashes)
    /// - Liveliness tokens use mangling (replace `/` with `%`)
    /// - Format: `<domain>/<topic>/<type>/<hash>`
    #[default]
    RmwZenoh,
    /// Runtime-provided format. Useful for external crates that need a different
    /// wire format without forking hiroz.
    Custom(Arc<dyn DynKeyExprFormatter>),
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
            KeyExprFormat::Custom(f) => f.topic_key_expr(entity),
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
            KeyExprFormat::Custom(f) => f.liveliness_key_expr(entity, zid),
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
            KeyExprFormat::Custom(f) => f.node_liveliness_key_expr(entity),
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
            KeyExprFormat::Custom(f) => f.parse_liveliness(ke),
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
            KeyExprFormat::Custom(f) => f.encode_qos(qos, keyless),
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
            KeyExprFormat::Custom(f) => f.decode_qos(s),
        }
    }

    /// Zenoh key expression pattern for the liveliness subscriber that drives graph discovery.
    ///
    /// For `RmwZenoh` this is `"@ros2_lv/{domain_id}/**"`. Custom formats supply their own
    /// admin space prefix via [`DynKeyExprFormatter::liveliness_pattern`].
    pub fn liveliness_pattern(&self, domain_id: usize) -> String {
        match self {
            KeyExprFormat::RmwZenoh => {
                #[cfg(feature = "rmw-zenoh")]
                {
                    alloc::format!(
                        "{}/{domain_id}/**",
                        <rmw_zenoh::RmwZenohFormatter as KeyExprFormatter>::ADMIN_SPACE
                    )
                }
                #[cfg(not(feature = "rmw-zenoh"))]
                {
                    String::new()
                }
            }
            KeyExprFormat::Custom(f) => f.liveliness_pattern(domain_id),
        }
    }
}

/// Object-safe companion to [`KeyExprFormatter`] for runtime-provided formats.
///
/// Implement this trait to supply a custom wire format to [`KeyExprFormat::Custom`].
/// Unlike [`KeyExprFormatter`] (which uses associated consts and static dispatch),
/// every method here takes `&self` so the implementation can be boxed as
/// `Arc<dyn DynKeyExprFormatter>`.
///
/// If your format is already a zero-sized type implementing [`KeyExprFormatter`],
/// use [`KeyExprFormatterAdapter`] to avoid writing the delegation boilerplate:
///
/// ```rust,ignore
/// use hiroz_protocol::{KeyExprFormat, KeyExprFormatterAdapter};
/// use std::sync::Arc;
///
/// let format = KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<MyFormatter>::new()));
/// ```
pub trait DynKeyExprFormatter: fmt::Debug + Send + Sync {
    /// Generate topic key expression for data publication/subscription.
    fn topic_key_expr(&self, entity: &EndpointEntity) -> Result<TopicKE>;

    /// Generate liveliness token for endpoint entity discovery.
    fn liveliness_key_expr(&self, entity: &EndpointEntity, zid: &ZenohId) -> Result<LivelinessKE>;

    /// Generate liveliness token for node entity discovery.
    fn node_liveliness_key_expr(&self, entity: &NodeEntity) -> Result<LivelinessKE>;

    /// Parse liveliness token back to entity.
    fn parse_liveliness(&self, ke: &KeyExpr) -> Result<Entity>;

    /// Encode QoS for liveliness token.
    fn encode_qos(&self, qos: &QosProfile, keyless: bool) -> String;

    /// Decode QoS from liveliness token.
    fn decode_qos(&self, s: &str) -> Result<(bool, QosProfile)>;

    /// Zenoh key expression pattern for the liveliness subscriber (e.g. `"@my_lv/{domain_id}/**"`).
    fn liveliness_pattern(&self, domain_id: usize) -> String;
}

/// Adapts any zero-sized [`KeyExprFormatter`] (static dispatch) into a [`DynKeyExprFormatter`]
/// (object-safe, heap-allocatable).
///
/// `T` must be a zero-sized type implementing [`KeyExprFormatter`].
///
/// # Example
///
/// ```rust,ignore
/// use hiroz_protocol::{KeyExprFormat, KeyExprFormatterAdapter};
/// use hiroz_protocol::format::rmw_zenoh::RmwZenohFormatter;
/// use std::sync::Arc;
///
/// // Equivalent to KeyExprFormat::RmwZenoh but via the dynamic path:
/// let fmt = KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<RmwZenohFormatter>::new()));
/// ```
pub struct KeyExprFormatterAdapter<T: KeyExprFormatter>(PhantomData<T>);

impl<T: KeyExprFormatter> KeyExprFormatterAdapter<T> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: KeyExprFormatter> Default for KeyExprFormatterAdapter<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: KeyExprFormatter> fmt::Debug for KeyExprFormatterAdapter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "KeyExprFormatterAdapter<{}>",
            core::any::type_name::<T>()
        )
    }
}

impl<T: KeyExprFormatter + Send + Sync + 'static> DynKeyExprFormatter
    for KeyExprFormatterAdapter<T>
{
    fn topic_key_expr(&self, entity: &EndpointEntity) -> Result<TopicKE> {
        T::topic_key_expr(entity)
    }

    fn liveliness_key_expr(&self, entity: &EndpointEntity, zid: &ZenohId) -> Result<LivelinessKE> {
        T::liveliness_key_expr(entity, zid)
    }

    fn node_liveliness_key_expr(&self, entity: &NodeEntity) -> Result<LivelinessKE> {
        T::node_liveliness_key_expr(entity)
    }

    fn parse_liveliness(&self, ke: &KeyExpr) -> Result<Entity> {
        T::parse_liveliness(ke)
    }

    fn encode_qos(&self, qos: &QosProfile, keyless: bool) -> String {
        T::encode_qos(qos, keyless)
    }

    fn decode_qos(&self, s: &str) -> Result<(bool, QosProfile)> {
        T::decode_qos(s)
    }

    fn liveliness_pattern(&self, domain_id: usize) -> String {
        alloc::format!("{}/{domain_id}/**", T::ADMIN_SPACE)
    }
}

/// Trait for key expression format implementations.
///
/// This trait abstracts the differences between key expression formats
/// used by different Zenoh-ROS bridges.
///
/// All methods are static (no `self`), making this trait suitable for zero-sized
/// formatter types and compile-time dispatch. For runtime extension, see
/// [`DynKeyExprFormatter`] and [`KeyExprFormatterAdapter`].
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
