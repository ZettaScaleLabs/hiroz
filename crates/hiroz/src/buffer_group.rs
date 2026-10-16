//! Subscriber side of rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base topic key to a subscriber whose
//! liveliness token has no `backends:` field. It sends the sample, as plain CDR, on
//! `<base>/_buf_cpu` to a subscriber whose token ends in `backends:cpu:`. A subscriber that joins
//! the group does both.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

use hiroz_protocol::{KeyExprFormat, qos::QosDurability};
use tracing::{debug, info};
use zenoh::{
    Result, Wait,
    key_expr::KeyExpr,
    liveliness::LivelinessToken,
    pubsub::Subscriber,
    sample::{Locality, Sample},
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
    group_key: Option<String>,
}

impl Plan {
    pub(crate) fn new(
        format: &KeyExprFormat,
        entity: &EndpointEntity,
        zid: &zenoh::session::ZenohId,
        join: bool,
    ) -> Result<Self> {
        let mut token_key = format.liveliness_key_expr(entity, zid)?.as_str().to_owned();
        let group_key = if join {
            token_key.push_str("/backends:cpu:");
            Some(format!("{}/_buf_cpu", format.topic_key_expr(entity)?.as_str()))
        } else {
            None
        };
        Ok(Self {
            token_key,
            group_key,
        })
    }

    /// Declare the group subscription, then the token. A publisher reads the token to decide
    /// where to send, so the subscription must exist before the token is visible.
    pub(crate) fn declare<F>(
        self,
        session: &zenoh::Session,
        locality: Option<Locality>,
        handler: Arc<F>,
    ) -> Result<Membership>
    where
        F: Fn(Sample) + Send + Sync + 'static,
    {
        let sub = match self.group_key {
            Some(key) => {
                let mut builder = session
                    .declare_subscriber(KeyExpr::try_from(key.clone())?)
                    .callback(move |sample| handler(sample));
                if let Some(locality) = locality {
                    builder = builder.allowed_origin(locality);
                }
                let sub = builder.wait()?;
                log_once(&key);
                Some(sub)
            }
            None => None,
        };
        let token = session
            .liveliness()
            .declare_token(KeyExpr::try_from(self.token_key)?)
            .wait()?;
        Ok(Membership {
            token: Some(token),
            _sub: sub,
        })
    }
}

/// What a subscriber holds for the group. Drop it before anything else the subscriber owns.
pub(crate) struct Membership {
    token: Option<LivelinessToken>,
    _sub: Option<Subscriber<()>>,
}

impl Drop for Membership {
    /// Undeclare the token first, so publishers stop sending to the group key before the
    /// subscription on it goes away.
    fn drop(&mut self) {
        drop(self.token.take());
    }
}

/// An operator learns here that the group key needs an access-control rule: a router that
/// denies the key drops buffer messages and reports no error. One line per key and process.
fn log_once(key: &str) {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let first = LOGGED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(key.to_owned());
    if first {
        info!(
            "joined the rmw_zenoh CPU buffer group: subscribed to key {key}. A router \
             access-control rule that denies this key drops buffer messages and reports no \
             error. Allow this key wherever you allow its topic key."
        );
    }
}
