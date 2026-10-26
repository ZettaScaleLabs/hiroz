//! The versioned C QoS struct `hiroz_qos_profile_v2_t`, and the `_with_qos_v2` creators.
//!
//! Every test calls the exported C functions the way a C caller does. Each test reads the QoS back
//! from the liveliness token of the entity, because that is where a peer reads it. A plain Zenoh
//! session plays the peer. No test needs a ROS installation.
//!
//! The suite needs hiroz's `ffi` feature, so it has the `raw-sub-tests` feature.

#![cfg(target_os = "linux")]
#![cfg(feature = "raw-sub-tests")]

mod common;

use std::{
    ffi::CString,
    time::{Duration, Instant},
};

use common::{TestRouter, buffer_rmw, buffer_rmw::tokens};
use hiroz::{
    ffi::{
        action::{
            CActionClient, CActionServer, hiroz_action_client_create_with_qos_v2,
            hiroz_action_client_destroy, hiroz_action_server_create_with_qos_v2,
            hiroz_action_server_destroy,
        },
        context::{
            CContext, CContextConfig, hiroz_context_create_with_config, hiroz_context_destroy,
        },
        node::{CNode, hiroz_node_create, hiroz_node_destroy},
        publisher::{
            CPublisher, hiroz_publisher_create_with_qos, hiroz_publisher_create_with_qos_v2,
            hiroz_publisher_destroy,
        },
        qos::{CActionQosV2, CQosProfile, CQosProfileV2, hiroz_qos_profile_v2_init},
        service::{
            CServiceClient, CServiceServer, hiroz_service_client_create_with_qos_v2,
            hiroz_service_client_destroy, hiroz_service_server_create_with_qos_v2,
            hiroz_service_server_destroy,
        },
        subscriber::{CSubscriber, hiroz_subscriber_create_with_qos_v2, hiroz_subscriber_destroy},
    },
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
};

const RECV: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(200);

const TYPE_NAME: &str = "std_msgs::msg::dds_::UInt8MultiArray_";
const TYPE_HASH: &str = "RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385";

/// The tokens whose entity name is `topic`, which has no leading slash.
fn tokens_of(session: &zenoh::Session, topic: &str) -> Vec<String> {
    let needle = format!("/%{}/", topic.replace('/', "%"));
    tokens(session)
        .into_iter()
        .filter(|k| k.contains(&needle))
        .collect()
}

/// The tokens of `topic`, once at least one is visible, or none after the deadline.
fn wait_tokens(session: &zenoh::Session, topic: &str) -> Vec<String> {
    let deadline = Instant::now() + RECV;
    loop {
        let hits = tokens_of(session, topic);
        if !hits.is_empty() || Instant::now() >= deadline {
            return hits;
        }
        std::thread::sleep(POLL);
    }
}

/// The QoS text of the one token of `topic`. A token has the QoS as its last segment, except that a
/// subscriber that joins a buffer group adds a `backends:` segment after it.
fn qos_text(session: &zenoh::Session, topic: &str) -> String {
    let hits = wait_tokens(session, topic);
    assert_eq!(hits.len(), 1, "expected one token for {topic}: {hits:?}");
    hits[0]
        .split('/')
        .rfind(|segment| !segment.starts_with("backends:"))
        .expect("a token has segments")
        .to_owned()
}

/// What a token carries for `qos`.
fn encoded(qos: &QosProfile) -> String {
    qos.to_protocol_qos().encode()
}

/// The QoS the tests ask for: not the default in any field that the token shows.
fn non_default() -> QosProfile {
    QosProfile {
        reliability: QosReliability::BestEffort,
        durability: QosDurability::TransientLocal,
        history: QosHistory::KeepLast(std::num::NonZeroUsize::new(7).unwrap()),
        ..Default::default()
    }
}

/// A v2 struct that asks for `non_default()`.
fn v2_non_default() -> CQosProfileV2 {
    let mut qos = init_v2();
    qos.reliability = 1;
    qos.durability = 1;
    qos.history = 0;
    qos.history_depth = 7;
    qos
}

fn init_v2() -> CQosProfileV2 {
    let mut qos = std::mem::MaybeUninit::<CQosProfileV2>::uninit();
    let rc = unsafe {
        hiroz_qos_profile_v2_init(qos.as_mut_ptr(), std::mem::size_of::<CQosProfileV2>())
    };
    assert_eq!(rc, 0, "hiroz_qos_profile_v2_init failed");
    unsafe { qos.assume_init() }
}

/// A context and a node made through the C ABI, as a C caller makes them.
struct CFixture {
    ctx: *mut CContext,
    node: *mut CNode,
}

impl CFixture {
    fn new(endpoint: &str, name: &str) -> Self {
        let endpoint = CString::new(endpoint).unwrap();
        let endpoints = [endpoint.as_ptr()];
        let mode = CString::new("client").unwrap();
        let config = CContextConfig {
            domain_id: 0,
            config_file: std::ptr::null(),
            connect_endpoints: endpoints.as_ptr(),
            connect_endpoints_count: 1,
            mode: mode.as_ptr(),
            disable_multicast_scouting: true,
            connect_to_local_zenohd: false,
            json_config: std::ptr::null(),
            remap_rules: std::ptr::null(),
            remap_rules_count: 0,
            enable_logging: false,
            namespace: std::ptr::null(),
        };
        let name = CString::new(name).unwrap();
        unsafe {
            let ctx = hiroz_context_create_with_config(&config);
            assert!(!ctx.is_null(), "the C context was not created");
            let node = hiroz_node_create(ctx, name.as_ptr(), std::ptr::null());
            assert!(!node.is_null(), "the C node was not created");
            Self { ctx, node }
        }
    }

    /// `hiroz_publisher_create_with_qos_v2` on `/topic`. Null on failure.
    fn publisher_v2(&self, topic: &str, qos: *const CQosProfileV2) -> *mut CPublisher {
        let topic = CString::new(format!("/{topic}")).unwrap();
        let type_name = CString::new(TYPE_NAME).unwrap();
        let type_hash = CString::new(TYPE_HASH).unwrap();
        unsafe {
            hiroz_publisher_create_with_qos_v2(
                self.node,
                topic.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                qos,
            )
        }
    }

    /// `hiroz_publisher_create_with_qos` on `/topic`. Null on failure.
    fn publisher_v1(&self, topic: &str, qos: *const CQosProfile) -> *mut CPublisher {
        let topic = CString::new(format!("/{topic}")).unwrap();
        let type_name = CString::new(TYPE_NAME).unwrap();
        let type_hash = CString::new(TYPE_HASH).unwrap();
        unsafe {
            hiroz_publisher_create_with_qos(
                self.node,
                topic.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                qos,
            )
        }
    }

    /// `hiroz_subscriber_create_with_qos_v2` on `/topic`. Null on failure.
    fn subscriber_v2(&self, topic: &str, qos: *const CQosProfileV2) -> *mut CSubscriber {
        extern "C" fn ignore(_user_data: usize, _data: *const u8, _len: usize) {}
        let topic = CString::new(format!("/{topic}")).unwrap();
        let type_name = CString::new(TYPE_NAME).unwrap();
        let type_hash = CString::new(TYPE_HASH).unwrap();
        unsafe {
            hiroz_subscriber_create_with_qos_v2(
                self.node,
                topic.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                ignore,
                0,
                qos,
            )
        }
    }

    /// `hiroz_service_client_create_with_qos_v2` on `/service`. Null on failure.
    fn service_client_v2(&self, service: &str, qos: *const CQosProfileV2) -> *mut CServiceClient {
        let service = CString::new(format!("/{service}")).unwrap();
        let type_name = CString::new("example_interfaces::srv::dds_::AddTwoInts_").unwrap();
        let type_hash = CString::new("").unwrap();
        unsafe {
            hiroz_service_client_create_with_qos_v2(
                self.node,
                service.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                qos,
            )
        }
    }

    /// `hiroz_service_server_create_with_qos_v2` on `/service`. Null on failure.
    fn service_server_v2(&self, service: &str, qos: *const CQosProfileV2) -> *mut CServiceServer {
        extern "C" fn refuse(
            _user_data: usize,
            _request: *const u8,
            _request_len: usize,
            _response: *mut *mut u8,
            _response_len: *mut usize,
        ) -> i32 {
            1
        }
        let service = CString::new(format!("/{service}")).unwrap();
        let type_name = CString::new("example_interfaces::srv::dds_::AddTwoInts_").unwrap();
        let type_hash = CString::new("").unwrap();
        unsafe {
            hiroz_service_server_create_with_qos_v2(
                self.node,
                service.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                refuse,
                0,
                qos,
            )
        }
    }

    /// `hiroz_action_client_create_with_qos_v2` on `/action`. Null on failure.
    fn action_client_v2(&self, action: &str, qos: *const CActionQosV2) -> *mut CActionClient {
        let action = CString::new(format!("/{action}")).unwrap();
        let (type_name, goal, result, feedback, hash) = action_names();
        unsafe {
            hiroz_action_client_create_with_qos_v2(
                self.node,
                action.as_ptr(),
                type_name.as_ptr(),
                goal.as_ptr(),
                hash.as_ptr(),
                result.as_ptr(),
                hash.as_ptr(),
                feedback.as_ptr(),
                hash.as_ptr(),
                qos,
            )
        }
    }

    /// `hiroz_action_server_create_with_qos_v2` on `/action`. Null on failure.
    fn action_server_v2(&self, action: &str, qos: *const CActionQosV2) -> *mut CActionServer {
        extern "C" fn reject(_user_data: usize, _goal: *const u8, _goal_len: usize) -> i32 {
            0
        }
        extern "C" fn fail(
            _user_data: usize,
            _goal_id: *const u8,
            _goal: *const u8,
            _goal_len: usize,
            _result: *mut *mut u8,
            _result_len: *mut usize,
        ) -> i32 {
            1
        }
        let action = CString::new(format!("/{action}")).unwrap();
        let (type_name, goal, result, feedback, hash) = action_names();
        unsafe {
            hiroz_action_server_create_with_qos_v2(
                self.node,
                action.as_ptr(),
                type_name.as_ptr(),
                goal.as_ptr(),
                hash.as_ptr(),
                result.as_ptr(),
                hash.as_ptr(),
                feedback.as_ptr(),
                hash.as_ptr(),
                reject,
                fail,
                0,
                qos,
            )
        }
    }
}

/// The type names and the empty hash that the action creators take.
fn action_names() -> (CString, CString, CString, CString, CString) {
    (
        CString::new("example_interfaces/action/Fibonacci").unwrap(),
        CString::new("example_interfaces/action/Fibonacci_Goal").unwrap(),
        CString::new("example_interfaces/action/Fibonacci_Result").unwrap(),
        CString::new("example_interfaces/action/Fibonacci_Feedback").unwrap(),
        CString::new("").unwrap(),
    )
}

impl Drop for CFixture {
    fn drop(&mut self) {
        unsafe {
            hiroz_node_destroy(self.node);
            hiroz_context_destroy(self.ctx);
        }
    }
}

/// The init function writes the size of the struct first and the same defaults that a null pointer
/// asks for.
#[test]
fn c_abi_qos_v2_init_writes_size_and_defaults() {
    let qos = init_v2();
    assert_eq!(
        qos.struct_size as usize,
        std::mem::size_of::<CQosProfileV2>()
    );
    let converted = unsafe { CQosProfileV2::to_qos_profile(&qos) }
        .expect("an initialised struct is valid")
        .expect("a non-null pointer gives a profile");
    assert_eq!(converted, QosProfile::default());

    let null = unsafe {
        hiroz_qos_profile_v2_init(std::ptr::null_mut(), std::mem::size_of::<CQosProfileV2>())
    };
    assert_ne!(null, 0, "init of a null pointer must fail");
}

/// A v2 publisher announces the QoS that the struct asks for, and so does a v1 publisher that asks
/// for the same. The default publisher is the control: its token differs.
#[test]
fn c_abi_publisher_qos_v2_reaches_the_token() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_pub");
    assert_ne!(
        encoded(&non_default()),
        encoded(&QosProfile::default()),
        "the control would not tell the two apart"
    );

    let qos = v2_non_default();
    let v2 = c.publisher_v2("c_qos_v2_pub_v2", &qos);
    let v1_qos = CQosProfile {
        reliability: 1,
        durability: 1,
        history: 0,
        history_depth: 7,
        deadline_sec: qos.deadline_sec,
        deadline_nsec: qos.deadline_nsec,
        lifespan_sec: qos.lifespan_sec,
        lifespan_nsec: qos.lifespan_nsec,
        liveliness: 0,
        liveliness_lease_sec: qos.liveliness_lease_sec,
        liveliness_lease_nsec: qos.liveliness_lease_nsec,
    };
    let v1 = c.publisher_v1("c_qos_v2_pub_v1", &v1_qos);
    let null = c.publisher_v2("c_qos_v2_pub_null", std::ptr::null());
    assert!(
        !v2.is_null() && !v1.is_null() && !null.is_null(),
        "a creator returned null"
    );

    assert_eq!(qos_text(&raw, "c_qos_v2_pub_v2"), encoded(&non_default()));
    assert_eq!(qos_text(&raw, "c_qos_v2_pub_v1"), encoded(&non_default()));
    assert_eq!(
        qos_text(&raw, "c_qos_v2_pub_null"),
        encoded(&QosProfile::default())
    );

    for publisher in [v2, v1, null] {
        unsafe { hiroz_publisher_destroy(publisher) };
    }
}

/// A caller built against a shorter struct sets `struct_size` below the full size. hiroz reads the
/// fields inside that size and takes the default for the rest.
#[test]
fn c_abi_qos_v2_reads_only_the_fields_inside_struct_size() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_prefix");

    // `durability` and `history_depth` hold a non-default value, outside the size given.
    let mut qos = v2_non_default();
    qos.struct_size = std::mem::offset_of!(CQosProfileV2, durability) as u32;
    let publisher = c.publisher_v2("c_qos_v2_prefix_pub", &qos);
    assert!(!publisher.is_null(), "the creator returned null");

    let expected = QosProfile {
        reliability: QosReliability::BestEffort,
        ..Default::default()
    };
    assert_eq!(qos_text(&raw, "c_qos_v2_prefix_pub"), encoded(&expected));

    unsafe { hiroz_publisher_destroy(publisher) };
}

/// A `struct_size` that cannot hold the field itself is a bug in the caller, often an
/// uninitialised struct. The creator returns null and creates no entity.
#[test]
fn c_abi_qos_v2_rejects_a_struct_size_below_the_size_field() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_short");

    for (i, size) in [0u32, 3].into_iter().enumerate() {
        let mut qos = v2_non_default();
        qos.struct_size = size;
        let topic = format!("c_qos_v2_short_{i}");
        let publisher = c.publisher_v2(&topic, &qos);
        assert!(publisher.is_null(), "struct_size {size} must be rejected");
        assert!(
            tokens_of(&raw, &topic).is_empty(),
            "a rejected creator left a token for {topic}"
        );
    }
}

/// A v2 subscriber announces the QoS that the struct asks for. The subscriber is volatile: a
/// transient-local subscriber never joins the buffer group, so it declares no token.
#[test]
fn c_abi_subscriber_qos_v2_reaches_the_token() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_sub");

    let qos = v2_with(|q| {
        q.reliability = 1;
        q.history_depth = 7;
    });
    let expected = encoded(&QosProfile {
        reliability: QosReliability::BestEffort,
        history: QosHistory::KeepLast(std::num::NonZeroUsize::new(7).unwrap()),
        ..Default::default()
    });
    assert_ne!(expected, encoded(&QosProfile::default()));
    let sub = c.subscriber_v2("c_qos_v2_sub_topic", &qos);
    assert!(!sub.is_null(), "the creator returned null");

    assert_eq!(qos_text(&raw, "c_qos_v2_sub_topic"), expected);

    unsafe { hiroz_subscriber_destroy(sub) };
}

/// A v2 service client and a v2 service server announce the QoS that the struct asks for. The
/// service does not change how a call is delivered. The token is where a peer reads the QoS.
#[test]
fn c_abi_service_qos_v2_reaches_the_token() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_srv");

    let qos = v2_non_default();
    let client = c.service_client_v2("c_qos_v2_srv_client", &qos);
    let server = c.service_server_v2("c_qos_v2_srv_server", &qos);
    let default_client = c.service_client_v2("c_qos_v2_srv_default", std::ptr::null());
    assert!(
        !client.is_null() && !server.is_null() && !default_client.is_null(),
        "a creator returned null"
    );

    assert_eq!(
        qos_text(&raw, "c_qos_v2_srv_client"),
        encoded(&non_default())
    );
    assert_eq!(
        qos_text(&raw, "c_qos_v2_srv_server"),
        encoded(&non_default())
    );
    assert_eq!(
        qos_text(&raw, "c_qos_v2_srv_default"),
        encoded(&QosProfile::default())
    );

    unsafe {
        hiroz_service_client_destroy(client);
        hiroz_service_client_destroy(default_client);
        hiroz_service_server_destroy(server);
    }
}

/// A service creator rejects a `struct_size` below the size field, as the publisher creator does.
#[test]
fn c_abi_service_qos_v2_rejects_a_short_struct_size() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_srv_short");

    let mut qos = v2_non_default();
    qos.struct_size = 0;
    let client = c.service_client_v2("c_qos_v2_srv_short_client", &qos);
    let server = c.service_server_v2("c_qos_v2_srv_short_server", &qos);
    assert!(
        client.is_null(),
        "the client creator accepted struct_size 0"
    );
    assert!(
        server.is_null(),
        "the server creator accepted struct_size 0"
    );
    for topic in ["c_qos_v2_srv_short_client", "c_qos_v2_srv_short_server"] {
        assert!(
            tokens_of(&raw, topic).is_empty(),
            "a rejected creator left a token for {topic}"
        );
    }
}

/// A v2 profile that differs from the default in one way that the token shows.
fn v2_with(edit: impl FnOnce(&mut CQosProfileV2)) -> CQosProfileV2 {
    let mut qos = init_v2();
    edit(&mut qos);
    qos
}

/// The five profiles of an action, each different, and what a token carries for each.
struct ActionProfiles {
    goal: CQosProfileV2,
    result: CQosProfileV2,
    cancel: CQosProfileV2,
    feedback: CQosProfileV2,
    status: CQosProfileV2,
}

impl ActionProfiles {
    fn new() -> Self {
        Self {
            goal: v2_with(|q| q.reliability = 1),
            result: v2_with(|q| q.durability = 1),
            cancel: v2_with(|q| q.history_depth = 7),
            feedback: v2_with(|q| q.history = 1),
            status: v2_with(|q| {
                q.reliability = 1;
                q.history_depth = 3;
            }),
        }
    }

    /// The struct that points at the five profiles. It must not outlive `self`.
    fn as_struct(&self) -> CActionQosV2 {
        CActionQosV2 {
            struct_size: std::mem::size_of::<CActionQosV2>() as u32,
            goal_service: &self.goal,
            result_service: &self.result,
            cancel_service: &self.cancel,
            feedback_topic: &self.feedback,
            status_topic: &self.status,
        }
    }
}

fn expected_goal() -> String {
    encoded(&QosProfile {
        reliability: QosReliability::BestEffort,
        ..Default::default()
    })
}

fn expected_result() -> String {
    encoded(&QosProfile {
        durability: QosDurability::TransientLocal,
        ..Default::default()
    })
}

fn expected_cancel() -> String {
    encoded(&QosProfile {
        history: QosHistory::KeepLast(std::num::NonZeroUsize::new(7).unwrap()),
        ..Default::default()
    })
}

fn expected_feedback() -> String {
    encoded(&QosProfile {
        history: QosHistory::KeepAll,
        ..Default::default()
    })
}

fn expected_status() -> String {
    encoded(&QosProfile {
        reliability: QosReliability::BestEffort,
        history: QosHistory::KeepLast(std::num::NonZeroUsize::new(3).unwrap()),
        ..Default::default()
    })
}

/// The five QoS values of an action are all different from each other and from the default. A
/// value that lands on the wrong entity therefore shows.
#[test]
fn c_abi_action_qos_values_are_distinct() {
    let all = [
        expected_goal(),
        expected_result(),
        expected_cancel(),
        expected_feedback(),
        expected_status(),
        encoded(&QosProfile::default()),
    ];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b, "the control cannot tell two entities apart");
        }
    }
}

/// An action server announces each of its five QoS profiles on the entity it belongs to.
#[test]
fn c_abi_action_server_qos_v2_reaches_each_token() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_server");

    let profiles = ActionProfiles::new();
    let qos = profiles.as_struct();
    let server = c.action_server_v2("c_qos_v2_act_server_name", &qos);
    assert!(!server.is_null(), "the creator returned null");

    let name = "c_qos_v2_act_server_name/_action";
    assert_eq!(
        qos_text(&raw, &format!("{name}/send_goal")),
        expected_goal()
    );
    assert_eq!(
        qos_text(&raw, &format!("{name}/get_result")),
        expected_result()
    );
    assert_eq!(
        qos_text(&raw, &format!("{name}/cancel_goal")),
        expected_cancel()
    );
    assert_eq!(
        qos_text(&raw, &format!("{name}/feedback")),
        expected_feedback()
    );
    assert_eq!(qos_text(&raw, &format!("{name}/status")), expected_status());

    unsafe { hiroz_action_server_destroy(server) };
}

/// An action client announces the QoS of its three services and of its status subscriber. Its
/// feedback subscriber stays out of the buffer group, so it declares no token and a peer cannot
/// read the feedback QoS.
#[test]
fn c_abi_action_client_qos_v2_reaches_each_token() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_client");

    let profiles = ActionProfiles::new();
    let qos = profiles.as_struct();
    let client = c.action_client_v2("c_qos_v2_act_client_name", &qos);
    assert!(!client.is_null(), "the creator returned null");

    let name = "c_qos_v2_act_client_name/_action";
    assert_eq!(
        qos_text(&raw, &format!("{name}/send_goal")),
        expected_goal()
    );
    assert_eq!(
        qos_text(&raw, &format!("{name}/get_result")),
        expected_result()
    );
    assert_eq!(
        qos_text(&raw, &format!("{name}/cancel_goal")),
        expected_cancel()
    );
    assert_eq!(qos_text(&raw, &format!("{name}/status")), expected_status());
    assert!(
        tokens_of(&raw, &format!("{name}/feedback")).is_empty(),
        "an action client declared a token for feedback"
    );

    unsafe { hiroz_action_client_destroy(client) };
}

/// The `status_topic` member reaches the status subscriber of an action client. Three clients
/// control each other: a member with a distinct profile, a member with the default profile, and
/// no member. The first announces the distinct QoS, so the value is not lost. The second announces
/// the default, so the QoS is not forced to the distinct value. The third creates no status
/// subscriber, as an action client without the member never did.
#[test]
fn c_abi_action_client_status_qos_v2_reaches_the_status_subscriber() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_status");

    let profiles = ActionProfiles::new();
    let distinct = profiles.as_struct();
    let default_status = init_v2();
    let mut with_default = profiles.as_struct();
    with_default.status_topic = &default_status;
    let mut without = profiles.as_struct();
    without.status_topic = std::ptr::null();

    let a = c.action_client_v2("c_qos_v2_act_status_distinct", &distinct);
    let b = c.action_client_v2("c_qos_v2_act_status_default", &with_default);
    let n = c.action_client_v2("c_qos_v2_act_status_none", &without);
    assert!(
        !a.is_null() && !b.is_null() && !n.is_null(),
        "a creator returned null"
    );

    assert_eq!(
        qos_text(&raw, "c_qos_v2_act_status_distinct/_action/status"),
        expected_status()
    );
    assert_ne!(expected_status(), encoded(&QosProfile::default()));
    assert_eq!(
        qos_text(&raw, "c_qos_v2_act_status_default/_action/status"),
        encoded(&QosProfile::default())
    );
    // The services of the third client are visible, so a missing status token is not a dead
    // client.
    assert_eq!(
        qos_text(&raw, "c_qos_v2_act_status_none/_action/send_goal"),
        expected_goal()
    );
    assert!(
        tokens_of(&raw, "c_qos_v2_act_status_none/_action/status").is_empty(),
        "an action client without a status member declared a status token"
    );

    unsafe {
        hiroz_action_client_destroy(a);
        hiroz_action_client_destroy(b);
        hiroz_action_client_destroy(n);
    }
}

/// A null pointer, and a null member, ask for the default profile.
#[test]
fn c_abi_action_qos_v2_null_means_default() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_null");

    let profiles = ActionProfiles::new();
    let mut partial = profiles.as_struct();
    partial.result_service = std::ptr::null();
    partial.cancel_service = std::ptr::null();
    partial.feedback_topic = std::ptr::null();
    partial.status_topic = std::ptr::null();
    let with_goal_only = c.action_client_v2("c_qos_v2_act_null_partial", &partial);
    let with_null = c.action_client_v2("c_qos_v2_act_null_all", std::ptr::null());
    assert!(
        !with_goal_only.is_null() && !with_null.is_null(),
        "a creator returned null"
    );

    let default = encoded(&QosProfile::default());
    let partial_name = "c_qos_v2_act_null_partial/_action";
    assert_eq!(
        qos_text(&raw, &format!("{partial_name}/send_goal")),
        expected_goal()
    );
    assert_eq!(
        qos_text(&raw, &format!("{partial_name}/get_result")),
        default
    );
    let null_name = "c_qos_v2_act_null_all/_action";
    for entity in ["send_goal", "get_result", "cancel_goal"] {
        assert_eq!(qos_text(&raw, &format!("{null_name}/{entity}")), default);
    }
    for name in [partial_name, null_name] {
        assert!(
            tokens_of(&raw, &format!("{name}/status")).is_empty(),
            "a null status member made a status token on {name}"
        );
    }

    unsafe {
        hiroz_action_client_destroy(with_goal_only);
        hiroz_action_client_destroy(with_null);
    }
}

/// A caller built against a shorter action struct sets `struct_size` below the full size. The
/// profiles beyond that size are not read, so they take the default.
#[test]
fn c_abi_action_qos_v2_reads_only_the_members_inside_struct_size() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_prefix");

    let profiles = ActionProfiles::new();
    let mut qos = profiles.as_struct();
    qos.struct_size = std::mem::offset_of!(CActionQosV2, result_service) as u32;
    let client = c.action_client_v2("c_qos_v2_act_prefix_name", &qos);
    assert!(!client.is_null(), "the creator returned null");

    let name = "c_qos_v2_act_prefix_name/_action";
    assert_eq!(
        qos_text(&raw, &format!("{name}/send_goal")),
        expected_goal()
    );
    assert_eq!(
        qos_text(&raw, &format!("{name}/get_result")),
        encoded(&QosProfile::default())
    );
    assert!(
        tokens_of(&raw, &format!("{name}/status")).is_empty(),
        "a status member outside struct_size made a status token"
    );

    unsafe { hiroz_action_client_destroy(client) };
}

/// An action creator rejects a `struct_size` below the size field, and so does it for a member
/// profile that has one. Neither leaves an entity behind.
#[test]
fn c_abi_action_qos_v2_rejects_a_short_struct_size() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_short");

    let profiles = ActionProfiles::new();
    let mut outer = profiles.as_struct();
    outer.struct_size = 0;
    let outer_client = c.action_client_v2("c_qos_v2_act_short_outer", &outer);
    assert!(
        outer_client.is_null(),
        "the outer struct_size 0 was accepted"
    );

    let mut bad_member = ActionProfiles::new();
    bad_member.cancel.struct_size = 0;
    let member = bad_member.as_struct();
    let member_server = c.action_server_v2("c_qos_v2_act_short_member", &member);
    assert!(
        member_server.is_null(),
        "a member with struct_size 0 was accepted"
    );

    for name in ["c_qos_v2_act_short_outer", "c_qos_v2_act_short_member"] {
        assert!(
            tokens_of(&raw, &format!("{name}/_action/send_goal")).is_empty(),
            "a rejected creator left a token for {name}"
        );
    }
}

/// A byte buffer with the alignment of the structs, to build a struct by hand. Every byte starts
/// as `0x01`: a member that a creator reads from it without a check is a pointer to address
/// `0x0101...`, or to address 1 when only one byte of it is copied.
#[repr(C, align(8))]
struct RawStruct([u8; 128]);

impl RawStruct {
    fn filled(struct_size: u32) -> Self {
        let mut raw = Self([0x01; 128]);
        raw.0[..4].copy_from_slice(&struct_size.to_ne_bytes());
        raw
    }

    fn put(&mut self, offset: usize, bytes: &[u8]) {
        self.0[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn action(&self) -> *const CActionQosV2 {
        self.0.as_ptr().cast()
    }
}

/// An action struct whose `struct_size` ends inside the first pointer member. The member bytes
/// are garbage, not null. A member that is not whole inside `struct_size` is not present, so the
/// creator must use the default for every entity and must not read the member.
#[test]
fn c_abi_action_qos_v2_ignores_a_partial_first_member() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_partial");

    let one_byte = (std::mem::offset_of!(CActionQosV2, goal_service) + 1) as u32;
    let partial = RawStruct::filled(one_byte);
    let client = c.action_client_v2("c_qos_v2_act_partial_client", partial.action());
    let server = c.action_server_v2("c_qos_v2_act_partial_server", partial.action());
    assert!(
        !client.is_null() && !server.is_null(),
        "a creator returned null"
    );

    let default = encoded(&QosProfile::default());
    for name in ["c_qos_v2_act_partial_client", "c_qos_v2_act_partial_server"] {
        for entity in ["send_goal", "get_result", "cancel_goal"] {
            assert_eq!(
                qos_text(&raw, &format!("{name}/_action/{entity}")),
                default,
                "{name} {entity}"
            );
        }
    }
    assert!(
        tokens_of(&raw, "c_qos_v2_act_partial_client/_action/status").is_empty(),
        "a partial status member made a status token"
    );

    unsafe {
        hiroz_action_client_destroy(client);
        hiroz_action_server_destroy(server);
    }
}

/// An action struct whose `struct_size` ends inside the last member. The members before it are
/// whole and are read: the goal profile is the positive control. The last member is garbage and
/// partly inside the size: it counts as null, so the client creates no status subscriber.
#[test]
fn c_abi_action_qos_v2_ignores_a_partial_last_member() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_act_partial_last");

    let profiles = ActionProfiles::new();
    let full = profiles.as_struct();
    let mut partial = RawStruct::filled(0);
    let before_status = std::mem::offset_of!(CActionQosV2, status_topic);
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&full as *const CActionQosV2).cast::<u8>(),
            partial.0.as_mut_ptr(),
            before_status,
        );
    }
    partial.put(0, &((before_status + 3) as u32).to_ne_bytes());
    let client = c.action_client_v2("c_qos_v2_act_partial_last_name", partial.action());
    assert!(!client.is_null(), "the creator returned null");

    let name = "c_qos_v2_act_partial_last_name/_action";
    assert_eq!(
        qos_text(&raw, &format!("{name}/send_goal")),
        expected_goal()
    );
    assert!(
        tokens_of(&raw, &format!("{name}/status")).is_empty(),
        "a partial status member made a status token"
    );

    unsafe { hiroz_action_client_destroy(client) };
}

/// A profile struct whose `struct_size` ends inside the `reliability` member. The low bytes that
/// are inside the size hold the value 1. A scalar that is not whole inside the size is not
/// present, so every entry point uses the default. A struct that holds `reliability` whole is the
/// positive control and announces best effort.
#[test]
fn c_abi_qos_v2_ignores_a_partial_scalar_member() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_scalar");

    let mut whole = init_v2();
    whole.reliability = 1;
    whole.struct_size = std::mem::offset_of!(CQosProfileV2, durability) as u32;
    let mut partial = init_v2();
    partial.reliability = 1;
    partial.struct_size = (std::mem::offset_of!(CQosProfileV2, reliability) + 2) as u32;

    let default = encoded(&QosProfile::default());
    let best_effort = encoded(&QosProfile {
        reliability: QosReliability::BestEffort,
        ..Default::default()
    });
    assert_ne!(default, best_effort);

    let p_whole = c.publisher_v2("c_qos_v2_scalar_pub_whole", &whole);
    let p_partial = c.publisher_v2("c_qos_v2_scalar_pub_partial", &partial);
    let s_whole = c.subscriber_v2("c_qos_v2_scalar_sub_whole", &whole);
    let s_partial = c.subscriber_v2("c_qos_v2_scalar_sub_partial", &partial);
    let c_whole = c.service_client_v2("c_qos_v2_scalar_cli_whole", &whole);
    let c_partial = c.service_client_v2("c_qos_v2_scalar_cli_partial", &partial);
    let v_whole = c.service_server_v2("c_qos_v2_scalar_srv_whole", &whole);
    let v_partial = c.service_server_v2("c_qos_v2_scalar_srv_partial", &partial);
    assert!(
        ![p_whole, p_partial].iter().any(|p| p.is_null())
            && ![s_whole, s_partial].iter().any(|p| p.is_null())
            && ![c_whole, c_partial].iter().any(|p| p.is_null())
            && ![v_whole, v_partial].iter().any(|p| p.is_null()),
        "a creator returned null"
    );

    for kind in ["pub", "sub", "cli", "srv"] {
        assert_eq!(
            qos_text(&raw, &format!("c_qos_v2_scalar_{kind}_whole")),
            best_effort,
            "{kind} with a whole member"
        );
        assert_eq!(
            qos_text(&raw, &format!("c_qos_v2_scalar_{kind}_partial")),
            default,
            "{kind} with a partial member"
        );
    }

    unsafe {
        hiroz_publisher_destroy(p_whole);
        hiroz_publisher_destroy(p_partial);
        hiroz_subscriber_destroy(s_whole);
        hiroz_subscriber_destroy(s_partial);
        hiroz_service_client_destroy(c_whole);
        hiroz_service_client_destroy(c_partial);
        hiroz_service_server_destroy(v_whole);
        hiroz_service_server_destroy(v_partial);
    }
}

/// A buffer longer than the struct, filled with a canary byte, and the init function called with a
/// size. The function writes at most `size` bytes: the bytes after it must keep the canary.
fn init_into_canary(size: usize) -> (RawStruct, i32) {
    const CANARY: u8 = 0xCC;
    let mut buffer = RawStruct([CANARY; 128]);
    assert!(std::mem::size_of::<CQosProfileV2>() + 16 <= buffer.0.len());
    let rc = unsafe { hiroz_qos_profile_v2_init(buffer.0.as_mut_ptr().cast(), size) };
    (buffer, rc)
}

fn word(buffer: &RawStruct, offset: usize) -> u32 {
    u32::from_ne_bytes(buffer.0[offset..offset + 4].try_into().unwrap())
}

/// An old program that was built against a header with a short struct passes its own size. The
/// init function writes only the whole fields inside that size, and records that size.
#[test]
fn c_abi_qos_v2_init_writes_no_more_than_the_size_given() {
    let reliability = std::mem::offset_of!(CQosProfileV2, reliability);
    let history = std::mem::offset_of!(CQosProfileV2, history);
    let durability = std::mem::offset_of!(CQosProfileV2, durability);

    // Whole fields: the size field and `reliability`. The bytes after them keep the canary.
    let (buffer, rc) = init_into_canary(durability);
    assert_eq!(rc, 0);
    assert_eq!(word(&buffer, 0) as usize, durability);
    assert_eq!(word(&buffer, reliability), 0);
    assert!(
        buffer.0[durability..].iter().all(|b| *b == 0xCC),
        "init wrote past the size given: {:?}",
        &buffer.0[durability..]
    );

    // A size that cuts `durability` in two: the field is not written, not even in part.
    let (buffer, rc) = init_into_canary(durability + 2);
    assert_eq!(rc, 0);
    assert_eq!(word(&buffer, 0) as usize, durability + 2);
    assert!(
        buffer.0[durability..].iter().all(|b| *b == 0xCC),
        "init wrote part of a field: {:?}",
        &buffer.0[durability..]
    );

    // Four fields fit: the history field is the last one that is written.
    let (buffer, rc) = init_into_canary(history + 4);
    assert_eq!(rc, 0);
    assert_eq!(word(&buffer, history), 0);
    assert!(buffer.0[history + 4..].iter().all(|b| *b == 0xCC));
}

/// A new program against an older library, and the reverse, both reduce to this: the size the
/// caller gives can be larger than the struct that the library knows. The library writes the
/// fields that it knows, and leaves the rest of the caller's allocation alone.
#[test]
fn c_abi_qos_v2_init_leaves_bytes_beyond_the_known_struct() {
    let known = std::mem::size_of::<CQosProfileV2>();
    let (buffer, rc) = init_into_canary(known + 16);
    assert_eq!(rc, 0);
    assert_eq!(word(&buffer, 0) as usize, known + 16);
    assert!(
        buffer.0[known..known + 16].iter().all(|b| *b == 0xCC),
        "init wrote beyond the struct that it knows"
    );
    let converted = unsafe { CQosProfileV2::to_qos_profile(buffer.0.as_ptr().cast()) }
        .expect("a larger struct_size is valid")
        .expect("a non-null pointer gives a profile");
    assert_eq!(converted, QosProfile::default());
}

/// A size below the size field cannot hold anything. The function fails and writes nothing.
#[test]
fn c_abi_qos_v2_init_rejects_a_size_below_the_size_field() {
    for size in [0usize, 3] {
        let (buffer, rc) = init_into_canary(size);
        assert_ne!(rc, 0, "size {size} must be rejected");
        assert!(
            buffer.0.iter().all(|b| *b == 0xCC),
            "a rejected init wrote bytes for size {size}"
        );
    }
}
