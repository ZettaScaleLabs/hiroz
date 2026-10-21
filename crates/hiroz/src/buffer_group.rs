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
/// transient-local subscriber never joins either, because nothing shows that its history reaches it
/// on the group key. The interoperability chapter of the user guide gives the details.
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
/// Only the `rmw_zenoh` key format has the group, so no other format joins. A transient-local
/// subscriber does not join, because nothing shows that its history reaches it on the group key.
/// The function logs the decision at debug level, for a join and for a subscriber that stays out.
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
    let join = wanted
        && matches!(format, KeyExprFormat::RmwZenoh)
        && entity.qos.durability != QosDurability::TransientLocal;
    debug!(
        "[BUF] cpu group: topic={} join={join} (group={group:?}, buffers={has_buffer_fields:?})",
        entity.topic
    );
    join
}

/// The join decision of one subscriber build, kept for the two steps that need it. The build
/// declares the subscriptions first, and they need the group key. It declares the token last, and
/// the token needs the same answer. `declare` also holds the token code that the two build paths,
/// `build_internal` and `build_raw_subscriber`, share. Computing a plan declares nothing.
pub(crate) struct Plan {
    token_key: String,
    /// The topic key and the group key, when joining.
    keys: Option<(String, String)>,
}

impl Plan {
    /// Read the topic key from `entity`, so the caller must have qualified the topic first.
    pub(crate) fn new(
        format: &KeyExprFormat,
        entity: &EndpointEntity,
        zid: &zenoh::session::ZenohId,
        join: bool,
    ) -> Result<Self> {
        let mut token_key = format.liveliness_key_expr(entity, zid)?.as_str().to_owned();
        let keys = if join {
            // The token of a CPU-only subscriber ends in `backends:cpu:`.
            token_key.push_str("/backends:cpu:");
            let base = format.topic_key_expr(entity)?.as_str().to_owned();
            // The group key is the base key and `_buf_cpu`, the key `rmw_zenoh_cpp` builds in
            // `make_cpu_group_key`.
            let group = format!("{base}/_buf_cpu");
            Some((base, group))
        } else {
            None
        };
        Ok(Self { token_key, keys })
    }

    /// The group key, when the subscriber joins.
    pub(crate) fn group_key(&self) -> Option<&str> {
        self.keys.as_ref().map(|(_, group)| group.as_str())
    }

    /// Declare the token, then log the join. A declaration that fails logs nothing. The caller has
    /// already declared both subscriptions, so a publisher that reads the token finds a
    /// subscription for the group key.
    pub(crate) fn declare(self, session: &zenoh::Session) -> Result<LivelinessToken> {
        let token = session
            .liveliness()
            .declare_token(KeyExpr::try_from(self.token_key)?)
            .wait()?;
        if let Some((base, group)) = &self.keys {
            log_join(base, group);
        }
        Ok(token)
    }
}

/// Log one join. An operator learns here that the subscription needs an access-control rule: a
/// router that denies the group key drops buffer messages and reports no error.
fn log_join(base: &str, group: &str) {
    debug!(
        "joined the rmw_zenoh CPU buffer group: subscribed to keys {base} and {group}. A router \
         access-control rule that denies {group} drops buffer messages and reports no error. Allow \
         {group} wherever you allow {base}."
    );
}
