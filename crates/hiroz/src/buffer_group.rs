//! Subscriber side of rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base topic key to a subscriber whose
//! liveliness token has no `backends:` field. It sends the sample, as plain CDR, on
//! `<base>/_buf_cpu` to a subscriber whose token ends in `backends:cpu:`. A subscriber that joins
//! the group does both.
//!
//! A joined subscriber has one subscription, on `<base>/**`. It reads the base key and the group
//! key, and drops every other key under the base in process. It declares no second subscription.

use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

use hiroz_protocol::{KeyExprFormat, qos::QosDurability};
use tracing::{debug, info};
use zenoh::{Result, Wait, key_expr::KeyExpr, liveliness::LivelinessToken, sample::Sample};

use crate::entity::EndpointEntity;

/// Whether a subscriber joins rmw_zenoh's CPU buffer group.
///
/// Set it with `with_buffer_group` on the subscriber or cache builder. The default is
/// [`BufferGroup::Auto`].
///
/// A joined subscriber declares one subscription on `<topic key>/**` in place of `<topic key>`. It
/// reads the topic key and the group key `<topic key>/_buf_cpu`, and its liveliness token announces
/// the group. Other keys under the topic key still reach the process and are discarded there.
///
/// The wider key matters to router access control. A rule that allows `<topic key>` and not
/// `<topic key>/**` starves a joined subscriber and raises no error. Allow `<topic key>/**` wherever
/// you allow `<topic key>`, or choose [`BufferGroup::Off`].
///
/// A transient-local subscriber never joins, and neither does a subscriber on a key format other
/// than `rmw_zenoh`'s, whatever the setting. The interoperability chapter of the user guide gives
/// the details.
///
/// New values may be added in a minor release, so match it with a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferGroup {
    /// Join when the message may carry a buffer: the message definition has an unbounded
    /// `uint8[]`, directly or nested, or hiroz has no definition to ask. This is the default.
    #[default]
    Auto,
    /// Join, whatever the message is, except for a transient-local subscriber or a key format other
    /// than `rmw_zenoh`'s.
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
#[doc(hidden)]
pub struct Plan {
    token_key: String,
    /// The topic key and the group key, when joining.
    keys: Option<(String, String)>,
}

impl Plan {
    pub fn new(
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

    /// Declare the token, then log the join. A declaration that fails logs nothing and leaves the
    /// key's `info` line for a later join. The subscriber's own subscription, on `<base>/**`,
    /// already serves the group key, so a publisher that reads the token finds a subscription.
    pub fn declare(self, session: &zenoh::Session) -> Result<LivelinessToken> {
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

/// The most group keys the process-wide join log remembers.
#[doc(hidden)]
pub const LOG_KEYS: usize = 1024;

/// What the join log does for one join.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinLogLevel {
    /// The key is new and the set has room: log at `info` level.
    Info,
    /// The key is already logged: log nothing.
    Skip,
    /// The key is new and the set is full: log at `debug` level and keep nothing.
    Debug,
}

/// The decision state of the join log: at most `cap` group keys, each logged once at `info` level.
/// A key past the cap is logged at `debug` level on every join and never stored, so the set stays
/// bounded. It does no logging and holds no lock. The process uses one instance, behind its
/// join log.
#[doc(hidden)]
#[derive(Debug)]
pub struct JoinLog {
    cap: usize,
    keys: HashSet<String>,
}

impl JoinLog {
    /// A log that remembers at most `cap` keys.
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            keys: HashSet::new(),
        }
    }

    /// Record one join of `group` and say how to log it.
    pub fn record(&mut self, group: &str) -> JoinLogLevel {
        if self.keys.len() < self.cap {
            if self.keys.insert(group.to_owned()) {
                JoinLogLevel::Info
            } else {
                JoinLogLevel::Skip
            }
        } else if self.keys.contains(group) {
            JoinLogLevel::Skip
        } else {
            JoinLogLevel::Debug
        }
    }

    /// How many keys the log holds now.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the log holds no key.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// An operator learns here that the subscription needs an access-control rule: a router that
/// denies `<base>/**` or the group key drops buffer messages and reports no error. The process
/// logs one line per group key for the first [`LOG_KEYS`] keys, and a key past the limit at `debug`
/// level on every join.
fn log_join(base: &str, group: &str) {
    static LOGGED: OnceLock<Mutex<JoinLog>> = OnceLock::new();
    let level = LOGGED
        .get_or_init(|| Mutex::new(JoinLog::new(LOG_KEYS)))
        .lock()
        .unwrap()
        .record(group);
    let line = || {
        format!(
            "joined the rmw_zenoh CPU buffer group: subscribed to key {base}/**, which includes \
             key {group}. A router access-control rule that denies {base}/** or {group} drops \
             buffer messages and reports no error. Allow {base}/** wherever you allow {base}."
        )
    };
    match level {
        JoinLogLevel::Info => info!("{}", line()),
        JoinLogLevel::Skip => {}
        JoinLogLevel::Debug => debug!("{}", line()),
    }
}
