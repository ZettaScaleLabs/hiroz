#![cfg(feature = "ffi")]

use std::sync::atomic::{AtomicUsize, Ordering};

use hiroz::ffi::subscriber::{hiroz_subscriber_create_owned, hiroz_subscriber_destroy};

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
    use hiroz::ffi::{
        context::{CContextConfig, hiroz_context_create_with_config, hiroz_context_destroy},
        node::{hiroz_node_create, hiroz_node_destroy},
    };
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

// Run the lock-reentry proof in a bounded child. A broken implementation can
// deadlock the transport callback and even session teardown, so an ordinary
// assertion timeout would not make the test process itself terminate.
fn historical_self_close_case(samples: bool) {
    let name = if samples {
        "owned_historical_sample_callback_can_destroy_its_subscriber"
    } else {
        "owned_historical_callback_can_destroy_its_subscriber"
    };
    if std::env::var("HIROZ_SUBSCRIBER_HISTORY_CHILD").as_deref() != Ok(name) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("HIROZ_SUBSCRIBER_HISTORY_CHILD", name)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "historical self-close child failed: {status}"
                );
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("historical subscriber self-close deadlocked");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    historical_self_close_child(samples);
}

#[derive(Default)]
struct HistoricalCallback {
    subscriber: AtomicUsize,
    active: std::sync::atomic::AtomicBool,
    closed: std::sync::atomic::AtomicBool,
    release: (std::sync::Mutex<bool>, std::sync::Condvar),
    drops: AtomicUsize,
    dropped_while_active: std::sync::atomic::AtomicBool,
}

extern "C" fn close_from_history(data: usize, _: *const u8, _: usize) {
    let state = unsafe { &*(data as *const HistoricalCallback) };
    state.active.store(true, Ordering::SeqCst);
    let subscriber = state.subscriber.swap(0, Ordering::SeqCst);
    assert_ne!(subscriber, 0);
    assert_eq!(unsafe { hiroz_subscriber_destroy(subscriber as *mut _) }, 0);
    state.closed.store(true, Ordering::SeqCst);
    let _ = state
        .release
        .1
        .wait_timeout_while(
            state.release.0.lock().unwrap(),
            std::time::Duration::from_secs(5),
            |ready| !*ready,
        )
        .unwrap();
    state.active.store(false, Ordering::SeqCst);
}

extern "C" fn close_sample_from_history(
    data: usize,
    bytes: *const u8,
    len: usize,
    _: *const u8,
    _: usize,
    _: *const std::ffi::c_char,
) {
    close_from_history(data, bytes, len);
}

extern "C" fn history_dropped(data: usize) {
    let state = unsafe { std::sync::Arc::from_raw(data as *const HistoricalCallback) };
    if state.active.load(Ordering::SeqCst) {
        state.dropped_while_active.store(true, Ordering::SeqCst);
    }
    state.drops.fetch_add(1, Ordering::SeqCst);
}

fn historical_self_close_child(samples: bool) {
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    use hiroz::ffi::{
        context::*, node::*, qos::CQosProfile, topic::hiroz_subscriber_create_samples,
    };
    use zenoh::Wait;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("tcp/127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let mut router_config = zenoh::Config::default();
    router_config
        .set_mode(Some(zenoh::config::WhatAmI::Router))
        .unwrap();
    router_config
        .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    router_config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let router = zenoh::open(router_config).wait().unwrap();
    // Hold the history query until the FFI constructor has returned its handle.
    let history = router.declare_queryable("**/@adv/**").wait().unwrap();
    let endpoint = std::ffi::CString::new(endpoint).unwrap();
    let endpoints = [endpoint.as_ptr()];
    let config = CContextConfig {
        domain_id: 0,
        config_file: std::ptr::null(),
        connect_endpoints: endpoints.as_ptr(),
        connect_endpoints_count: 1,
        mode: c"client".as_ptr(),
        disable_multicast_scouting: true,
        connect_to_local_zenohd: false,
        json_config: c"{\"listen/endpoints\":[],\"scouting/gossip/enabled\":false}".as_ptr(),
        remap_rules: std::ptr::null(),
        remap_rules_count: 0,
        enable_logging: false,
        namespace: std::ptr::null(),
    };
    let qos = CQosProfile {
        durability: 1,
        history_depth: 10,
        ..unsafe { std::mem::zeroed() }
    };
    let state = Arc::new(HistoricalCallback::default());
    unsafe {
        let context = hiroz_context_create_with_config(&config);
        assert!(!context.is_null());
        let node = hiroz_node_create(context, c"historical_self_close".as_ptr(), std::ptr::null());
        assert!(!node.is_null());
        let user_data = Arc::into_raw(state.clone()) as usize;
        let subscriber = if samples {
            hiroz_subscriber_create_samples(
                node,
                c"/historical_close".as_ptr(),
                c"test::msg::dds_::Message_".as_ptr(),
                c"RIHS01_0000000000000000000000000000000000000000000000000000000000000000".as_ptr(),
                close_sample_from_history,
                user_data,
                &qos,
                0,
                history_dropped,
            )
        } else {
            hiroz_subscriber_create_owned(
                node,
                c"/historical_close".as_ptr(),
                c"test::msg::dds_::Message_".as_ptr(),
                c"RIHS01_0000000000000000000000000000000000000000000000000000000000000000".as_ptr(),
                close_from_history,
                user_data,
                &qos,
                history_dropped,
            )
        };
        assert!(!subscriber.is_null());
        state
            .subscriber
            .store(subscriber as usize, Ordering::SeqCst);
        let query = history
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        let topic = query
            .key_expr()
            .as_str()
            .strip_suffix("/@adv/**")
            .unwrap()
            .to_owned();
        query.reply(topic, vec![1, 2, 3]).wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !state.closed.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            state.closed.load(Ordering::SeqCst),
            "destroy did not return from historical callback"
        );
        assert_eq!(state.drops.load(Ordering::SeqCst), 0);
        hiroz_node_destroy(node);
        hiroz_context_destroy(context);
        *state.release.0.lock().unwrap() = true;
        state.release.1.notify_all();
        drop(query);
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.drops.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(state.drops.load(Ordering::SeqCst), 1);
        assert!(!state.dropped_while_active.load(Ordering::SeqCst));
    }
}

#[test]
fn owned_historical_callback_can_destroy_its_subscriber() {
    historical_self_close_case(false);
}

#[test]
fn owned_historical_sample_callback_can_destroy_its_subscriber() {
    historical_self_close_case(true);
}
