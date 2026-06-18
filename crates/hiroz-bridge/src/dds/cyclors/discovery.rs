use std::{ffi::CStr, mem::MaybeUninit};

use cyclors::{
    DDS_BUILTIN_TOPIC_DCPSPUBLICATION, DDS_BUILTIN_TOPIC_DCPSSUBSCRIPTION,
    dds_builtintopic_endpoint_t, dds_create_listener, dds_create_reader, dds_entity_t,
    dds_get_instance_handle, dds_get_participant, dds_instance_handle_t,
    dds_instance_state_DDS_IST_ALIVE, dds_lset_data_available, dds_return_loan, dds_sample_info_t,
    dds_take,
};
use flume::Sender;

use crate::dds::{
    discovery::{DiscoveredEndpoint, DiscoveryEvent},
    gid::Gid,
    participant::BridgeQos,
};

const MAX_SAMPLES: usize = 32;

#[derive(Clone, Copy)]
enum BuiltinTopicKind {
    Publication,
    Subscription,
}

unsafe extern "C" fn on_data(dr: dds_entity_t, arg: *mut std::os::raw::c_void) {
    let btx = unsafe { &*(arg as *const (BuiltinTopicKind, Sender<DiscoveryEvent>)) };
    let kind = btx.0;
    let sender = &btx.1;

    let (_, dpih) = unsafe {
        let dp = dds_get_participant(dr);
        let mut dpih: dds_instance_handle_t = 0;
        let _ = dds_get_instance_handle(dp, &mut dpih);
        (dp, dpih)
    };

    #[allow(clippy::uninit_assumed_init)]
    let mut si = MaybeUninit::<[dds_sample_info_t; MAX_SAMPLES]>::uninit();
    let mut samples: [*mut std::os::raw::c_void; MAX_SAMPLES] = [std::ptr::null_mut(); MAX_SAMPLES];

    let (n, si) = unsafe {
        let n = dds_take(
            dr,
            samples.as_mut_ptr(),
            si.as_mut_ptr() as *mut dds_sample_info_t,
            MAX_SAMPLES,
            MAX_SAMPLES as u32,
        );
        let si = si.assume_init();
        (n, si)
    };

    for i in 0..n as usize {
        let result = unsafe {
            let sample = samples[i] as *mut dds_builtintopic_endpoint_t;
            let participant_ih = (*sample).participant_instance_handle;
            let is_alive = si[i].instance_state == dds_instance_state_DDS_IST_ALIVE;
            let key: Gid = (*sample).key.v.into();

            if participant_ih == dpih {
                continue;
            }

            let topic_name = match CStr::from_ptr((*sample).topic_name).to_str() {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };
            if topic_name.starts_with("DCPS") {
                continue;
            }
            let type_name = match CStr::from_ptr((*sample).type_name).to_str() {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };
            let participant_key: Gid = (*sample).participant_key.v.into();
            let keyless = (*sample).key.v[15] == 3 || (*sample).key.v[15] == 4;
            let bridge_qos: BridgeQos = cyclors::qos::Qos::from_qos_native((*sample).qos).into();

            (
                key,
                participant_key,
                topic_name,
                type_name,
                keyless,
                bridge_qos,
                is_alive,
            )
        };

        let (key, participant_key, topic_name, type_name, keyless, qos, is_alive) = result;

        if is_alive {
            tracing::debug!(
                "Discovered DDS endpoint {key} on {topic_name} ({type_name}) keyless={keyless}",
            );
            let endpoint = DiscoveredEndpoint {
                key,
                participant_key,
                topic_name,
                type_name,
                keyless,
                qos,
            };
            let event = match kind {
                BuiltinTopicKind::Publication => DiscoveryEvent::DiscoveredPublication(endpoint),
                BuiltinTopicKind::Subscription => DiscoveryEvent::DiscoveredSubscription(endpoint),
            };
            let _ = sender.try_send(event);
        } else {
            let event = match kind {
                BuiltinTopicKind::Publication => DiscoveryEvent::UndiscoveredPublication(key),
                BuiltinTopicKind::Subscription => DiscoveryEvent::UndiscoveredSubscription(key),
            };
            let _ = sender.try_send(event);
        }
    }

    unsafe {
        dds_return_loan(dr, samples.as_mut_ptr(), MAX_SAMPLES as i32);
    }
}

/// Install builtin-topic listeners on `dp` and forward events to `tx`.
///
/// State boxes are intentionally leaked — they live as long as the participant.
pub fn run_discovery_raw(dp: dds_entity_t, tx: Sender<DiscoveryEvent>) {
    unsafe {
        for kind in [
            BuiltinTopicKind::Publication,
            BuiltinTopicKind::Subscription,
        ] {
            let state = Box::new((kind, tx.clone()));
            let raw_ptr = Box::into_raw(state) as *mut std::os::raw::c_void;
            let listener = dds_create_listener(raw_ptr);
            dds_lset_data_available(listener, Some(on_data));
            let builtin = match kind {
                BuiltinTopicKind::Publication => DDS_BUILTIN_TOPIC_DCPSPUBLICATION,
                BuiltinTopicKind::Subscription => DDS_BUILTIN_TOPIC_DCPSSUBSCRIPTION,
            };
            dds_create_reader(dp, builtin, std::ptr::null(), listener);
        }
    }
}
