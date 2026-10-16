//! Subscriber side of rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base topic key to a subscriber whose
//! liveliness token has no `backends:` field. It sends the sample, as plain CDR, on
//! `<base>/_buf_cpu` to a subscriber whose token ends in `backends:cpu:`. A subscriber that joins
//! the group does both.
//!
//! A joined subscriber has one subscription, on `<base>/**`. It reads the base key and the group
//! key, and drops every other key under the base in process. It declares no second subscription,
//! because the declaration time of a session grows with the square of its subscription count.

use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

use hiroz_protocol::{KeyExprFormat, qos::QosDurability};
use tracing::{debug, info};
use zenoh::{
    Result, Wait,
    key_expr::KeyExpr,
    liveliness::LivelinessToken,
    sample::Sample,
};

use crate::entity::EndpointEntity;

/// Whether a subscriber joins rmw_zenoh's CPU buffer group.
///
/// Set it with `with_buffer_group` on the subscriber builder.
///
/// New values may be added in a minor release, so match it with a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferGroup {
    /// Join when the message may carry a buffer: the message definition has an unbounded
    /// `uint8[]`, directly or nested, or hiroz has no definition to ask. This is the default.
    #[default]
    Auto,
    /// Join, whatever the message is.
    Cpu,
    /// Do not join.
    Off,
}

/// Whether a subscriber joins. `has_buffer_fields` is `None` when hiroz has no message definition.
///
/// Only the `rmw_zenoh` key format has the group, and `rmw_zenoh` serves transient-local history
/// on the base key only, so neither joins whatever the caller asks.
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

/// What a subscriber will declare: the token string and, when joining, the group key. Computing
/// it declares nothing.
pub(crate) struct Plan {
    token_key: String,
    /// The topic key and the group key, when joining.
    keys: Option<(String, String)>,
}

impl Plan {
    pub(crate) fn new(
        format: &KeyExprFormat,
        entity: &EndpointEntity,
        zid: &zenoh::session::ZenohId,
        join: bool,
    ) -> Result<Self> {
        let mut token_key = format.liveliness_key_expr(entity, zid)?.as_str().to_owned();
        let keys = if join {
            token_key.push_str("/backends:cpu:");
            let base = format.topic_key_expr(entity)?.as_str().to_owned();
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

    /// Declare the token. The subscriber's own subscription, on `<base>/**`, already serves the
    /// group key, so a publisher that reads the token finds a subscription.
    pub(crate) fn declare(self, session: &zenoh::Session) -> Result<Membership> {
        if let Some((base, group)) = &self.keys {
            log_once(base, group);
        }
        let token = session
            .liveliness()
            .declare_token(KeyExpr::try_from(self.token_key)?)
            .wait()?;
        Ok(Membership { token: Some(token) })
    }
}

/// The key a joined subscriber subscribes on: its base key and everything under it. It reads the
/// base key and the group key, and drops the rest in process.
pub(crate) fn union_key(base: &str) -> Result<KeyExpr<'static>> {
    KeyExpr::try_from(format!("{base}/**"))
}

/// Wrap `handler` so that it sees the base key and the group key only.
pub(crate) fn only_base_and_group<F>(
    base: String,
    group_key: String,
    handler: F,
) -> impl Fn(Sample) + Send + Sync + 'static
where
    F: Fn(Sample) + Send + Sync + 'static,
{
    move |sample: Sample| {
        let key = sample.key_expr().as_str();
        if key == base || key == group_key {
            handler(sample);
        }
    }
}

/// What a subscriber holds for the group. Drop it before anything else the subscriber owns.
pub(crate) struct Membership {
    token: Option<LivelinessToken>,
}

impl Drop for Membership {
    /// Undeclare the token first, so publishers stop sending to the group key before the
    /// subscription that serves it goes away.
    fn drop(&mut self) {
        drop(self.token.take());
    }
}

/// An operator learns here that the subscription and the group key need access-control rules: a
/// router that denies either key drops buffer messages and reports no error. One line per group
/// key and process.
///
/// `LOGGED` holds one string per distinct group key, never more: `insert` ignores a key it
/// already holds, and nothing else adds to the set. A group key is `<topic key>/_buf_cpu`, so the
/// set is as large as the number of distinct topic keys that joined in this process, and each
/// entry is no longer than its key. A process that joins an unbounded number of distinct topic
/// names grows the set with it. The set is not pruned, because a pruned key would log a second
/// time.
fn log_once(base: &str, group: &str) {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let first = LOGGED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(group.to_owned());
    if first {
        info!(
            "joined the rmw_zenoh CPU buffer group: subscribed to key {base}/**, which includes \
             key {group}. A router access-control rule that denies either key drops buffer \
             messages and reports no error. Allow both keys wherever you allow its topic key."
        );
    }
}
