use std::{mem::MaybeUninit, slice};

use anyhow::{Result, anyhow};
use cyclors::{
    DDS_ANY_STATE, cdds_create_blob_topic, dds_create_listener, dds_create_reader, dds_entity_t,
    dds_lset_data_available, dds_reader_wait_for_historical_data, dds_sample_info_t,
    dds_strretcode, dds_takecdr, ddsi_serdata, ddsi_serdata_size, ddsi_serdata_to_ser_ref,
    ddsi_serdata_to_ser_unref, ddsi_serdata_unref, ddsrt_iovec_t,
};

use crate::dds::participant::BridgeQos;

use super::entity::DdsEntity;

// ─── Raw sample wrapper ───────────────────────────────────────────────────────

struct RawSample {
    sdref: *mut ddsi_serdata,
    data: ddsrt_iovec_t,
}

impl RawSample {
    unsafe fn create(serdata: *const ddsi_serdata) -> Self {
        let mut data = ddsrt_iovec_t {
            iov_base: std::ptr::null_mut(),
            iov_len: 0,
        };
        let sdref = unsafe {
            let size = ddsi_serdata_size(serdata);
            ddsi_serdata_to_ser_ref(serdata, 0, size as usize, &mut data)
        };
        RawSample { sdref, data }
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.data.iov_base as *const u8, self.data.iov_len) }
    }
}

impl Drop for RawSample {
    fn drop(&mut self) {
        unsafe {
            ddsi_serdata_to_ser_unref(self.sdref, &self.data);
        }
    }
}

unsafe impl Send for RawSample {}

// ─── Reader callback trampoline ───────────────────────────────────────────────

unsafe extern "C" fn on_data_available<F>(dr: dds_entity_t, arg: *mut std::os::raw::c_void)
where
    F: Fn(Vec<u8>),
{
    let cb = unsafe { &*(arg as *const F) };
    let mut zp: *mut ddsi_serdata = std::ptr::null_mut();
    #[allow(clippy::uninit_assumed_init)]
    let mut si = MaybeUninit::<[dds_sample_info_t; 1]>::uninit();
    unsafe {
        while dds_takecdr(
            dr,
            &mut zp,
            1,
            si.as_mut_ptr() as *mut dds_sample_info_t,
            DDS_ANY_STATE,
        ) > 0
        {
            let si = si.assume_init();
            if si[0].valid_data {
                let raw = RawSample::create(zp);
                cb(raw.as_slice().to_vec());
            }
            ddsi_serdata_unref(zp);
        }
    }
}

// ─── Public factory ───────────────────────────────────────────────────────────

/// Create a CycloneDDS blob reader.  `callback` receives each valid CDR sample
/// as a `Vec<u8>` (4-byte CDR header + payload).
pub fn create_reader<F>(
    dp: dds_entity_t,
    topic: &str,
    type_name: &str,
    keyless: bool,
    qos: BridgeQos,
    callback: F,
) -> Result<DdsEntity>
where
    F: Fn(Vec<u8>) + Send + 'static,
{
    unsafe {
        let c_topic = std::ffi::CString::new(topic).unwrap();
        let c_type = std::ffi::CString::new(type_name).unwrap();
        let topic_h = cdds_create_blob_topic(dp, c_topic.into_raw(), c_type.into_raw(), keyless);
        if topic_h < 0 {
            return Err(anyhow!("cdds_create_blob_topic failed: {topic_h}"));
        }

        let cb_box = Box::new(callback);
        let listener = dds_create_listener(Box::into_raw(cb_box) as *mut std::os::raw::c_void);
        dds_lset_data_available(listener, Some(on_data_available::<F>));

        let cyclors_qos: cyclors::qos::Qos = qos.into();
        let qos_native = cyclors_qos.to_qos_native();
        let reader = dds_create_reader(dp, topic_h, qos_native, listener);
        cyclors::qos::Qos::delete_qos_native(qos_native);

        if reader < 0 {
            let msg = std::ffi::CStr::from_ptr(dds_strretcode(-reader))
                .to_str()
                .unwrap_or("unknown DDS error");
            return Err(anyhow!("dds_create_reader failed: {msg}"));
        }

        let res = dds_reader_wait_for_historical_data(reader, cyclors::qos::DDS_100MS_DURATION);
        if res < 0 {
            tracing::warn!("dds_reader_wait_for_historical_data: {res}");
        }

        Ok(DdsEntity::new(reader))
    }
}
