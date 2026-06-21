//! Message forwarding between Humble and Jazzy Zenoh sessions.
//!
//! # Pub/Sub forwarding
//! Raw CDR bytes are forwarded verbatim between sessions with the key expression
//! hash segment rewritten for the target distro.
//!
//! # Service forwarding
//! Zenoh services use queryables (server) and get() (client). The bridge
//! declares a queryable on one side and re-issues queries on the other.

use anyhow::Result;
use std::sync::Arc;
use zenoh::{Session, Wait, bytes::ZBytes};

/// Attachment key used to mark bridge-forwarded messages.
///
/// When a forwarder puts a message it attaches this marker.  Any forwarder that
/// sees the marker on an incoming sample drops it immediately, preventing the
/// two bidirectional forwarders (h2j and j2h) from feeding each other in an
/// infinite feedback loop via the shared router.
const BRIDGE_MARKER_KEY: &[u8] = b"_hiroz_bridge";

/// A running pub/sub forwarder for a single topic pair.
///
/// Dropping this handle stops the forwarder (subscriber is dropped).
pub struct ForwarderHandle {
    /// Keep the subscriber alive.
    _subscriber: zenoh::pubsub::Subscriber<()>,
}

/// Start forwarding messages from `src_session` key expression `src_ke`
/// to `dst_session` using key expression `dst_ke`.
///
/// The payload bytes are forwarded verbatim (CDR serialisation is identical
/// between Humble and Jazzy for the same message type).
///
/// # Loop prevention
///
/// Forwarded messages carry a [`BRIDGE_MARKER_KEY`] attachment.  The subscriber
/// callback drops any incoming sample that already carries the marker so that
/// the h2j and j2h forwarders do not feed each other.
pub fn start_forwarder(
    src_session: Arc<Session>,
    src_ke: String,
    dst_session: Arc<Session>,
    dst_ke: String,
) -> Result<ForwarderHandle> {
    // Use Arc<str> so the closure captures a reference-counted pointer rather
    // than cloning the full String on every message.
    let src_ke_log: Arc<str> = src_ke.as_str().into();
    let dst_ke_arc: Arc<str> = dst_ke.as_str().into();

    let sub = src_session
        .declare_subscriber(src_ke.clone())
        .callback({
            move |sample| {
                // Drop messages that were already forwarded by the bridge to
                // prevent h2j ↔ j2h feedback loops through the shared router.
                if sample
                    .attachment()
                    .map(|a| a.to_bytes().as_ref() == BRIDGE_MARKER_KEY)
                    .unwrap_or(false)
                {
                    return;
                }

                let payload: ZBytes = sample.payload().clone();
                let session = dst_session.clone();
                let ke = dst_ke_arc.clone();
                let src = src_ke_log.clone();
                tracing::debug!("forwarder {src}: received message, forwarding to {ke}");
                tokio::spawn(async move {
                    let result = session
                        .put(ke.as_ref(), payload)
                        .attachment(BRIDGE_MARKER_KEY)
                        .await;
                    if let Err(e) = result {
                        tracing::warn!("forwarder {src} → {ke}: put failed: {e}");
                    } else {
                        tracing::debug!("forwarder → {ke}: put ok");
                    }
                });
            }
        })
        .wait()
        .map_err(|e| anyhow::anyhow!("declare_subscriber {src_ke}: {e}"))?;

    Ok(ForwarderHandle { _subscriber: sub })
}

/// A running service forwarder.
///
/// Intercepts queries on `proxy_ke` and re-issues them on `target_ke` in the
/// other session, then forwards the reply back to the original querier.
///
/// Dropping this handle stops the forwarder.
pub struct ServiceForwarderHandle {
    _queryable: zenoh::query::Queryable<()>,
}

/// Start a service forwarder: declare a queryable on `proxy_session` at
/// `proxy_ke`, and for each query re-issue it via `target_session` at
/// `target_ke`, forwarding the first reply back.
pub fn start_service_forwarder(
    proxy_session: Arc<Session>,
    proxy_ke: String,
    target_session: Arc<Session>,
    target_ke: String,
) -> Result<ServiceForwarderHandle> {
    let target_ke_arc: Arc<str> = target_ke.as_str().into();
    let proxy_ke_log: Arc<str> = proxy_ke.as_str().into();

    let queryable = proxy_session
        .declare_queryable(proxy_ke.clone())
        .complete(true)
        .callback({
            let target_session = target_session.clone();
            move |query| {
                let target_session = target_session.clone();
                let target_ke = target_ke_arc.clone();
                let proxy_ke_log = proxy_ke_log.clone();
                // Extract payload and attachment from the query.
                // The attachment carries the request identity (sequence number, writer GUID)
                // required by hiroz service.rs — it must be forwarded verbatim.
                let payload: Option<ZBytes> = query.payload().cloned();
                let attachment: Option<ZBytes> = query.attachment().cloned();
                tracing::debug!(
                    "service forwarder {proxy_ke_log}: query received, forwarding to {target_ke}"
                );
                tokio::spawn(async move {
                    let mut get = target_session.get(target_ke.as_ref());
                    if let Some(p) = payload {
                        get = get.payload(p);
                    }
                    if let Some(a) = attachment {
                        get = get.attachment(a);
                    }
                    let replies = match get.await {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(
                                "service forwarder {proxy_ke_log} → {target_ke}: get failed: {e}"
                            );
                            return;
                        }
                    };
                    tracing::debug!(
                        "service forwarder {proxy_ke_log} → {target_ke}: waiting for reply"
                    );
                    // Forward the first reply back to the original querier.
                    // Use the query's own KE (Jazzy KE) so the reply routes
                    // back correctly — Zenoh requires the reply KE to intersect
                    // the original queried KE, not the target KE.
                    let reply_ke = query.key_expr().to_string();
                    while let Ok(reply) = replies.recv_async().await {
                        match reply.result() {
                            Ok(sample) => {
                                let payload = sample.payload().clone();
                                tracing::debug!(
                                    "service forwarder {proxy_ke_log}: got reply, sending back"
                                );
                                if let Err(e) = query.reply(&reply_ke, payload).await {
                                    tracing::warn!(
                                        "service forwarder reply failed: {e}"
                                    );
                                }
                                break;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "service forwarder {proxy_ke_log} → {target_ke}: reply error: {e:?}"
                                );
                            }
                        }
                    }
                });
            }
        })
        .wait()
        .map_err(|e| anyhow::anyhow!("declare_queryable {proxy_ke}: {e}"))?;

    Ok(ServiceForwarderHandle {
        _queryable: queryable,
    })
}
