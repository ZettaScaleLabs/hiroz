//! DdsBridgeExt — typed extension trait on ZNode for ergonomic DDS bridging.

use anyhow::Result;
use hiroz::{MessageTypeInfo, node::ZNode};
use hiroz_protocol::entity::TypeHash;

use super::{
    names::dds_type_to_ros2_type,
    participant::{BridgeQos, DdsParticipant},
    pubsub::{ZDdsPubBridge, ZDdsSubBridge},
};

/// Typed convenience methods on [`ZNode`] for creating DDS↔Zenoh pub/sub bridges.
///
/// Each method extracts the ROS 2 type name and type hash from the compile-time
/// type parameter `T` and delegates to the corresponding untyped bridge constructor.
///
/// ```rust,ignore
/// use ros_z_dds::DdsBridgeExt;
///
/// let _pub = node.bridge_dds_pub::<RosString>("/chatter", &participant).await?;
/// let _sub = node.bridge_dds_sub::<RosString>("/chatter", &participant).await?;
/// ```
#[allow(async_fn_in_trait)]
pub trait DdsBridgeExt {
    /// Bridge a DDS topic publisher to a Zenoh publisher.
    ///
    /// The type `T` must implement `MessageTypeInfo` — all generated ros-z message
    /// types satisfy this automatically.
    async fn bridge_dds_pub<T: MessageTypeInfo>(
        &self,
        topic: &str,
        participant: &impl DdsParticipant,
    ) -> Result<ZDdsPubBridge<impl DdsParticipant>>;

    /// Bridge a Zenoh subscriber to a DDS topic writer.
    async fn bridge_dds_sub<T: MessageTypeInfo>(
        &self,
        topic: &str,
        participant: &impl DdsParticipant,
    ) -> Result<ZDdsSubBridge<impl DdsParticipant>>;
}

impl DdsBridgeExt for ZNode {
    async fn bridge_dds_pub<T: MessageTypeInfo>(
        &self,
        topic: &str,
        participant: &impl DdsParticipant,
    ) -> Result<ZDdsPubBridge<impl DdsParticipant>> {
        let ros2_type = dds_type_to_ros2_type(T::type_name());
        let type_hash: TypeHash = T::type_hash();
        ZDdsPubBridge::new(
            self,
            topic,
            &ros2_type,
            Some(type_hash),
            participant,
            BridgeQos::default(),
            true,
            10,
        )
        .await
    }

    async fn bridge_dds_sub<T: MessageTypeInfo>(
        &self,
        topic: &str,
        participant: &impl DdsParticipant,
    ) -> Result<ZDdsSubBridge<impl DdsParticipant>> {
        let ros2_type = dds_type_to_ros2_type(T::type_name());
        let type_hash: TypeHash = T::type_hash();
        ZDdsSubBridge::new(
            self,
            topic,
            &ros2_type,
            Some(type_hash),
            participant,
            BridgeQos::default(),
            true,
        )
        .await
    }
}
