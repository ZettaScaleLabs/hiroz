//! Convenience re-exports for common hiroz types.
//!
//! Import everything with `use hiroz::prelude::*;` to get the types and traits
//! needed for most use cases without hunting through submodules.
//!
//! # Example
//!
//! ```rust,ignore
//! use hiroz::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let ctx = ZContextBuilder::default().build()?;
//!     let node = ctx.create_node("my_node").build()?;
//!     // ...
//!     Ok(())
//! }
//! ```

/// The builder trait — required to call `.build()` on any builder type.
pub use crate::Builder;

/// Core runtime types.
pub use crate::context::{ZContext, ZContextBuilder};
pub use crate::node::ZNode;
pub use crate::pubsub::{ZPub, ZSub};
pub use crate::service::{RequestId, ServiceReply, ServiceRequest, ZClient, ZServer};

pub use crate::action::server::{Accepted, Executing, Requested};
/// Action types.
pub use crate::action::{ClientGoalHandle, GoalId, GoalStatus};

/// QoS configuration types.
pub use crate::qos::{
    QosDurability, QosDuration, QosHistory, QosLiveliness, QosProfile, QosReliability,
};

/// Action type marker trait.
pub use crate::action::ZAction;

/// Trait bounds for custom messages and services.
pub use crate::ros_msg::{ActionTypeInfo, MessageTypeInfo, ServiceTypeInfo, WithTypeInfo};

/// Type identity helpers for custom message definitions.
pub use crate::entity::{TypeHash, TypeInfo};

/// Cache subscriber: retains a sliding window of messages indexed by time.
pub use crate::cache::{ExtractorStamp, ZCache, ZCacheBuilder, ZenohStamp};

/// Parameter types for ROS 2-compatible node parameters.
pub use crate::parameter::{
    FloatingPointRange, IntegerRange, Parameter, ParameterClient, ParameterDescriptor,
    ParameterList, ParameterTarget, ParameterType, ParameterValue, SetParametersResult,
};

/// The `Result` alias used throughout hiroz (equivalent to `zenoh::Result`).
pub use zenoh::Result;

/// Lifecycle node support.
pub use crate::lifecycle::{
    CallbackReturn, LifecycleState, ManagedEntity, ZLifecycleClient, ZLifecycleNode,
    ZLifecyclePublisher,
};

/// Time and clock support.
pub use crate::time::{ClockKind, ZClock, ZDuration, ZInterval, ZSleep, ZTime};
