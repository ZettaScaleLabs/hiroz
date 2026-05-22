use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tracing::warn;
use zenoh::Result;

use crate::{
    msg::{ZMessage, ZSerializer},
    pubsub::ZPub,
};

/// Trait for entities managed by [`ZLifecycleNode`](super::node::ZLifecycleNode).
///
/// Lifecycle publishers implement this trait so the node can bulk-activate or
/// bulk-deactivate all its publishers when a lifecycle transition occurs.
pub trait ManagedEntity: Send + Sync {
    fn on_activate(&self);
    fn on_deactivate(&self);
}

/// A ROS 2 lifecycle-aware publisher.
///
/// Wraps a [`ZPub`] and silently drops `publish()` calls while the publisher is
/// in the deactivated state. The activated state is toggled by the owning
/// [`ZLifecycleNode`](super::node::ZLifecycleNode) via the [`ManagedEntity`] trait.
///
/// # Example
///
/// ```no_run
/// use hiroz::lifecycle::ZLifecycleNode;
/// use hiroz_msgs::ros::std_msgs::String as RosString;
///
/// # fn example(node: &mut ZLifecycleNode) -> zenoh::Result<()> {
/// let pub_ = node.create_publisher::<RosString>("chatter")?;
/// // pub_ drops messages until the node is activated
/// # Ok(())
/// # }
/// ```
pub struct ZLifecyclePublisher<T: ZMessage, S: ZSerializer = <T as ZMessage>::Serdes> {
    inner: ZPub<T, S>,
    activated: Arc<AtomicBool>,
    /// Throttle "publisher not activated" warnings to one per deactivation cycle.
    should_warn: AtomicBool,
}

impl<T: ZMessage, S: ZSerializer> ZLifecyclePublisher<T, S> {
    pub(super) fn new(inner: ZPub<T, S>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            activated: Arc::new(AtomicBool::new(false)),
            should_warn: AtomicBool::new(true),
        })
    }

    /// Publish a message. Silently dropped (with one warning) when deactivated.
    pub fn publish(&self, msg: &T) -> Result<()>
    where
        T: 'static,
        S: for<'a> crate::msg::ZSerializer<Input<'a> = &'a T> + 'static,
    {
        if !self.activated.load(Ordering::Relaxed) {
            if self
                .should_warn
                .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                warn!(
                    topic = %self.inner.entity.topic,
                    "publish() called while lifecycle publisher is deactivated — message dropped"
                );
            }
            return Ok(());
        }
        self.inner.publish(msg)
    }

    /// Returns `true` if this publisher is currently activated.
    pub fn is_activated(&self) -> bool {
        self.activated.load(Ordering::Relaxed)
    }

    /// The fully-qualified topic name.
    pub fn topic_name(&self) -> &str {
        &self.inner.entity.topic
    }
}

impl<T: ZMessage, S: ZSerializer + Send + Sync> ManagedEntity for ZLifecyclePublisher<T, S> {
    fn on_activate(&self) {
        self.activated.store(true, Ordering::Relaxed);
        // Re-arm the warning so it fires on the next deactivation cycle.
        self.should_warn.store(true, Ordering::Relaxed);
    }

    fn on_deactivate(&self) {
        self.activated.store(false, Ordering::Relaxed);
    }
}

impl<T: ZMessage + std::fmt::Debug, S: ZSerializer> std::fmt::Debug for ZLifecyclePublisher<T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZLifecyclePublisher")
            .field("topic", &self.inner.entity.topic)
            .field("activated", &self.activated.load(Ordering::Relaxed))
            .finish()
    }
}
