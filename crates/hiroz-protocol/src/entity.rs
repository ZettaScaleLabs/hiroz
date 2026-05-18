//! ROS 2 entity types for key expression generation.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use core::{fmt::Display, ops::Deref};
use zenoh::{key_expr::KeyExpr, session::ZenohId};

use crate::qos::QosProfile;

/// Placeholder for empty namespace/enclave.
pub const EMPTY_PLACEHOLDER: &str = "%";

/// Liveliness key expression wrapper.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LivelinessKE(pub KeyExpr<'static>);

impl LivelinessKE {
    pub fn new(ke: KeyExpr<'static>) -> Self {
        Self(ke)
    }
}

impl Deref for LivelinessKE {
    type Target = KeyExpr<'static>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Topic key expression wrapper.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TopicKE(KeyExpr<'static>);

impl TopicKE {
    pub fn new(ke: KeyExpr<'static>) -> Self {
        Self(ke)
    }
}

impl Deref for TopicKE {
    type Target = KeyExpr<'static>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for TopicKE {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// ROS 2 node entity.
#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct NodeEntity {
    pub domain_id: usize,
    pub z_id: ZenohId,
    pub id: usize,
    pub name: String,
    pub namespace: String,
    pub enclave: String,
}

impl NodeEntity {
    pub fn new(
        domain_id: usize,
        z_id: ZenohId,
        id: usize,
        name: String,
        namespace: String,
        enclave: String,
    ) -> Self {
        Self {
            domain_id,
            z_id,
            id,
            name,
            namespace,
            enclave,
        }
    }
}

/// ROS 2 endpoint kind (publisher, subscription, service, client).
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Publisher,
    Subscription,
    Service,
    Client,
}

impl Display for EndpointKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EndpointKind::Publisher => write!(f, "MP"),
            EndpointKind::Subscription => write!(f, "MS"),
            EndpointKind::Service => write!(f, "SS"),
            EndpointKind::Client => write!(f, "SC"),
        }
    }
}

impl core::str::FromStr for EndpointKind {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "MP" => Ok(EndpointKind::Publisher),
            "MS" => Ok(EndpointKind::Subscription),
            "SS" => Ok(EndpointKind::Service),
            "SC" => Ok(EndpointKind::Client),
            _ => Err("Invalid endpoint kind"),
        }
    }
}

/// ROS 2 entity kind: either a node or an endpoint.
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Node,
    Endpoint(EndpointKind),
}

impl Display for EntityKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EntityKind::Node => write!(f, "NN"),
            EntityKind::Endpoint(k) => k.fmt(f),
        }
    }
}

impl core::str::FromStr for EntityKind {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "NN" => Ok(EntityKind::Node),
            _ => Ok(EntityKind::Endpoint(s.parse()?)),
        }
    }
}

impl From<EndpointKind> for EntityKind {
    fn from(kind: EndpointKind) -> Self {
        EntityKind::Endpoint(kind)
    }
}

impl TryFrom<EntityKind> for EndpointKind {
    type Error = &'static str;

    fn try_from(kind: EntityKind) -> Result<Self, Self::Error> {
        match kind {
            EntityKind::Endpoint(k) => Ok(k),
            EntityKind::Node => Err("Node is not a valid endpoint kind"),
        }
    }
}

/// Type hash (RIHS format).
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct TypeHash {
    pub version: u8,
    pub value: [u8; 32],
}

impl TypeHash {
    /// Placeholder value for type hash when not supported (ROS 2 Humble)
    const TYPE_HASH_NOT_SUPPORTED: &'static str = "TypeHashNotSupported";

    pub const fn new(version: u8, value: [u8; 32]) -> Self {
        Self { version, value }
    }

    pub const fn zero() -> Self {
        Self {
            version: 1,
            value: [0u8; 32],
        }
    }

    pub fn from_rihs_string(rihs_str: &str) -> Option<Self> {
        // Handle ROS 2 Humble's "not supported" placeholder
        if rihs_str == Self::TYPE_HASH_NOT_SUPPORTED {
            return Some(TypeHash::zero());
        }

        if let Some(hex_part) = rihs_str.strip_prefix("RIHS01_") {
            if hex_part.len() == 64 {
                let mut hash_bytes = [0u8; 32];
                for (i, chunk) in hex_part.as_bytes().chunks(2).enumerate() {
                    if i < 32 {
                        if let Ok(byte_val) =
                            u8::from_str_radix(core::str::from_utf8(chunk).unwrap_or("00"), 16)
                        {
                            hash_bytes[i] = byte_val;
                        } else {
                            return None;
                        }
                    }
                }
                return Some(TypeHash {
                    version: 1,
                    value: hash_bytes,
                });
            }
        }
        None
    }

    /// Returns true when RIHS01 type hashing is available (not the case on ROS 2 Humble).
    pub fn is_supported() -> bool {
        cfg!(not(feature = "no-type-hash"))
    }

    pub fn to_rihs_string(&self) -> String {
        #[cfg(feature = "no-type-hash")]
        {
            // ROS 2 Humble doesn't support type hashing
            Self::TYPE_HASH_NOT_SUPPORTED.to_string()
        }

        #[cfg(not(feature = "no-type-hash"))]
        {
            use alloc::format;
            match self.version {
                1 => {
                    let hex_str: String = self.value.iter().map(|b| format!("{:02x}", b)).collect();
                    format!("RIHS01_{}", hex_str)
                }
                _ => format!(
                    "RIHS{:02x}_{}",
                    self.version,
                    self.value
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                ),
            }
        }
    }
}

impl Display for TypeHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.to_rihs_string())
    }
}

/// Type information (name + hash).
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub hash: TypeHash,
}

impl TypeInfo {
    pub fn new(name: &str, hash: TypeHash) -> Self {
        TypeInfo {
            name: name.to_string(),
            hash,
        }
    }
}

/// ROS 2 endpoint entity (publisher, subscription, service, client).
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct EndpointEntity {
    pub id: usize,
    pub node: Option<NodeEntity>,
    pub kind: EndpointKind,
    pub topic: String,
    pub type_info: Option<TypeInfo>,
    pub qos: QosProfile,
}

impl EndpointEntity {
    pub fn entity_kind(&self) -> EntityKind {
        self.kind.into()
    }
}

/// Generic ROS 2 entity (node or endpoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entity {
    Node(NodeEntity),
    Endpoint(EndpointEntity),
}

/// Errors during entity conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityConversionError {
    MissingAdminSpace,
    MissingDomainId,
    MissingZId,
    MissingNodeId,
    MissingEntityId,
    MissingEntityKind,
    MissingEnclave,
    MissingNamespace,
    MissingNodeName,
    MissingTopicName,
    MissingTopicType,
    MissingTopicHash,
    MissingTopicQoS,
    ParsingError,
    QosDecodeError(crate::qos::QosDecodeError),
}

impl Display for EntityConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EntityConversionError {}
