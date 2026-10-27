//! Exercise transport-generated timeouts through the actual FFI call handle.
#![cfg(feature = "ffi")]

use std::{ptr, time::Duration};

use hiroz::{
    ffi::{
        ErrorCode,
        context::{CContextConfig, hiroz_context_create_with_config, hiroz_context_destroy},
        node::{hiroz_node_create, hiroz_node_destroy},
        service::*,
    },
    prelude::*,
};
use zenoh::Wait;

const HASH: &std::ffi::CStr =
    c"RIHS01_0000000000000000000000000000000000000000000000000000000000000000";
const TYPE: &std::ffi::CStr = c"test::srv::dds_::TimeoutProbe_";

#[test]
fn transport_timeout_buffered_before_poll_keeps_timeout_code() {
    let config = CContextConfig {
        domain_id: 0,
        config_file: ptr::null(),
        connect_endpoints: ptr::null(),
        connect_endpoints_count: 0,
        mode: ptr::null(),
        disable_multicast_scouting: true,
        connect_to_local_zenohd: false,
        json_config: c"{\"listen/endpoints\":[],\"scouting/gossip/enabled\":false}".as_ptr(),
        remap_rules: ptr::null(),
        remap_rules_count: 0,
        enable_logging: false,
        namespace: ptr::null(),
    };
    unsafe {
        let context = hiroz_context_create_with_config(&config);
        assert!(!context.is_null());
        let node = hiroz_node_create(context, c"ffi_timeout_probe".as_ptr(), ptr::null());
        assert!(!node.is_null());
        let server = hiroz_service_server_create_queued(
            node,
            c"timeout_probe".as_ptr(),
            TYPE.as_ptr(),
            HASH.as_ptr(),
            ptr::null(),
        );
        assert!(!server.is_null());
        let client = hiroz_service_client_create(
            node,
            c"timeout_probe".as_ptr(),
            TYPE.as_ptr(),
            HASH.as_ptr(),
            TYPE.as_ptr(),
            HASH.as_ptr(),
        );
        assert!(!client.is_null());
        assert_eq!(hiroz_service_client_wait_for_service(client, 2000), 0);
        let mut call = ptr::null_mut();
        assert_eq!(
            hiroz_service_client_start_call(client, [1].as_ptr(), 1, 100, &mut call),
            0
        );
        let mut request = ptr::null_mut();
        let mut data = ptr::null_mut();
        let mut len = 0;
        let mut id = std::mem::zeroed();
        assert_eq!(
            hiroz_service_server_take_request(
                server,
                2000,
                &mut request,
                &mut data,
                &mut len,
                &mut id
            ),
            0
        );
        hiroz::ffi::serialize::hiroz_free_bytes(data, len);

        // Keep the real query alive, but send no response. Do not poll until
        // after Zenoh has put its synthetic timeout reply into the call queue;
        // this exercises the callback path rather than only the poll deadline.
        std::thread::sleep(Duration::from_millis(400));
        let result = hiroz_service_call_poll(call, 0, &mut data, &mut len);
        hiroz_service_call_destroy(call);
        hiroz_service_request_destroy(request);
        hiroz_service_client_destroy(client);
        hiroz_service_server_destroy(server);
        hiroz_node_destroy(node);
        hiroz_context_destroy(context);
        assert_eq!(result, ErrorCode::ServiceTimeout as i32);
        assert!(data.is_null());
        assert_eq!(len, 0);
    }
}

#[test]
fn server_payload_named_timeout_is_not_a_transport_timeout() {
    let context = ZContextBuilder::default()
        .with_json("scouting/multicast/enabled", false)
        .with_json("scouting/gossip/enabled", false)
        .with_json("listen/endpoints", Vec::<String>::new())
        .build()
        .unwrap();
    let node = context.create_node("server_error_probe").build().unwrap();
    let selector = format!(
        "0/server_error_probe/{}/{}",
        TYPE.to_str().unwrap(),
        HASH.to_str().unwrap()
    );
    let _server = node
        .session()
        .declare_queryable(selector)
        .complete(true)
        .callback(|query| {
            query
                .reply_err("Timeout")
                .encoding(zenoh::bytes::Encoding::ZENOH_STRING)
                .wait()
                .unwrap();
        })
        .wait()
        .unwrap();
    let client = node
        .create_raw_service_client(
            "/server_error_probe",
            TYPE.to_str().unwrap(),
            HASH.to_str().unwrap(),
        )
        .unwrap();
    let result = client.call_raw(&[1], Duration::from_secs(2));
    assert!(
        matches!(result, Err(hiroz::error::Error::Other(_))),
        "server error was misclassified: {result:?}"
    );
}
