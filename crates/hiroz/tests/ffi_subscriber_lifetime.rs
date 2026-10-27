#![cfg(feature = "ffi")]

use hiroz::ffi::subscriber::{hiroz_subscriber_create_owned, hiroz_subscriber_destroy};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

extern "C" fn receive(_: usize, _: *const u8, _: usize) {}

extern "C" fn dropped(user_data: usize) {
    let count = unsafe { &*(user_data as *const AtomicUsize) };
    count.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn owned_callback_is_released_when_creation_fails() {
    let drops = AtomicUsize::new(0);
    let subscriber = unsafe {
        hiroz_subscriber_create_owned(
            std::ptr::null_mut(),
            c"topic".as_ptr(),
            c"test/msg/Message".as_ptr(),
            c"invalid".as_ptr(),
            receive,
            &drops as *const AtomicUsize as usize,
            std::ptr::null(),
            dropped,
        )
    };
    assert!(subscriber.is_null());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn owned_callback_is_released_after_subscriber_destroy() {
    use hiroz::ffi::context::{
        CContextConfig, hiroz_context_create_with_config, hiroz_context_destroy,
    };
    use hiroz::ffi::node::{hiroz_node_create, hiroz_node_destroy};
    let config = CContextConfig {
        domain_id: 0,
        config_file: std::ptr::null(),
        connect_endpoints: std::ptr::null(),
        connect_endpoints_count: 0,
        mode: std::ptr::null(),
        disable_multicast_scouting: true,
        connect_to_local_zenohd: false,
        json_config: c"{\"listen/endpoints\":[],\"scouting/gossip/enabled\":false}".as_ptr(),
        remap_rules: std::ptr::null(),
        remap_rules_count: 0,
        enable_logging: false,
        namespace: std::ptr::null(),
    };
    let drops = AtomicUsize::new(0);
    unsafe {
        let context = hiroz_context_create_with_config(&config);
        assert!(!context.is_null());
        let node = hiroz_node_create(context, c"callback_lifetime".as_ptr(), std::ptr::null());
        assert!(!node.is_null());
        let subscriber = hiroz_subscriber_create_owned(
            node,
            c"topic".as_ptr(),
            c"test/msg/Message".as_ptr(),
            c"RIHS01_0000000000000000000000000000000000000000000000000000000000000000".as_ptr(),
            receive,
            &drops as *const AtomicUsize as usize,
            std::ptr::null(),
            dropped,
        );
        assert!(!subscriber.is_null());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(hiroz_subscriber_destroy(subscriber), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        hiroz_node_destroy(node);
        hiroz_context_destroy(context);
    }
}
