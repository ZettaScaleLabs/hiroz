//! Subscriber side of rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh_cpp` publisher writes a message with a buffer field on
//! `<base>/_buf_cpu`. The payload is plain CDR. The publisher does this for each subscriber whose
//! liveliness token ends in `backends:cpu:`. The released publisher writes nothing on `<base>` for
//! a subscriber whose token has no `backends:` field. A publisher that is not buffer-aware writes
//! `<base>` only.
//!
//! A subscriber joins the group when its token ends in `backends:cpu:`. It then has two exact
//! subscriptions, on `<base>` and on `<base>/_buf_cpu`, as `rmw_zenoh_cpp` has. It needs `<base>`
//! for publishers that are not buffer-aware. It reads no other key under `<base>`, so it never
//! receives the samples that a publisher writes for other subscribers.
//!
//! The build declares both subscriptions first and the token last, as `rmw_zenoh_cpp` does in
//! `SubscriptionData::init`. A publisher that reads the token then finds a subscription for the
//! group key.

use hiroz_protocol::{KeyExprFormat, qos::QosDurability};
use tracing::debug;
use zenoh::{Result, Wait, key_expr::KeyExpr, liveliness::LivelinessToken};

use crate::entity::EndpointEntity;

/// Whether a subscriber joins rmw_zenoh's CPU buffer group.
///
/// Set it with `with_buffer_group` on the subscriber or cache builder. The default is
/// [`BufferGroup::Auto`].
///
/// A joined subscriber declares two exact subscriptions, one on `<topic key>` and one on the group
/// key `<topic key>/_buf_cpu`, and its liveliness token announces the group. It receives no other
/// key under the topic key.
///
/// Router access control matters for the second key. A rule that allows `<topic key>` and not
/// `<topic key>/_buf_cpu` starves the group key and raises no error. A buffer-aware publisher
/// writes a buffer message on the group key only. Allow `<topic key>/_buf_cpu` wherever you allow
/// `<topic key>`, or choose [`BufferGroup::Off`].
///
/// A subscriber on a key format other than `rmw_zenoh`'s never joins, whatever the setting. A
/// transient-local subscriber never joins either, because the released `rmw_zenoh_cpp` serves its
/// history on the base key only. The interoperability chapter of the user guide gives the details.
///
/// New values may be added in a minor release, so match it with a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferGroup {
    /// Join when the message may carry a buffer: the message definition has an unbounded
    /// `uint8[]` or `char[]`, directly or nested, or hiroz has no definition to ask. The typed
    /// join also counts an unbounded `byte[]`, which `rmw_zenoh_cpp` does not: one extra join, no
    /// data lost. This is the default.
    #[default]
    Auto,
    /// Join, whatever the message is, except for a transient-local subscriber or a key format other
    /// than `rmw_zenoh`'s. Use it when the publisher counts a field that the rule for `Auto` does
    /// not count.
    Cpu,
    /// Do not join.
    Off,
}

/// Whether a subscriber joins. `has_buffer_fields` is `None` when hiroz has no message definition.
///
/// Only the `rmw_zenoh` key format has the group. A transient-local subscriber stays out: the
/// released `rmw_zenoh_cpp` serves its history on the base key only.
pub(crate) fn joins(
    group: BufferGroup,
    has_buffer_fields: Option<bool>,
    format: &KeyExprFormat,
    entity: &EndpointEntity,
) -> bool {
    let wanted = match group {
        BufferGroup::Cpu => true,
        BufferGroup::Off => false,
        BufferGroup::Auto => has_buffer_fields != Some(false),
    };
    wanted
        && matches!(format, KeyExprFormat::RmwZenoh)
        && entity.qos.durability != QosDurability::TransientLocal
}

/// The group key of a topic key. `rmw_zenoh_cpp` builds it in `make_cpu_group_key`.
pub(crate) fn group_key(base: &str) -> String {
    format!("{base}/_buf_cpu")
}

/// Declare the liveliness token of a subscriber. The token of a joined subscriber ends in
/// `backends:cpu:`. Call this after the subscriptions are declared: a publisher that reads the
/// token must find a subscription for the group key.
pub(crate) fn declare_token(
    session: &zenoh::Session,
    format: &KeyExprFormat,
    entity: &EndpointEntity,
    join: bool,
) -> Result<LivelinessToken> {
    let mut token_key = format
        .liveliness_key_expr(entity, &session.zid())?
        .as_str()
        .to_owned();
    if join {
        token_key.push_str("/backends:cpu:");
    }
    session
        .liveliness()
        .declare_token(KeyExpr::try_from(token_key)?)
        .wait()
}

/// Log one join, so an operator can find the two keys that a router rule must allow.
pub(crate) fn log_join(base: &str, group: &str) {
    debug!(
        "joins the rmw_zenoh CPU buffer group: subscribes to keys {base} and {group}. A router \
         rule must allow both keys."
    );
}
