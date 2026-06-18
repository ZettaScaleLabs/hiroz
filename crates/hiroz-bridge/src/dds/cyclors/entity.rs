use anyhow::{Result, anyhow};
use cyclors::{dds_delete, dds_entity_t, dds_get_guid, dds_guid_t};

use crate::dds::participant::DdsReader;

/// RAII wrapper around a `dds_entity_t`.
///
/// Calls `dds_delete` on drop, preventing ghost-subscriber accumulation.
pub struct DdsEntity(pub(super) dds_entity_t);

impl DdsEntity {
    /// Wrap an existing DDS entity handle.
    ///
    /// # Safety
    /// `handle` must be a valid entity created by the caller and not yet deleted.
    pub unsafe fn new(handle: dds_entity_t) -> Self {
        Self(handle)
    }

    pub fn raw(&self) -> dds_entity_t {
        self.0
    }
}

impl Drop for DdsEntity {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                dds_delete(self.0);
            }
        }
    }
}

unsafe impl Send for DdsEntity {}
unsafe impl Sync for DdsEntity {}

impl DdsReader for DdsEntity {
    fn guid(&self) -> Result<[u8; 16]> {
        get_entity_guid(self.0)
    }
}

/// Get the 16-byte DDS GUID of any DDS entity.
pub(super) fn get_entity_guid(entity: dds_entity_t) -> Result<[u8; 16]> {
    unsafe {
        let mut guid = dds_guid_t { v: [0u8; 16] };
        let r = dds_get_guid(entity, &mut guid);
        if r == 0 {
            Ok(guid.v)
        } else {
            Err(anyhow!("dds_get_guid failed: {r}"))
        }
    }
}
