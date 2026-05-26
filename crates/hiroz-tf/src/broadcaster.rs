use std::num::NonZeroUsize;

use hiroz::Builder;
use hiroz::msg::NativeCdrSerdes;
use hiroz::node::ZNode;
use hiroz::pubsub::ZPub;
use hiroz::qos::{QosDurability, QosHistory, QosProfile, QosReliability};
use hiroz_msgs::geometry_msgs::TransformStamped;
use hiroz_msgs::tf2_msgs::TFMessage;

type TfPub = ZPub<TFMessage, NativeCdrSerdes<TFMessage>>;

fn volatile_qos() -> QosProfile {
    QosProfile {
        reliability: QosReliability::Reliable,
        durability: QosDurability::Volatile,
        history: QosHistory::KeepLast(NonZeroUsize::new(100).unwrap()),
        ..Default::default()
    }
}

fn transient_local_qos() -> QosProfile {
    QosProfile {
        reliability: QosReliability::Reliable,
        durability: QosDurability::TransientLocal,
        history: QosHistory::KeepLast(NonZeroUsize::new(100).unwrap()),
        ..Default::default()
    }
}

/// Publishes dynamic transforms to `/tf` (Volatile durability).
///
/// Use [`crate::Buffer`] on the same or another node to receive these transforms.
pub struct TransformBroadcaster {
    pub_: TfPub,
}

impl TransformBroadcaster {
    /// Create a broadcaster attached to `node`. Declares a publisher on `/tf`.
    pub fn new(node: &ZNode) -> zenoh::Result<Self> {
        let pub_ = node
            .create_pub::<TFMessage>("/tf")
            .with_qos(volatile_qos())
            .build()?;
        Ok(Self { pub_ })
    }

    /// Publish a single transform to `/tf`.
    pub fn send_transform(&self, tf: TransformStamped) -> zenoh::Result<()> {
        self.send_transforms(vec![tf])
    }

    /// Publish multiple transforms to `/tf` in a single message.
    pub fn send_transforms(&self, transforms: Vec<TransformStamped>) -> zenoh::Result<()> {
        self.pub_.publish(&TFMessage { transforms })
    }
}

/// Publishes static transforms to `/tf_static` (TransientLocal durability).
///
/// Late-joining subscribers automatically receive all previously published
/// static transforms via `PublicationCache` replay.
///
/// All timestamps are unconditionally set to `{sec: 0, nanosec: 0}` before
/// publishing, which is required by the tf2 standard for `/tf_static` messages
/// and ensures interoperability with ROS 2 tf2 clients and rviz2.
pub struct StaticTransformBroadcaster {
    pub_: TfPub,
}

impl StaticTransformBroadcaster {
    /// Create a static broadcaster attached to `node`. Declares a publisher on `/tf_static`.
    pub fn new(node: &ZNode) -> zenoh::Result<Self> {
        let pub_ = node
            .create_pub::<TFMessage>("/tf_static")
            .with_qos(transient_local_qos())
            .build()?;
        Ok(Self { pub_ })
    }

    /// Publish a single static transform to `/tf_static`.
    ///
    /// The timestamp in `tf` is overwritten with zero before publishing.
    pub fn send_transform(&self, tf: TransformStamped) -> zenoh::Result<()> {
        self.send_transforms(vec![tf])
    }

    /// Publish multiple static transforms to `/tf_static` in a single message.
    ///
    /// All timestamps are overwritten with zero before publishing.
    pub fn send_transforms(&self, transforms: Vec<TransformStamped>) -> zenoh::Result<()> {
        self.pub_.publish(&TFMessage {
            transforms: zero_timestamps(transforms),
        })
    }
}

/// Zero all timestamps in `transforms`, as required by the tf2 standard for `/tf_static`.
fn zero_timestamps(transforms: Vec<TransformStamped>) -> Vec<TransformStamped> {
    transforms
        .into_iter()
        .map(|mut tf| {
            tf.header.stamp = hiroz_msgs::builtin_interfaces::Time { sec: 0, nanosec: 0 };
            tf
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiroz_msgs::builtin_interfaces::Time;
    use hiroz_msgs::geometry_msgs::{Quaternion, Transform, Vector3};
    use hiroz_msgs::std_msgs::Header;

    fn make_tf(parent: &str, child: &str, sec: i32) -> TransformStamped {
        TransformStamped {
            header: Header {
                frame_id: parent.to_string(),
                stamp: Time { sec, nanosec: 500 },
            },
            child_frame_id: child.to_string(),
            transform: Transform {
                translation: Vector3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
                rotation: Quaternion {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            },
        }
    }

    #[test]
    fn zero_timestamps_clears_all_stamps() {
        let tfs = vec![make_tf("map", "odom", 10), make_tf("odom", "base_link", 20)];
        let zeroed = zero_timestamps(tfs);
        for tf in &zeroed {
            assert_eq!(tf.header.stamp.sec, 0);
            assert_eq!(tf.header.stamp.nanosec, 0);
        }
    }

    #[test]
    fn zero_timestamps_preserves_other_fields() {
        let tfs = vec![make_tf("map", "sensor", 5)];
        let zeroed = zero_timestamps(tfs);
        assert_eq!(zeroed[0].header.frame_id, "map");
        assert_eq!(zeroed[0].child_frame_id, "sensor");
        assert!((zeroed[0].transform.translation.x - 1.0).abs() < 1e-10);
    }
}
