//! Subscriber side of rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base topic key to a
//! subscriber whose liveliness token has no `backends:` field. It sends the
//! sample, as plain CDR, on `<base>/_buf_cpu` to a subscriber whose token ends
//! in `backends:cpu:`. A subscriber that joins the group does both.

use std::sync::Arc;

use hiroz_protocol::{KeyExprFormat, qos::QosDurability};
use tracing::debug;
use zenoh::{
    Result, Wait,
    key_expr::KeyExpr,
    liveliness::LivelinessToken,
    pubsub::Subscriber,
    sample::{Locality, Sample},
};

use crate::entity::EndpointEntity;

/// What a subscriber holds for the group.
///
/// A struct drops its fields in declaration order. Declare this first in the
/// owner, and the token first in here: publishers then stop sending before the
/// subscriptions go away.
pub(crate) struct CpuGroup {
    _token: Option<LivelinessToken>,
    _sub: Option<Subscriber<()>>,
}

/// Declare the liveliness token and, when joining, the subscription on the group key.
///
/// `wanted` is the caller's choice, else what the message definition says; unknown does not
/// join. Only the `rmw_zenoh` key format has the group, and `rmw_zenoh` serves transient-local
/// history on the base key only. A joined subscriber announces `backends:cpu:` in its token and
/// also reads `<base>/_buf_cpu`, with the locality of the base subscription. Without a join the
/// token is the plain one when `token_required` is set (typed subscribers), else none (raw ones).
pub(crate) fn declare<F>(
    session: &zenoh::Session,
    format: &KeyExprFormat,
    entity: &EndpointEntity,
    locality: Option<Locality>,
    wanted: Option<bool>,
    token_required: bool,
    handler: Arc<F>,
) -> Result<CpuGroup>
where
    F: Fn(Sample) + Send + Sync + 'static,
{
    let join = joins(format, entity, wanted);
    if !join && !token_required {
        return Ok(CpuGroup {
            _token: None,
            _sub: None,
        });
    }
    let mut token_key = format
        .liveliness_key_expr(entity, &session.zid())?
        .as_str()
        .to_owned();
    let sub = if join {
        token_key.push_str("/backends:cpu:");
        let group_key = format!("{}/_buf_cpu", format.topic_key_expr(entity)?.as_str());
        let mut sub = session
            .declare_subscriber(KeyExpr::try_from(group_key)?)
            .callback(move |sample| handler(sample));
        if let Some(locality) = locality {
            sub = sub.allowed_origin(locality);
        }
        Some(sub.wait()?)
    } else {
        None
    };
    let token = session
        .liveliness()
        .declare_token(KeyExpr::try_from(token_key)?)
        .wait()?;
    Ok(CpuGroup {
        _token: Some(token),
        _sub: sub,
    })
}

fn joins(format: &KeyExprFormat, entity: &EndpointEntity, wanted: Option<bool>) -> bool {
    let join = wanted == Some(true)
        && matches!(format, KeyExprFormat::RmwZenoh)
        && entity.qos.durability != QosDurability::TransientLocal;
    debug!("[BUF] cpu group: topic={} join={join} (wanted={wanted:?})", entity.topic);
    join
}

#[cfg(test)]
mod tests {
    use super::*;

    use hiroz_protocol::{KeyExprFormatterAdapter, RmwZenohFormatter};

    use crate::entity::EndpointKind;

    fn entity(durability: QosDurability) -> EndpointEntity {
        let mut entity = EndpointEntity {
            id: 0,
            node: None,
            kind: EndpointKind::Subscription,
            topic: "/t".to_string(),
            type_info: None,
            qos: Default::default(),
        };
        entity.qos.durability = durability;
        entity
    }

    /// Pins: only a wanted join on rmw_zenoh with a volatile subscriber joins. Unknown stays out,
    /// and even a wanted join is ignored for transient-local and for a custom format.
    #[test]
    fn joins_only_when_wanted_on_rmw_zenoh_and_not_transient_local() {
        let volatile = entity(QosDurability::Volatile);
        let rmw = &KeyExprFormat::RmwZenoh;
        assert!(joins(rmw, &volatile, Some(true)));
        assert!(!joins(rmw, &volatile, Some(false)));
        assert!(!joins(rmw, &volatile, None));
        assert!(!joins(rmw, &entity(QosDurability::TransientLocal), Some(true)));
        let custom = KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<RmwZenohFormatter>::new()));
        assert!(!joins(&custom, &volatile, Some(true)));
    }
}
