use std::ffi::{CStr, CString};

use anyhow::{Result, anyhow};
use cyclors::{
    cdds_create_blob_topic, dds_create_writer, dds_entity_t, dds_get_entity_sertype,
    dds_strretcode, dds_writecdr, ddsi_serdata_from_ser_iov, ddsi_serdata_kind_SDK_DATA,
    ddsi_sertype, ddsrt_iov_len_t, ddsrt_iovec_t,
};

use crate::dds::participant::{BridgeQos, DdsWriter};

use super::{entity::DdsEntity, entity::get_entity_guid};

/// CycloneDDS implementation of [`DdsWriter`].
pub struct CyclorsWriter(pub(super) DdsEntity);

impl DdsWriter for CyclorsWriter {
    fn write_cdr(&self, data: Vec<u8>) -> Result<()> {
        write_cdr_raw(self.0.raw(), data)
    }

    fn instance_handle(&self) -> Result<u64> {
        get_instance_handle(self.0.raw())
    }

    fn guid(&self) -> Result<[u8; 16]> {
        get_entity_guid(self.0.raw())
    }
}

/// Create a CycloneDDS blob writer on the given participant.
pub fn create_writer(
    dp: dds_entity_t,
    topic: &str,
    type_name: &str,
    keyless: bool,
    qos: BridgeQos,
) -> Result<CyclorsWriter> {
    unsafe {
        let c_topic = CString::new(topic).unwrap();
        let c_type = CString::new(type_name).unwrap();
        let topic_h = cdds_create_blob_topic(dp, c_topic.into_raw(), c_type.into_raw(), keyless);
        if topic_h < 0 {
            return Err(anyhow!("cdds_create_blob_topic failed: {topic_h}"));
        }

        let cyclors_qos: cyclors::qos::Qos = qos.into();
        let qos_native = cyclors_qos.to_qos_native();
        let writer = dds_create_writer(dp, topic_h, qos_native, std::ptr::null_mut());
        cyclors::qos::Qos::delete_qos_native(qos_native);

        if writer < 0 {
            let msg = CStr::from_ptr(dds_strretcode(-writer))
                .to_str()
                .unwrap_or("unknown");
            return Err(anyhow!("dds_create_writer failed: {msg}"));
        }
        Ok(CyclorsWriter(DdsEntity::new(writer)))
    }
}

/// Write raw CDR bytes through a DDS entity handle.
fn write_cdr_raw(writer: dds_entity_t, data: Vec<u8>) -> Result<()> {
    unsafe {
        let len = data.len();
        let mut data = std::mem::ManuallyDrop::new(data);
        let ptr = data.as_mut_ptr();
        let cap = data.capacity();

        let iov_len: ddsrt_iov_len_t = len;
        let iov = ddsrt_iovec_t {
            iov_base: ptr as *mut std::os::raw::c_void,
            iov_len,
        };

        let mut sertype: *const ddsi_sertype = std::ptr::null();
        let ret = dds_get_entity_sertype(writer, &mut sertype);
        if ret < 0 {
            drop(Vec::from_raw_parts(ptr, len, cap));
            let msg = CStr::from_ptr(dds_strretcode(ret))
                .to_str()
                .unwrap_or("unknown");
            return Err(anyhow!("dds_get_entity_sertype failed: {msg}"));
        }

        let serdata = ddsi_serdata_from_ser_iov(sertype, ddsi_serdata_kind_SDK_DATA, 1, &iov, len);
        let ret = dds_writecdr(writer, serdata);
        drop(Vec::from_raw_parts(ptr, len, cap));

        if ret < 0 {
            let msg = CStr::from_ptr(dds_strretcode(ret))
                .to_str()
                .unwrap_or("unknown");
            return Err(anyhow!("dds_writecdr failed: {msg}"));
        }
        Ok(())
    }
}

fn get_instance_handle(entity: dds_entity_t) -> Result<u64> {
    unsafe {
        let mut handle: cyclors::dds_instance_handle_t = 0;
        let ret = cyclors::dds_get_instance_handle(entity, &mut handle);
        if ret == 0 {
            Ok(handle)
        } else {
            Err(anyhow!("dds_get_instance_handle failed: {ret}"))
        }
    }
}
