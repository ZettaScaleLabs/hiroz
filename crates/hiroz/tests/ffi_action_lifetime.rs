#![cfg(feature = "ffi")]

use hiroz::ffi::action::{
    CActionClient, CActionQos, CActionRequest, CActionServer, hiroz_action_client_create_owned,
    hiroz_action_client_create_with_options, hiroz_action_client_destroy,
    hiroz_action_request_create, hiroz_action_request_destroy, hiroz_action_request_poll,
    hiroz_action_server_create_with_options, hiroz_action_server_destroy,
    hiroz_action_server_execute, hiroz_action_server_publish_feedback_message,
};
use hiroz::ffi::context::{
    CContext, CContextConfig, hiroz_context_create_with_config, hiroz_context_destroy,
};
use hiroz::ffi::node::{CNode, hiroz_node_create, hiroz_node_destroy};
use hiroz::ffi::publisher::{
    CPublisher, hiroz_publisher_create_with_qos, hiroz_publisher_destroy, hiroz_publisher_publish,
};
use hiroz::ffi::qos::CQosProfile;
use hiroz::ffi::serialize::hiroz_free_bytes;
use hiroz::ffi::service::{hiroz_service_server_create, hiroz_service_server_destroy};
use hiroz::qos::QosDuration;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};
use zenoh::Wait;

const HASH: &std::ffi::CStr =
    c"RIHS01_0000000000000000000000000000000000000000000000000000000000000000";

fn qos(transient_local: bool) -> CQosProfile {
    CQosProfile {
        reliability: 0,
        durability: i32::from(transient_local),
        history: 0,
        history_depth: 10,
        deadline_sec: QosDuration::INFINITE.sec,
        deadline_nsec: QosDuration::INFINITE.nsec,
        lifespan_sec: QosDuration::INFINITE.sec,
        lifespan_nsec: QosDuration::INFINITE.nsec,
        liveliness: 0,
        liveliness_lease_sec: QosDuration::INFINITE.sec,
        liveliness_lease_nsec: QosDuration::INFINITE.nsec,
    }
}

#[derive(Default)]
struct Events {
    entered: AtomicUsize,
    active: AtomicUsize,
    dropped: AtomicUsize,
    released_while_active: AtomicBool,
    destroy_self: AtomicUsize,
    release: (Mutex<bool>, Condvar),
}

impl Events {
    fn release(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }

    fn wait_for_delivery(&self) {
        wait_until(|| self.entered.load(Ordering::SeqCst) > 0);
    }

    fn wait_in_callback(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
        let client = self.destroy_self.swap(0, Ordering::SeqCst);
        if client != 0 {
            unsafe { hiroz_action_client_destroy(client as *mut CActionClient) };
        }
        self.entered.fetch_add(1, Ordering::SeqCst);
        let _ = self
            .release
            .1
            .wait_timeout_while(
                self.release.0.lock().unwrap(),
                Duration::from_secs(5),
                |ready| !*ready,
            )
            .unwrap();
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

extern "C" fn event(data: usize, _: i32, _: *const u8, _: usize) {
    let events = unsafe { retain_events(data) };
    events.wait_in_callback();
}

extern "C" fn dropped(data: usize) {
    let events = unsafe { Arc::from_raw(data as *const Events) };
    if events.active.load(Ordering::SeqCst) != 0 {
        events.released_while_active.store(true, Ordering::SeqCst);
    }
    events.dropped.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn blocked_service(
    data: usize,
    _: *const u8,
    _: usize,
    _: *mut *mut u8,
    _: *mut usize,
) -> i32 {
    let events = unsafe { retain_events(data) };
    events.wait_in_callback();
    -1
}

unsafe fn retain_events(data: usize) -> Arc<Events> {
    unsafe {
        Arc::increment_strong_count(data as *const Events);
        Arc::from_raw(data as *const Events)
    }
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "callback lifecycle did not advance"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Fixture {
    context: *mut CContext,
    node: *mut CNode,
    publisher: *mut CPublisher,
    publications: Mutex<Vec<Arc<Mutex<Option<std::thread::JoinHandle<i32>>>>>>,
}

struct Publication(Arc<Mutex<Option<std::thread::JoinHandle<i32>>>>);
impl Publication {
    fn join(self) -> std::thread::Result<i32> {
        self.0.lock().unwrap().take().unwrap().join()
    }
}

impl Fixture {
    fn new(status: bool) -> Self {
        Self::with_endpoint(status, None)
    }

    fn with_endpoint(status: bool, endpoint: Option<&std::ffi::CStr>) -> Self {
        let endpoints = endpoint.map(|endpoint| [endpoint.as_ptr()]);
        let config = CContextConfig {
            domain_id: 0,
            config_file: std::ptr::null(),
            connect_endpoints: endpoints.as_ref().map_or(std::ptr::null(), |e| e.as_ptr()),
            connect_endpoints_count: usize::from(endpoint.is_some()),
            mode: if endpoint.is_some() {
                c"client".as_ptr()
            } else {
                std::ptr::null()
            },
            disable_multicast_scouting: true,
            connect_to_local_zenohd: false,
            json_config: c"{\"listen/endpoints\":[],\"scouting/gossip/enabled\":false}".as_ptr(),
            remap_rules: std::ptr::null(),
            remap_rules_count: 0,
            enable_logging: false,
            namespace: std::ptr::null(),
        };
        unsafe {
            let context = hiroz_context_create_with_config(&config);
            assert!(!context.is_null());
            let node = hiroz_node_create(context, c"action_lifetime".as_ptr(), std::ptr::null());
            assert!(!node.is_null());
            let (topic, name) = if status {
                (
                    c"/lifetime/_action/status",
                    c"action_msgs::msg::dds_::GoalStatusArray_",
                )
            } else {
                (
                    c"/lifetime/_action/feedback",
                    c"lifetime::action::dds_::Test_FeedbackMessage_",
                )
            };
            let publisher = hiroz_publisher_create_with_qos(
                node,
                topic.as_ptr(),
                name.as_ptr(),
                HASH.as_ptr(),
                &qos(status),
            );
            assert!(!publisher.is_null());
            Self {
                context,
                node,
                publisher,
                publications: Mutex::new(Vec::new()),
            }
        }
    }

    fn client(&self, events: &Arc<Events>, owned: bool) -> *mut CActionClient {
        unsafe { client(self.node, events, owned) }
    }

    fn publish(&self) -> Publication {
        let publisher = self.publisher as usize;
        let publication = Arc::new(Mutex::new(Some(std::thread::spawn(move || unsafe {
            hiroz_publisher_publish(publisher as *mut CPublisher, [0, 1, 0, 0].as_ptr(), 4)
        }))));
        self.publications.lock().unwrap().push(publication.clone());
        Publication(publication)
    }
}

unsafe fn client(node: *mut CNode, events: &Arc<Events>, owned: bool) -> *mut CActionClient {
    let data = if owned {
        Arc::into_raw(events.clone())
    } else {
        Arc::as_ptr(events)
    } as usize;
    // Match Go ActionQoSDefault, including the AdvancedSubscriber status path.
    let qos = CActionQos {
        goal: qos(false),
        result: qos(false),
        cancel: qos(false),
        feedback: qos(false),
        status: qos(true),
    };
    unsafe {
        if owned {
            hiroz_action_client_create_owned(
                node,
                c"/lifetime".as_ptr(),
                c"lifetime/action/Test".as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                &qos,
                event,
                data,
                dropped,
            )
        } else {
            hiroz_action_client_create_with_options(
                node,
                c"/lifetime".as_ptr(),
                c"lifetime/action/Test".as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                HASH.as_ptr(),
                &qos,
                event,
                data,
            )
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Join even when a test assertion unwinds; never free a publisher while
        // a delivery thread is still using it. Callbacks have a bounded wait.
        for publication in self.publications.get_mut().unwrap().drain(..) {
            if let Some(thread) = publication.lock().unwrap().take() {
                let _ = thread.join();
            }
        }
        unsafe {
            hiroz_publisher_destroy(self.publisher);
            hiroz_node_destroy(self.node);
            hiroz_context_destroy(self.context);
        }
    }
}

#[test]
fn owned_callback_data_is_released_once_on_creation_failure() {
    let events = Arc::new(Events::default());
    assert!(unsafe { client(std::ptr::null_mut(), &events, true) }.is_null());
    assert_eq!(events.dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn owned_callback_data_outlives_destroy_during_feedback_and_status_delivery() {
    for status in [false, true] {
        let fixture = Fixture::new(status);
        let events = Arc::new(Events::default());
        let client = fixture.client(&events, true);
        assert!(!client.is_null());
        let delivery = fixture.publish();
        events.wait_for_delivery();
        assert_eq!(unsafe { hiroz_action_client_destroy(client) }, 0);
        assert_eq!(events.dropped.load(Ordering::SeqCst), 0);
        events.release();
        assert_eq!(delivery.join().unwrap(), 0);
        wait_until(|| events.dropped.load(Ordering::SeqCst) != 0);
        assert_eq!(events.dropped.load(Ordering::SeqCst), 1);
        assert!(!events.released_while_active.load(Ordering::SeqCst));
        assert_eq!(fixture.publish().join().unwrap(), 0);
        assert_eq!(events.entered.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn owned_callback_can_destroy_its_own_client() {
    let fixture = Fixture::new(true);
    let events = Arc::new(Events::default());
    let client = fixture.client(&events, true);
    assert!(!client.is_null());
    events.destroy_self.store(client as usize, Ordering::SeqCst);
    let delivery = fixture.publish();
    events.wait_for_delivery(); // Signals after self-destroy has returned.
    assert_eq!(events.dropped.load(Ordering::SeqCst), 0);
    events.release();
    assert_eq!(delivery.join().unwrap(), 0);
    wait_until(|| events.dropped.load(Ordering::SeqCst) != 0);
    assert_eq!(events.dropped.load(Ordering::SeqCst), 1);
    assert!(!events.released_while_active.load(Ordering::SeqCst));
}

#[test]
fn owned_historical_status_callback_can_destroy_its_own_client() {
    // Hold an actual AdvancedSubscriber history response until construction has
    // returned. Unlike live delivery, the history callback holds the advanced
    // subscriber state mutex without retaining its live subscriber callback.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("tcp/127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let mut config = zenoh::Config::default();
    config
        .set_mode(Some(zenoh::config::WhatAmI::Router))
        .unwrap();
    config
        .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let session = zenoh::open(config).wait().unwrap();
    let history = session.declare_queryable("**/@adv/**").wait().unwrap();
    let endpoint = std::ffi::CString::new(endpoint).unwrap();
    let fixture = Fixture::with_endpoint(true, Some(&endpoint));
    let events = Arc::new(Events::default());
    let client = fixture.client(&events, true);
    assert!(!client.is_null());
    events.destroy_self.store(client as usize, Ordering::SeqCst);
    let query = history
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let status_key = query
        .key_expr()
        .as_str()
        .strip_suffix("/@adv/**")
        .unwrap()
        .to_owned();
    query.reply(status_key, vec![0, 1, 0, 0]).wait().unwrap();
    events.wait_for_delivery(); // Destroy must return while the history lock is held.
    assert_eq!(events.dropped.load(Ordering::SeqCst), 0);
    events.release();
    drop(query);
    wait_until(|| events.dropped.load(Ordering::SeqCst) != 0);
    assert_eq!(events.dropped.load(Ordering::SeqCst), 1);
    assert!(!events.released_while_active.load(Ordering::SeqCst));
}

#[test]
fn legacy_destroy_drains_the_entire_foreign_callback() {
    let fixture = Fixture::new(true);
    let events = Arc::new(Events::default());
    let client = fixture.client(&events, false);
    assert!(!client.is_null());
    let delivery = fixture.publish();
    events.wait_for_delivery();
    let (tx, rx) = mpsc::channel();
    let client = client as usize;
    let destroy = std::thread::spawn(move || {
        let result = unsafe { hiroz_action_client_destroy(client as *mut CActionClient) };
        tx.send(result).unwrap();
    });
    let prematurely_destroyed = rx.recv_timeout(Duration::from_millis(100)).is_ok();
    events.release();
    assert_eq!(delivery.join().unwrap(), 0);
    destroy.join().unwrap();
    assert!(
        !prematurely_destroyed,
        "destroy returned while foreign data was in use"
    );
}

#[test]
fn pending_request_clone_retains_callback_data_after_client_destroy() {
    let fixture = Fixture::new(false);
    let events = Arc::new(Events::default());
    let service_events = Arc::new(Events::default());
    unsafe {
        let server = hiroz_service_server_create(
            fixture.node,
            c"/lifetime/_action/get_result".as_ptr(),
            c"lifetime::action::dds_::Test_GetResult_".as_ptr(),
            HASH.as_ptr(),
            c"lifetime::action::dds_::Test_GetResult_".as_ptr(),
            HASH.as_ptr(),
            blocked_service,
            Arc::as_ptr(&service_events) as usize,
        );
        assert!(!server.is_null());
        let client = fixture.client(&events, true);
        assert!(!client.is_null());
        let request = hiroz_action_request_create(client, 1, [0, 1, 0, 0].as_ptr(), 4, 10_000);
        assert!(!request.is_null());
        service_events.wait_for_delivery();
        assert_eq!(hiroz_action_client_destroy(client), 0);
        assert_eq!(events.dropped.load(Ordering::SeqCst), 0);
        // The retained transport must no longer enter foreign event code.
        assert_eq!(fixture.publish().join().unwrap(), 0);
        assert_eq!(events.entered.load(Ordering::SeqCst), 0);
        hiroz_action_request_destroy(request);
        wait_until(|| events.dropped.load(Ordering::SeqCst) != 0);
        assert_eq!(events.dropped.load(Ordering::SeqCst), 1);
        service_events.release();
        hiroz_service_server_destroy(server);
    }
}

extern "C" fn deferred_goal(_: usize, _: *const u8, _: *const u8, _: usize) -> i32 {
    2
}

extern "C" fn deferred_execution(
    _: usize,
    _: *const u8,
    _: *const u8,
    _: usize,
    _: *mut *mut u8,
    _: *mut usize,
) -> i32 {
    0
}

extern "C" fn reject_cancel(_: usize, _: *const u8) -> i32 {
    0
}

#[derive(Default)]
struct ServerHandles {
    server: *mut CActionServer,
    client: *mut CActionClient,
    request: *mut CActionRequest,
}

impl Drop for ServerHandles {
    fn drop(&mut self) {
        unsafe {
            hiroz_action_request_destroy(self.request);
            hiroz_action_client_destroy(self.client);
            hiroz_action_server_destroy(self.server);
        }
    }
}

#[test]
fn server_retains_session_until_endpoints_are_destroyed() {
    let mut fixture = Fixture::new(false);
    let mut handles = ServerHandles::default();
    let events = Arc::new(Events::default());
    events.release();
    unsafe {
        let empty_result = [0, 1, 0, 0, 0];
        handles.server = hiroz_action_server_create_with_options(
            fixture.node,
            c"/lifetime".as_ptr(),
            c"lifetime/action/Test".as_ptr(),
            HASH.as_ptr(),
            HASH.as_ptr(),
            HASH.as_ptr(),
            HASH.as_ptr(),
            HASH.as_ptr(),
            std::ptr::null(),
            deferred_goal,
            deferred_execution,
            reject_cancel,
            0,
            empty_result.as_ptr(),
            empty_result.len(),
            10_000,
            0,
        );
        assert!(!handles.server.is_null());
        handles.client = fixture.client(&events, true);
        assert!(!handles.client.is_null());
        // A real accepted goal lets feedback publication check session liveness
        // without reaching into the private handle or retaining another owner.
        let mut message = vec![0, 1, 0, 0];
        message.extend_from_slice(&[1; 16]);
        handles.request =
            hiroz_action_request_create(handles.client, 0, message.as_ptr(), message.len(), 3_000);
        assert!(!handles.request.is_null());
        let mut bytes = std::ptr::null_mut();
        let mut len = 0;
        let mut status = 1;
        wait_until(|| {
            status = hiroz_action_request_poll(handles.request, &mut bytes, &mut len);
            status != 1
        });
        assert_eq!(status, 0);
        let accepted = len > 4 && *bytes.add(4) == 1;
        hiroz_free_bytes(bytes, len);
        assert!(accepted);
        hiroz_action_request_destroy(std::mem::take(&mut handles.request));
        assert_eq!(
            hiroz_action_client_destroy(std::mem::take(&mut handles.client)),
            0
        );
        assert_eq!(
            hiroz_action_server_execute(handles.server, [1; 16].as_ptr()),
            0
        );
        assert_eq!(
            hiroz_publisher_destroy(std::mem::take(&mut fixture.publisher)),
            0
        );
        assert_eq!(hiroz_node_destroy(std::mem::take(&mut fixture.node)), 0);
        assert_eq!(
            hiroz_context_destroy(std::mem::take(&mut fixture.context)),
            0
        );
        // Before the lifetime guard this returns ActionFeedbackFailed because
        // parent teardown has already closed the still-live server's session.
        assert_eq!(
            hiroz_action_server_publish_feedback_message(
                handles.server,
                message.as_ptr(),
                message.len(),
            ),
            0,
            "server lost its session before its endpoints were destroyed",
        );
    }
}
