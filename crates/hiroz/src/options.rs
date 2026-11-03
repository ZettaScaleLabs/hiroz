//! Option structs that a node holds as defaults and a builder holds as its own copy.

use crate::BufferGroup;

/// Options of a subscriber.
///
/// A node holds one value as the default for every subscriber it creates, and each subscriber
/// builder starts from a copy of it. Set the node default with
/// [`ZNodeBuilder::with_subscriber_options`](crate::node::ZNodeBuilder::with_subscriber_options). A
/// setting on a builder, such as `with_buffer_group`, replaces that setting for that subscriber
/// only. The default value changes no behaviour.
///
/// The struct is `#[non_exhaustive]`: a later release can add options without a breaking change.
/// Start from [`SubscriberOptions::default`] and chain the `with_` methods.
///
/// # Example
///
/// ```no_run
/// use hiroz::{BufferGroup, Builder, SubscriberOptions};
///
/// # fn main() -> zenoh::Result<()> {
/// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
/// let node = ctx
///     .create_node("my_node")
///     .with_subscriber_options(SubscriberOptions::default().with_buffer_group(BufferGroup::Off))
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SubscriberOptions {
    /// Whether a subscriber joins rmw_zenoh's CPU buffer group. The default is
    /// [`BufferGroup::Auto`].
    pub buffer_group: BufferGroup,
}

impl SubscriberOptions {
    /// Set [`buffer_group`](SubscriberOptions::buffer_group).
    pub fn with_buffer_group(mut self, group: BufferGroup) -> Self {
        self.buffer_group = group;
        self
    }
}
