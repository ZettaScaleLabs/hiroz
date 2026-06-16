//! Key expression format handling for hiroz.
//!
//! This crate provides key expression generation for mapping ROS 2 entities
//! (nodes, topics, services, actions) to Zenoh key expressions.
//!
//! # Formats
//!
//! hiroz supports multiple key expression formats:
//!
//! - **RmwZenoh** (default): Compatible with `rmw_zenoh_cpp`, the official
//!   ROS 2 RMW implementation using Zenoh. Uses `strip_slashes()` for topic
//!   key expressions and mangling for liveliness tokens.
//!
//! # no_std Support
//!
//! This crate is `no_std` compatible with `alloc`:
//!
//! ```toml
//! [dependencies]
//! hiroz-protocol = { version = "0.1", default-features = false }
//! ```
//!
//! # Example
//!
//! ```rust
//! use hiroz_protocol::{KeyExprFormat, entity::*};
//!
//! let format = KeyExprFormat::default(); // RmwZenoh
//! let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
//! let node = NodeEntity::new(0, zid, 0, "my_node".to_string(), "/".to_string(), String::new());
//!
//! let entity = EndpointEntity {
//!     id: 1,
//!     node: Some(node),
//!     kind: EndpointKind::Publisher,
//!     topic: "/chatter".to_string(),
//!     type_info: None,
//!     qos: Default::default(),
//! };
//!
//! // Generate topic key expression
//! let topic_ke = format.topic_key_expr(&entity).unwrap();
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod entity;
pub mod format;
pub mod qos;

pub use entity::{
    EndpointEntity, EndpointKind, Entity, EntityKind, NodeEntity, TypeHash, TypeInfo,
};
#[cfg(feature = "rmw-zenoh")]
pub use format::rmw_zenoh::RmwZenohFormatter;
pub use format::{DynKeyExprFormatter, KeyExprFormat, KeyExprFormatter, KeyExprFormatterAdapter};
