//! Subscriber side of rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh_cpp` publisher writes a message with a buffer field on
//! `<base>/_buf_cpu`, for each subscriber whose liveliness token ends in `backends:cpu:`. A
//! publisher that is not buffer-aware writes `<base>` only. A joined subscriber therefore has two
//! exact subscriptions, on `<base>` and on `<base>/_buf_cpu`, as `rmw_zenoh_cpp` has. It has no
//! other subscription under `<base>`, so the samples that a publisher writes for other subscribers
//! never arrive. It declares both subscriptions before the token.

use hiroz_protocol::{KeyExprFormat, qos::QosDurability};
use tracing::debug;
use zenoh::{Result, Wait, key_expr::KeyExpr, liveliness::LivelinessToken};

use crate::entity::EndpointEntity;

/// Whether a subscriber joins rmw_zenoh's CPU buffer group.
///
/// Set it with `with_buffer_group` on the subscriber or cache builder. The default is
/// [`BufferGroup::Auto`].
///
/// A joined subscriber subscribes to `<topic key>` and `<topic key>/_buf_cpu`, and its liveliness
/// token announces the group. A router rule that allows `<topic key>` and not
/// `<topic key>/_buf_cpu` stops a joined subscriber from receiving buffer messages, without an
/// error. Allow both keys.
///
/// [`BufferGroup::Off`] stops a subscriber from joining, so it needs no rule for the group key. A
/// subscriber with no `backends:` field receives no buffer message from a buffer-aware publisher
/// until a release of `rmw_zenoh_cpp` carries the fix that the user guide names. `Off` therefore
/// does not bring those messages back.
///
/// A subscriber on a key format other than `rmw_zenoh`'s never joins. A transient-local subscriber
/// never joins: the released `rmw_zenoh_cpp` serves its history on the base key only. The
/// interoperability chapter of the user guide gives the details.
///
/// Match this enum with a wildcard arm: a later release can add values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferGroup {
    /// Join when the message can carry a buffer: an unbounded `uint8[]` or `char[]`, directly or
    /// nested, or hiroz has no message definition. A typed message also counts an unbounded
    /// `byte[]`, which `rmw_zenoh_cpp` does not: one extra join, no data lost. This is the default. A raw subscriber has no definition, so it joins.
    #[default]
    Auto,
    /// Join whatever the message is, except for a transient-local subscriber or a key format other
    /// than `rmw_zenoh`'s. Use it when the publisher counts a field that `Auto` does not.
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
        base,
        group, "joins the rmw_zenoh CPU buffer group; a router rule must allow both keys"
    );
}
