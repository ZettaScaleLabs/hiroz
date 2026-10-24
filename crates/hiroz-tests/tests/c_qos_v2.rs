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
        context::{
            CContext, CContextConfig, hiroz_context_create_with_config, hiroz_context_destroy,
        },
        node::{CNode, hiroz_node_create, hiroz_node_destroy},
        publisher::{
            CPublisher, hiroz_publisher_create_with_qos, hiroz_publisher_create_with_qos_v2,
            hiroz_publisher_destroy,
        },
        qos::{CQosProfile, CQosProfileV2, hiroz_qos_profile_v2_init},
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
    let rc = unsafe { hiroz_qos_profile_v2_init(qos.as_mut_ptr()) };
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

    let null = unsafe { hiroz_qos_profile_v2_init(std::ptr::null_mut()) };
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

/// A v2 subscriber announces the QoS that the struct asks for.
#[test]
fn c_abi_subscriber_qos_v2_reaches_the_token() {
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_qos_v2_sub");

    let qos = v2_non_default();
    let sub = c.subscriber_v2("c_qos_v2_sub_topic", &qos);
    assert!(!sub.is_null(), "the creator returned null");

    assert_eq!(
        qos_text(&raw, "c_qos_v2_sub_topic"),
        encoded(&non_default())
    );

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
