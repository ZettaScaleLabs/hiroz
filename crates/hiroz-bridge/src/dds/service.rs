//! ZDdsServiceBridge (DDS server→Zenoh queryable) and ZDdsClientBridge (DDS client→Zenoh querier).

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow};
use hiroz::node::ZNode;
use hiroz_protocol::entity::{EndpointEntity, EndpointKind, TypeHash, TypeInfo};
use parking_lot::Mutex;
use zenoh::{
    Wait,
    bytes::ZBytes,
    query::{Query, Queryable},
};

use super::{
    gid::Gid,
    names::{ros2_name_to_dds_reply_topic, ros2_name_to_dds_request_topic, ros2_type_to_dds_type},
    participant::{BridgeQos, DdsParticipant, DdsReader, DdsWriter},
    qos::{adapt_reader_qos_for_writer, adapt_writer_qos_for_reader, bridge_qos_to_qos_profile},
};

// ─── CDR helpers ─────────────────────────────────────────────────────────────

const CDR_HEADER_LE: [u8; 4] = [0, 1, 0, 0];
const CDR_HEADER_BE: [u8; 4] = [0, 0, 0, 0];

fn cdr_header_matching(payload: &[u8]) -> [u8; 4] {
    if payload.get(1).copied().unwrap_or(1) == 1 {
        CDR_HEADER_LE
    } else {
        CDR_HEADER_BE
    }
}

/// Sequence number embedded in every ROS 2 CDR service payload.
///
/// Layout: [4-byte CDR header] [8-byte client_guid] [8-byte seq_num LE] [payload]
fn extract_sequence_number(raw: &[u8]) -> Option<u64> {
    if raw.len() < 20 {
        return None;
    }
    Some(u64::from_le_bytes(raw[12..20].try_into().unwrap()))
}

/// Derive the DDS reply type name from the request type name.
///
/// `example_interfaces::srv::dds_::AddTwoInts_Request_` → `…_Response_`
fn reply_type_from_request_type(req_type: &str, ros2_type: &str) -> String {
    if let Some(base) = req_type.strip_suffix("_Request_") {
        format!("{base}_Response_")
    } else {
        let dds_base = ros2_type.replace('/', "::");
        format!("{dds_base}_Response_")
    }
}

// ─── ZDdsServiceBridge ────────────────────────────────────────────────────────

/// Routes a DDS service server to a Zenoh queryable.
///
/// A Zenoh queryable is declared with `.complete(true)` so clients across a router
/// topology see this bridge as a complete service provider.  Each Zenoh query is
/// translated to a DDS request; the matching DDS reply (matched by sequence number
/// from the CDR header) is forwarded back as the query reply.
pub struct ZDdsServiceBridge<P: DdsParticipant> {
    _req_writer: Arc<P::Writer>,
    _rep_reader: P::Reader,
    _queryable: Queryable<()>,
}

unsafe impl<P: DdsParticipant> Send for ZDdsServiceBridge<P> {}
unsafe impl<P: DdsParticipant> Sync for ZDdsServiceBridge<P> {}

impl<P: DdsParticipant> ZDdsServiceBridge<P> {
    /// Create a new DDS-server → Zenoh queryable bridge.
    ///
    /// The Zenoh queryable is declared on a wildcard key (`{domain}/{service}/**`) so
    /// that clients using any type hash — or none at all — can reach it transparently.
    ///
    /// - `ros2_name` — ROS 2 service name (e.g. `/add_two_ints`)
    /// - `ros2_type` — ROS 2 service type (e.g. `example_interfaces/srv/AddTwoInts`)
    /// - `qos` — DDS QoS for the request writer / reply reader
    pub async fn new(
        node: &ZNode,
        ros2_name: &str,
        ros2_type: &str,
        participant: &P,
        qos: BridgeQos,
    ) -> Result<Self> {
        // Routing entity: no type_info so topic_key_expr returns the EMPTY placeholder key.
        // We then widen it to a wildcard so any Zenoh client can reach this bridge queryable
        // regardless of the type hash embedded in the client's query key expression.
        let entity = EndpointEntity {
            id: node.next_entity_id(),
            node: Some(node.node_entity().clone()),
            kind: EndpointKind::Service,
            topic: ros2_name.to_string(),
            type_info: None,
            qos: bridge_qos_to_qos_profile(&qos),
        };
        let topic_ke = node
            .keyexpr_format()
            .topic_key_expr(&entity)
            .map_err(|e| anyhow!("topic_key_expr failed: {e}"))?;

        // Convert "…/EMPTY_TOPIC_TYPE/EMPTY_TOPIC_HASH" → "…/**" for rmw-zenoh keys so
        // the queryable responds to clients regardless of their type/hash.  For ros2dds
        // keys (which carry no type/hash) the key is used as-is.
        let ke_str = topic_ke.as_str();
        let ke_str = ke_str
            .strip_suffix("/EMPTY_TOPIC_TYPE/EMPTY_TOPIC_HASH")
            .map(|prefix| format!("{prefix}/**"))
            .unwrap_or_else(|| ke_str.to_string());
        let ke: zenoh::key_expr::OwnedKeyExpr = ke_str
            .try_into()
            .map_err(|e| anyhow!("invalid key expr: {e}"))?;

        let dds_type = ros2_type_to_dds_type(ros2_type);
        let req_type = format!("{}_Request_", dds_type.trim_end_matches('_'));
        let rep_type = reply_type_from_request_type(&req_type, ros2_type);
        let req_topic = ros2_name_to_dds_request_topic(ros2_name);
        let rep_topic = ros2_name_to_dds_reply_topic(ros2_name);
        let req_writer_qos = adapt_reader_qos_for_writer(&qos);

        tracing::info!(
            "ZDdsServiceBridge: DDS {} → Zenoh {}",
            req_topic,
            ke.as_str()
        );

        let seq_counter = Arc::new(AtomicU64::new(0));
        let in_flight: Arc<Mutex<HashMap<u64, Query>>> = Arc::new(Mutex::new(HashMap::new()));

        let req_writer = Arc::new(participant.create_writer(
            &req_topic,
            &req_type,
            true,
            req_writer_qos.clone(),
        )?);
        let client_guid = req_writer.instance_handle()?;

        let in_flight_rep = Arc::clone(&in_flight);
        let rep_reader_qos = adapt_writer_qos_for_reader(&qos);
        let rep_reader = participant.create_reader(
            &rep_topic,
            &rep_type,
            true,
            rep_reader_qos,
            Box::new(move |bytes: Vec<u8>| {
                let raw = bytes.as_slice();
                let seq = match extract_sequence_number(raw) {
                    Some(s) => s,
                    None => {
                        tracing::warn!("ZDdsServiceBridge: reply too short ({} bytes)", raw.len());
                        return;
                    }
                };
                if let Some(query) = in_flight_rep.lock().remove(&seq) {
                    let zenoh_payload = if raw.len() >= 20 {
                        let mut v = Vec::with_capacity(4 + (raw.len() - 20));
                        v.extend_from_slice(&cdr_header_matching(raw));
                        v.extend_from_slice(&raw[20..]);
                        v
                    } else {
                        raw.to_vec()
                    };
                    let zbytes: ZBytes = zenoh_payload.into();
                    let ke = query.key_expr().clone();
                    if let Err(e) = query.reply(ke, zbytes).wait() {
                        tracing::warn!("ZDdsServiceBridge: query reply failed: {e}");
                    }
                } else {
                    tracing::debug!("ZDdsServiceBridge: no in-flight query for seq={seq}");
                }
            }),
        )?;

        let in_flight_q = Arc::clone(&in_flight);
        let seq_counter_q = Arc::clone(&seq_counter);
        let req_writer_q = Arc::clone(&req_writer);
        let ke_log = topic_ke.as_str().to_string();

        let queryable = node
            .session()
            .declare_queryable(ke.clone())
            .complete(true)
            .callback(move |query: Query| {
                tracing::debug!("ZDdsServiceBridge: query on {ke_log}");
                let seq = seq_counter_q.fetch_add(1, Ordering::Relaxed);

                let query_payload: Vec<u8> = match query.payload() {
                    Some(p) => p.to_bytes().into_owned(),
                    None => vec![],
                };
                let payload_body = if query_payload.len() >= 4 {
                    &query_payload[4..]
                } else {
                    query_payload.as_slice()
                };

                let cdr_hdr = cdr_header_matching(&query_payload);
                let mut dds_payload = Vec::with_capacity(4 + 16 + payload_body.len());
                dds_payload.extend_from_slice(&cdr_hdr);
                dds_payload.extend_from_slice(&client_guid.to_le_bytes());
                dds_payload.extend_from_slice(&seq.to_le_bytes());
                dds_payload.extend_from_slice(payload_body);

                in_flight_q.lock().insert(seq, query);

                if let Err(e) = req_writer_q.write_cdr(dds_payload) {
                    tracing::warn!("ZDdsServiceBridge: DDS write failed: {e}");
                    in_flight_q.lock().remove(&seq);
                }
            })
            .await
            .map_err(|e| anyhow!("declare_queryable failed: {e}"))?;

        Ok(Self {
            _req_writer: req_writer,
            _rep_reader: rep_reader,
            _queryable: queryable,
        })
    }

    /// Return the DDS request-writer GID (appears in `writer_gid_seq` in ros_discovery_info).
    pub fn writer_guid(&self) -> Option<Gid> {
        self._req_writer.guid().ok().map(Gid::from)
    }

    /// Return the DDS reply-reader GID (appears in `reader_gid_seq` in ros_discovery_info).
    pub fn reader_guid(&self) -> Option<Gid> {
        self._rep_reader.guid().ok().map(Gid::from)
    }
}

// ─── ZDdsClientBridge ────────────────────────────────────────────────────────

/// Request forwarded from the DDS reader callback to the async dispatch task.
struct PendingRequest {
    hdr: [u8; 16],
    payload: Vec<u8>,
}

/// Routes a DDS service client to a Zenoh querier.
///
/// A DDS reader watches the DDS request topic. Each incoming request is forwarded
/// as a Zenoh `get()` query to the service key expression. The reply is reconstructed
/// with the original 16-byte DDS header (client GUID + sequence number) and written
/// back to the DDS reply topic.
pub struct ZDdsClientBridge<P: DdsParticipant> {
    _req_reader: P::Reader,
    _rep_writer: Arc<P::Writer>,
}

unsafe impl<P: DdsParticipant> Send for ZDdsClientBridge<P> {}
unsafe impl<P: DdsParticipant> Sync for ZDdsClientBridge<P> {}

impl<P: DdsParticipant> ZDdsClientBridge<P> {
    /// Create a new DDS-client → Zenoh querier bridge.
    ///
    /// - `ros2_name` — ROS 2 service name (e.g. `/add_two_ints`)
    /// - `ros2_type` — ROS 2 service type (e.g. `example_interfaces/srv/AddTwoInts`)
    /// - `type_hash` — optional RIHS01 type hash
    /// - `qos` — DDS QoS for the reply writer / request reader
    /// - `timeout` — Zenoh querier timeout (use 10s for regular services, 300s for action get_result)
    pub async fn new(
        node: &ZNode,
        ros2_name: &str,
        ros2_type: &str,
        type_hash: Option<TypeHash>,
        participant: &P,
        qos: BridgeQos,
        timeout: Duration,
    ) -> Result<Self> {
        let type_info = type_hash.map(|h| TypeInfo::new(ros2_type, h));
        let entity = EndpointEntity {
            id: node.next_entity_id(),
            node: Some(node.node_entity().clone()),
            kind: EndpointKind::Client,
            topic: ros2_name.to_string(),
            type_info,
            qos: bridge_qos_to_qos_profile(&qos),
        };

        let topic_ke = node
            .keyexpr_format()
            .topic_key_expr(&entity)
            .map_err(|e| anyhow!("topic_key_expr failed: {e}"))?;

        let ke: zenoh::key_expr::OwnedKeyExpr = topic_ke
            .as_str()
            .to_owned()
            .try_into()
            .map_err(|e| anyhow!("invalid key expr: {e}"))?;

        let dds_type = ros2_type_to_dds_type(ros2_type);
        let req_type = format!("{}_Request_", dds_type.trim_end_matches('_'));
        let rep_type = reply_type_from_request_type(&req_type, ros2_type);
        let req_topic = ros2_name_to_dds_request_topic(ros2_name);
        let rep_topic = ros2_name_to_dds_reply_topic(ros2_name);
        let rep_writer_qos = adapt_reader_qos_for_writer(&qos);
        let req_reader_qos = adapt_reader_qos_for_writer(&qos);

        tracing::info!(
            "ZDdsClientBridge: Zenoh {} → DDS {}",
            topic_ke.as_str(),
            rep_topic
        );

        let rep_writer =
            Arc::new(participant.create_writer(&rep_topic, &rep_type, true, rep_writer_qos)?);

        let (tx, rx) = flume::bounded::<PendingRequest>(64);

        let querier = node
            .session()
            .declare_querier(ke)
            .timeout(timeout)
            .await
            .map_err(|e| anyhow!("declare_querier failed: {e}"))?;

        let writer_task = Arc::clone(&rep_writer);
        let rep_topic_log = rep_topic.clone();
        tokio::spawn(async move {
            while let Ok(req) = rx.recv_async().await {
                let zbytes: ZBytes = req.payload.into();
                let replies = match querier.get().payload(zbytes).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("ZDdsClientBridge: querier.get() failed: {e}");
                        continue;
                    }
                };

                for reply in replies {
                    let reply_bytes: Vec<u8> = match reply.result() {
                        Ok(sample) => sample.payload().to_bytes().into_owned(),
                        Err(e) => {
                            tracing::warn!("ZDdsClientBridge: reply error: {e}");
                            continue;
                        }
                    };
                    let reply_body = if reply_bytes.len() >= 4 {
                        &reply_bytes[4..]
                    } else {
                        reply_bytes.as_slice()
                    };
                    let mut dds_reply = Vec::with_capacity(4 + 16 + reply_body.len());
                    dds_reply.extend_from_slice(&cdr_header_matching(&reply_bytes));
                    dds_reply.extend_from_slice(&req.hdr);
                    dds_reply.extend_from_slice(reply_body);

                    if let Err(e) = writer_task.write_cdr(dds_reply) {
                        tracing::warn!(
                            "ZDdsClientBridge: DDS write to {rep_topic_log} failed: {e}"
                        );
                    }
                    break;
                }
            }
        });

        let req_reader = participant.create_reader(
            &req_topic,
            &req_type,
            true,
            req_reader_qos,
            Box::new(move |bytes: Vec<u8>| {
                let raw = bytes.as_slice();
                if raw.len() < 20 {
                    tracing::warn!("ZDdsClientBridge: request too short ({} bytes)", raw.len());
                    return;
                }
                let mut hdr = [0u8; 16];
                hdr.copy_from_slice(&raw[4..20]);
                let mut payload = Vec::with_capacity(4 + raw.len() - 20);
                payload.extend_from_slice(&cdr_header_matching(raw));
                payload.extend_from_slice(&raw[20..]);
                let _ = tx.try_send(PendingRequest { hdr, payload });
            }),
        )?;

        Ok(Self {
            _req_reader: req_reader,
            _rep_writer: rep_writer,
        })
    }

    /// Return the DDS request-reader GID (appears in `reader_gid_seq` in ros_discovery_info).
    pub fn reader_guid(&self) -> Option<Gid> {
        self._req_reader.guid().ok().map(Gid::from)
    }

    /// Return the DDS reply-writer GID (appears in `writer_gid_seq` in ros_discovery_info).
    pub fn writer_guid(&self) -> Option<Gid> {
        self._rep_writer.guid().ok().map(Gid::from)
    }
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sequence_number_valid() {
        let mut raw = vec![0u8; 24];
        raw[12..20].copy_from_slice(&42u64.to_le_bytes());
        assert_eq!(extract_sequence_number(&raw), Some(42));
    }

    #[test]
    fn test_extract_sequence_number_too_short() {
        assert_eq!(extract_sequence_number(&[0u8; 19]), None);
        assert_eq!(extract_sequence_number(&[]), None);
    }

    #[test]
    fn test_extract_sequence_number_exact_boundary() {
        assert_eq!(extract_sequence_number(&[0u8; 20]), Some(0));
    }

    #[test]
    fn test_extract_sequence_number_max_u64() {
        let mut raw = vec![0u8; 24];
        raw[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(extract_sequence_number(&raw), Some(u64::MAX));
    }

    #[test]
    fn test_cdr_header_matching_le() {
        assert_eq!(cdr_header_matching(&[0, 1, 0, 0]), CDR_HEADER_LE);
    }

    #[test]
    fn test_cdr_header_matching_be() {
        assert_eq!(cdr_header_matching(&[0, 0, 0, 0]), CDR_HEADER_BE);
    }

    #[test]
    fn test_cdr_header_matching_empty_defaults_to_le() {
        assert_eq!(cdr_header_matching(&[]), CDR_HEADER_LE);
    }

    #[test]
    fn test_reply_type_from_request_type_standard() {
        assert_eq!(
            reply_type_from_request_type(
                "example_interfaces::srv::dds_::AddTwoInts_Request_",
                "example_interfaces/srv/AddTwoInts"
            ),
            "example_interfaces::srv::dds_::AddTwoInts_Response_"
        );
    }

    #[test]
    fn test_reply_type_from_request_type_fallback() {
        assert_eq!(
            reply_type_from_request_type(
                "something_unexpected",
                "example_interfaces/srv/AddTwoInts"
            ),
            "example_interfaces::srv::AddTwoInts_Response_"
        );
    }

    #[test]
    fn test_request_payload_construction() {
        let client_guid = 0xaabbccdd_u64;
        let seq = 5u64;
        let payload_body = &[10u8, 20, 30];

        let mut dds_payload = Vec::new();
        dds_payload.extend_from_slice(&CDR_HEADER_LE);
        dds_payload.extend_from_slice(&client_guid.to_le_bytes());
        dds_payload.extend_from_slice(&seq.to_le_bytes());
        dds_payload.extend_from_slice(payload_body);

        assert_eq!(&dds_payload[..4], &CDR_HEADER_LE);
        assert_eq!(&dds_payload[4..12], &client_guid.to_le_bytes());
        assert_eq!(&dds_payload[12..20], &seq.to_le_bytes());
        assert_eq!(&dds_payload[20..], payload_body);
    }

    #[test]
    fn test_reply_header_stripping() {
        let mut dds_reply = vec![0u8; 24];
        dds_reply[1] = 1; // LE
        dds_reply[20..24].copy_from_slice(&[1, 2, 3, 4]);

        let raw = dds_reply.as_slice();
        let zenoh_payload: Vec<u8> = if raw.len() >= 20 {
            let mut v = Vec::new();
            v.extend_from_slice(&cdr_header_matching(raw));
            v.extend_from_slice(&raw[20..]);
            v
        } else {
            raw.to_vec()
        };
        assert_eq!(&zenoh_payload[..4], &CDR_HEADER_LE);
        assert_eq!(&zenoh_payload[4..], &[1, 2, 3, 4]);
    }

    #[test]
    fn test_reply_reconstruction_injects_saved_header() {
        let mut saved_hdr = [0u8; 16];
        saved_hdr[..8].copy_from_slice(&0x1234_u64.to_le_bytes());
        saved_hdr[8..].copy_from_slice(&7u64.to_le_bytes());

        let body = [0xAA, 0xBB, 0xCC];
        let mut zenoh_reply = CDR_HEADER_LE.to_vec();
        zenoh_reply.extend_from_slice(&body);

        let reply_body = &zenoh_reply[4..];
        let mut dds_reply = Vec::new();
        dds_reply.extend_from_slice(&cdr_header_matching(&zenoh_reply));
        dds_reply.extend_from_slice(&saved_hdr);
        dds_reply.extend_from_slice(reply_body);

        assert_eq!(&dds_reply[..4], &CDR_HEADER_LE);
        assert_eq!(&dds_reply[4..20], &saved_hdr);
        assert_eq!(&dds_reply[20..], &body);
    }
}
