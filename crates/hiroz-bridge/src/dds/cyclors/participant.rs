use std::ffi::CString;

use anyhow::{Result, anyhow};
use cyclors::{dds_create_domain, dds_create_participant};
use flume::Receiver;

use crate::dds::{
    discovery::DiscoveryEvent,
    participant::{BridgeQos, DdsParticipant},
};

use super::{
    discovery::run_discovery_raw,
    entity::{DdsEntity, get_entity_guid},
    reader::create_reader,
    writer::{CyclorsWriter, create_writer},
};

/// CycloneDDS-backed participant.
pub struct CyclorsParticipant {
    entity: DdsEntity,
}

// Safety: CycloneDDS entity handles are thread-safe for read/write operations.
unsafe impl Send for CyclorsParticipant {}
unsafe impl Sync for CyclorsParticipant {}

impl DdsParticipant for CyclorsParticipant {
    type Reader = DdsEntity;
    type Writer = CyclorsWriter;

    fn create(domain_id: u32) -> Result<Self> {
        // CycloneDDS 0.11+ does not read CYCLONEDDS_URI when dds_create_domain
        // receives a null config pointer — it uses hardcoded defaults instead.
        // Explicitly pass the env-var value so network interface overrides work.
        let config_env = std::env::var("CYCLONEDDS_URI").unwrap_or_default();
        let config_cstr =
            CString::new(config_env.as_str()).unwrap_or_else(|_| CString::new("").unwrap());
        let config_ptr = if config_env.is_empty() {
            std::ptr::null()
        } else {
            config_cstr.as_ptr()
        };

        unsafe {
            dds_create_domain(domain_id, config_ptr);
            let participant = dds_create_participant(domain_id, std::ptr::null(), std::ptr::null());
            if participant < 0 {
                return Err(anyhow!("dds_create_participant failed: {participant}"));
            }
            Ok(Self {
                entity: DdsEntity::new(participant),
            })
        }
    }

    fn participant_guid(&self) -> Result<[u8; 16]> {
        get_entity_guid(self.entity.raw())
    }

    fn run_discovery(&self) -> Receiver<DiscoveryEvent> {
        let (tx, rx) = flume::bounded(256);
        run_discovery_raw(self.entity.raw(), tx);
        rx
    }

    fn create_reader(
        &self,
        topic: &str,
        type_name: &str,
        keyless: bool,
        qos: BridgeQos,
        callback: Box<dyn Fn(Vec<u8>) + Send + 'static>,
    ) -> Result<DdsEntity> {
        create_reader(self.entity.raw(), topic, type_name, keyless, qos, callback)
    }

    fn create_writer(
        &self,
        topic: &str,
        type_name: &str,
        keyless: bool,
        qos: BridgeQos,
    ) -> Result<CyclorsWriter> {
        create_writer(self.entity.raw(), topic, type_name, keyless, qos)
    }
}
