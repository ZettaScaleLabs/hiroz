//! # hiroz — Zenoh-native ROS 2 in pure Rust
//!
//! `hiroz` provides ROS 2-style pub/sub, services, and actions built directly
//! on [Zenoh](https://zenoh.io), with no C/C++ dependencies.
//!
//! ## Getting started
//!
//! ```rust,ignore
//! use hiroz::prelude::*;
//! use hiroz_msgs::std_msgs::String as RosString;
//!
//! let ctx = ZContextBuilder::default().build()?;
//! let node = ctx.create_node("talker").build()?;
//! let publisher = node.create_pub::<RosString>("/chatter").build()?;
//! publisher.async_publish(&RosString { data: "hello".into() }).await?;
//! ```
//!
//! ## Sync and async APIs
//!
//! Most hiroz types expose both a blocking and an async variant of each
//! operation. The naming convention is:
//!
//! | Suffix | Behaviour |
//! |--------|-----------|
//! | *(none)* | Blocking — safe to call from any context |
//! | `_async` | Async — must be `.await`ed inside a Tokio (or compatible) runtime |
//!
//! For example, [`ZPub::publish`](pubsub::ZPub::publish) blocks until
//! the put completes, while [`ZPub::async_publish`](pubsub::ZPub::async_publish)
//! yields to the async executor.
//!
//! ## Imports
//!
//! The easiest way to import all common types is via the prelude:
//!
//! ```rust,ignore
//! use hiroz::prelude::*;
//! ```
//!
//! Or import types individually from their modules.

/// ROS 2 action support (goal, feedback, result).
pub mod action;
/// Attachment helpers for carrying metadata alongside messages.
pub mod attachment;
/// Timestamp-indexed, capacity-bounded message cache.
pub mod cache;
mod common;
/// Configuration types and builder helpers.
pub mod config;
/// Zenoh session context and context builder.
pub mod context;
/// Dynamic (schema-less) message support.
pub mod dynamic;
pub mod encoding;
/// Entity identity types (`TypeHash`, `TypeInfo`).
pub mod entity;
/// Graph events emitted by the Zenoh network graph.
pub mod event;
#[cfg(feature = "ffi")]
pub mod ffi;
/// ROS 2 graph introspection (node/topic/service discovery).
pub mod graph;
/// ROS 2 lifecycle node support (state machine, lifecycle publisher).
pub mod lifecycle;
/// Typed message wrappers and helpers.
pub mod msg;
/// ROS 2 node creation and management.
pub mod node;
/// Convenience re-exports for common hiroz types.
pub mod prelude;
/// Publishers and subscribers.
pub mod pubsub;
/// Python FFI bridge types.
pub mod python_bridge;
/// Quality-of-Service profiles and options.
pub mod qos;
/// Internal message queues.
pub mod queue;
/// Message type metadata traits (`WithTypeInfo`, etc.).
pub mod ros_msg;
/// ROS 2 service client and server.
pub mod service;
/// Shared-memory transport helpers.
pub mod shm;
/// Time and clock primitives for runtime and replay integration.
pub mod time;
/// ROS 2 topic name validation and manipulation.
pub mod topic_name;
/// Owned Zenoh buffer type.
pub mod zbuf;

/// Zero-copy buffer view for Python bindings.
#[cfg(feature = "python")]
pub mod zbuf_view;

#[macro_use]
pub mod utils;

/// ROS 2 parameter subsystem.
pub mod parameter;

pub use attachment::GidArray;
pub use entity::{TypeHash, TypeInfo};
pub use hiroz_derive::MessageTypeInfo;
pub use ros_msg::{ActionTypeInfo, MessageTypeInfo, ServiceTypeInfo, WithTypeInfo};
pub use zbuf::ZBuf;
pub use zenoh::Result;

/// Builds a configured object, consuming the builder.
///
/// All hiroz builders implement this trait. Bring it into scope to call `.build()`:
///
/// ```rust,ignore
/// use hiroz::Builder;
/// let ctx = ZContextBuilder::default().build()?;
/// ```
///
/// Alternatively, use `use hiroz::prelude::*;` which includes `Builder`.
pub trait Builder {
    /// The type produced by this builder.
    type Output;
    /// Consume the builder and construct the configured object.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid or if network/resource
    /// initialization fails (e.g. Zenoh session could not be opened).
    fn build(self) -> Result<Self::Output>;
}

impl Builder for config::RouterConfigBuilder {
    type Output = zenoh::Config;
    fn build(self) -> Result<zenoh::Config> {
        self.build_config()
    }
}

impl Builder for config::SessionConfigBuilder {
    type Output = zenoh::Config;
    fn build(self) -> Result<zenoh::Config> {
        self.build_config()
    }
}
