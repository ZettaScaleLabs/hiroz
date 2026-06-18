//! `ros_discovery_info` publishing — makes bridge DDS entities visible to the ROS 2 graph.
//!
//! Publishes a `ParticipantEntitiesInfo` CDR-LE message on the `ros_discovery_info` DDS topic
//! at ~1 Hz whenever the set of bridged DDS readers/writers changes.  This allows standard ROS 2
//! tools (`ros2 topic list`, `ros2 node list`, `ros2 service list`) to see the bridge.
//!
//! Wire format: CDR-LE with 16-byte GIDs (Iron/Jazzy compatible).

use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Result;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use super::{
    gid::Gid,
    participant::{
        BridgeQos, DdsParticipant, DdsWriter, Durability, DurabilityKind, History, HistoryKind,
        Reliability, ReliabilityKind,
    },
};

const ROS_DISCOVERY_INFO_TOPIC_NAME: &str = "ros_discovery_info";
const ROS_DISCOVERY_INFO_TOPIC_TYPE: &str = "rmw_dds_common::msg::dds_::ParticipantEntitiesInfo_";
const PUBLISH_INTERVAL_MS: u64 = 1000;

// ─── CDR wire structs ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct NodeEntitiesInfo {
    node_namespace: String,
    node_name: String,
    reader_gid_seq: Vec<Gid>,
    writer_gid_seq: Vec<Gid>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ParticipantEntitiesInfo {
    gid: Gid,
    node_entities_info_seq: Vec<NodeEntitiesInfo>,
}

// ─── Internal state ───────────────────────────────────────────────────────────

struct State {
    participant_gid: Gid,
    node_namespace: String,
    node_name: String,
    reader_gids: HashSet<Gid>,
    writer_gids: HashSet<Gid>,
    changed: bool,
}

impl State {
    fn serialize_cdr(&self) -> Option<Vec<u8>> {
        let info = ParticipantEntitiesInfo {
            gid: self.participant_gid,
            node_entities_info_seq: vec![NodeEntitiesInfo {
                node_namespace: self.node_namespace.clone(),
                node_name: self.node_name.clone(),
                reader_gid_seq: self.reader_gids.iter().cloned().collect(),
                writer_gid_seq: self.writer_gids.iter().cloned().collect(),
            }],
        };
        match cdr::serialize::<_, _, cdr::CdrLe>(&info, cdr::Infinite) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!("ros_discovery_info: CDR serialization failed: {e}");
                None
            }
        }
    }
}

// ─── RosDiscoveryPublisher ────────────────────────────────────────────────────

/// Publishes `ros_discovery_info` CDR messages so the ROS 2 graph can see bridge endpoints.
///
/// Created once per [`ZDdsBridge`](crate::bridge::ZDdsBridge). Call
/// [`add_reader`] / [`add_writer`] as DDS bridge routes are established, and the
/// matching `remove_*` methods when routes are torn down.  Changes are broadcast
/// to the DDS participant graph at ~1 Hz.
pub struct RosDiscoveryPublisher {
    state: Arc<Mutex<State>>,
    _task: JoinHandle<()>,
}

impl RosDiscoveryPublisher {
    /// Create a new publisher backed by `participant`.
    ///
    /// - `namespace` — bridge node namespace (e.g. `"/"`)
    /// - `node_name` — bridge node name (e.g. `"ros_z_bridge"`)
    pub fn new<P: DdsParticipant>(
        participant: &P,
        namespace: &str,
        node_name: &str,
    ) -> Result<Self> {
        let participant_gid = Gid::from(participant.participant_guid()?);

        let qos = BridgeQos {
            reliability: Some(Reliability {
                kind: ReliabilityKind::Reliable,
                max_blocking_time: None,
            }),
            durability: Some(Durability {
                kind: DurabilityKind::TransientLocal,
            }),
            history: Some(History {
                kind: HistoryKind::KeepLast,
                depth: 1,
            }),
            ignore_local: true,
            ..Default::default()
        };

        let writer = participant.create_writer(
            ROS_DISCOVERY_INFO_TOPIC_NAME,
            ROS_DISCOVERY_INFO_TOPIC_TYPE,
            true,
            qos,
        )?;

        let state = Arc::new(Mutex::new(State {
            participant_gid,
            node_namespace: namespace.to_string(),
            node_name: node_name.to_string(),
            reader_gids: HashSet::new(),
            writer_gids: HashSet::new(),
            changed: true,
        }));

        let state_bg = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(PUBLISH_INTERVAL_MS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let payload = {
                    let mut s = state_bg.lock();
                    if !s.changed {
                        continue;
                    }
                    s.changed = false;
                    s.serialize_cdr()
                };
                if let Some(bytes) = payload {
                    if let Err(e) = writer.write_cdr(bytes) {
                        tracing::warn!("ros_discovery_info: DDS write failed: {e}");
                    } else {
                        tracing::debug!("ros_discovery_info: published update");
                    }
                }
            }
        });

        tracing::info!(
            "ros_discovery_info: publishing for {}{} (participant GID {})",
            namespace,
            node_name,
            participant_gid,
        );

        Ok(Self { state, _task: task })
    }

    pub fn add_reader(&self, gid: Gid) {
        let mut s = self.state.lock();
        s.reader_gids.insert(gid);
        s.changed = true;
        tracing::debug!("ros_discovery_info: +reader {gid}");
    }

    pub fn remove_reader(&self, gid: Gid) {
        let mut s = self.state.lock();
        s.reader_gids.remove(&gid);
        s.changed = true;
        tracing::debug!("ros_discovery_info: -reader {gid}");
    }

    pub fn add_writer(&self, gid: Gid) {
        let mut s = self.state.lock();
        s.writer_gids.insert(gid);
        s.changed = true;
        tracing::debug!("ros_discovery_info: +writer {gid}");
    }

    pub fn remove_writer(&self, gid: Gid) {
        let mut s = self.state.lock();
        s.writer_gids.remove(&gid);
        s.changed = true;
        tracing::debug!("ros_discovery_info: -writer {gid}");
    }
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gid(byte: u8) -> Gid {
        Gid::from([byte; 16])
    }

    #[test]
    fn test_state_serializes_to_valid_cdr() {
        let state = State {
            participant_gid: make_gid(0x01),
            node_namespace: "/".to_string(),
            node_name: "bridge".to_string(),
            reader_gids: HashSet::new(),
            writer_gids: HashSet::new(),
            changed: false,
        };
        let bytes = state.serialize_cdr().expect("serialization failed");
        // CDR header (4) + participant gid (16) + seq len (4) + 1 node entry
        assert!(
            bytes.len() >= 24,
            "CDR output too short: {} bytes",
            bytes.len()
        );
        // CDR LE header = [0, 1, 0, 0]
        assert_eq!(&bytes[..4], &[0, 1, 0, 0]);
        // participant gid bytes
        assert_eq!(&bytes[4..20], &[0x01u8; 16]);
        // node_entities_info_seq length = 1
        assert_eq!(&bytes[20..24], &[1, 0, 0, 0]);
    }

    #[test]
    fn test_state_serializes_gids_in_reader_writer_seqs() {
        let mut state = State {
            participant_gid: make_gid(0x02),
            node_namespace: "/".to_string(),
            node_name: "bridge".to_string(),
            reader_gids: HashSet::from([make_gid(0xAA)]),
            writer_gids: HashSet::from([make_gid(0xBB), make_gid(0xCC)]),
            changed: false,
        };
        let bytes = state.serialize_cdr().expect("serialization failed");
        // Round-trip: count reader GIDs and writer GIDs embedded in the payload
        let reader_count = bytes.windows(16).filter(|w| *w == [0xAA; 16]).count();
        let writer_count = bytes
            .windows(16)
            .filter(|w| *w == [0xBB; 16] || *w == [0xCC; 16])
            .count();
        assert_eq!(reader_count, 1);
        assert_eq!(writer_count, 2);
        // Swap and verify changed flag works
        state.changed = true;
        let bytes2 = state.serialize_cdr();
        assert!(bytes2.is_some());
    }

    #[test]
    fn test_add_remove_reader_writer() {
        let gid_a = make_gid(0x10);
        let gid_b = make_gid(0x20);
        let mut state = State {
            participant_gid: make_gid(0x00),
            node_namespace: "/".to_string(),
            node_name: "bridge".to_string(),
            reader_gids: HashSet::new(),
            writer_gids: HashSet::new(),
            changed: false,
        };

        state.reader_gids.insert(gid_a);
        state.writer_gids.insert(gid_b);
        assert!(state.reader_gids.contains(&gid_a));
        assert!(state.writer_gids.contains(&gid_b));

        state.reader_gids.remove(&gid_a);
        assert!(!state.reader_gids.contains(&gid_a));
    }
}
