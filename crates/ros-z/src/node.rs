use std::{sync::Arc, time::Duration};

use tracing::{debug, info, warn};
use zenoh::{Result, Session, Wait, liveliness::LivelinessToken};

#[cfg(feature = "ffi")]
use crate::ffi::publisher::RawPublisher;
use crate::{
    Builder, ServiceTypeInfo, WithTypeInfo,
    action::{client::ZActionClientBuilder, server::ZActionServerBuilder},
    cache::ZCacheBuilder,
    context::{GlobalCounter, RemapRules},
    dynamic::{
        DiscoveredTopicSchema, DynPubBuilder, DynSub, DynSubBuilder, DynamicMessage,
        DynamicSerdeCdrSerdes, MessageSchema, SchemaDiscovery, TypeDescriptionService,
        discovered_schema_type_info, schema_type_info,
    },
    entity::*,
    graph::Graph,
    msg::{ZMessage, ZService},
    parameter::{
        Parameter, ParameterDescriptor, ParameterValue, SetParametersResult,
        service::{ParameterService, ParameterServiceConfig},
    },
    pubsub::{ZPubBuilder, ZSubBuilder},
    service::{ZClientBuilder, ZServerBuilder},
};

/// A ROS 2-style node: a named participant that owns publishers, subscribers,
/// service clients, service servers, and action clients/servers.
///
/// Create a node via [`ZContext::create_node`](crate::context::ZContext::create_node):
///
/// ```rust,ignore
/// use ros_z::prelude::*;
///
/// let ctx = ZContextBuilder::default().build()?;
/// let node = ctx.create_node("my_node").build()?;
/// ```
pub struct ZNode {
    pub(crate) entity: NodeEntity,
    pub(crate) session: Arc<Session>,
    counter: Arc<GlobalCounter>,
    pub(crate) graph: Arc<Graph>,
    pub(crate) remap_rules: RemapRules,
    _lv_token: LivelinessToken,
    pub(crate) clock: crate::time::ZClock,
    pub(crate) shm_config: Option<Arc<crate::shm::ShmConfig>>,
    pub(crate) keyexpr_format: ros_z_protocol::KeyExprFormat,
    /// Optional type description service for this node.
    /// Enabled via `ZNodeBuilder::with_type_description_service()`.
    /// The service uses callback mode and requires no background task.
    type_desc_service: Option<TypeDescriptionService>,
    /// Parameter service providing ROS 2-compatible parameter management.
    /// Enabled by default; disable via `ZNodeBuilder::without_parameters()`.
    parameter_service: Option<ParameterService>,
}

impl std::fmt::Debug for ZNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZNode")
            .field("entity", &self.entity)
            .finish_non_exhaustive()
    }
}

pub struct ZNodeBuilder {
    pub(crate) domain_id: usize,
    pub(crate) name: String,
    pub(crate) namespace: String,
    pub(crate) enclave: String,
    pub(crate) session: Arc<Session>,
    pub(crate) counter: Arc<GlobalCounter>,
    pub(crate) graph: Arc<Graph>,
    pub(crate) remap_rules: RemapRules,
    pub(crate) clock: crate::time::ZClock,
    pub(crate) shm_config: Option<Arc<crate::shm::ShmConfig>>,
    pub(crate) keyexpr_format: ros_z_protocol::KeyExprFormat,
    /// Whether to enable the type description service for this node.
    pub(crate) enable_type_desc_service: bool,
    /// Whether to enable parameter services for this node (default: true).
    pub(crate) enable_parameters: bool,
    /// Initial parameter overrides applied at declaration time.
    pub(crate) parameter_overrides: std::collections::HashMap<String, ParameterValue>,
}

impl ZNodeBuilder {
    pub fn with_namespace<S: AsRef<str>>(mut self, namespace: S) -> Self {
        self.namespace = normalize_node_namespace(namespace.as_ref());
        self
    }

    /// Override SHM configuration for this node (and its publishers).
    ///
    /// This overrides the context-level SHM configuration for all publishers
    /// created from this node.
    ///
    /// # Example
    /// ```no_run
    /// use ros_z::shm::{ShmConfig, ShmProviderBuilder};
    /// use ros_z::Builder;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = ros_z::context::ZContextBuilder::default().build()?;
    /// let provider = Arc::new(ShmProviderBuilder::new(20 * 1024 * 1024).build()?);
    /// let config = ShmConfig::new(provider).with_threshold(5_000);
    ///
    /// let node = ctx.create_node("my_node")
    ///     .with_shm_config(config)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_config(mut self, config: crate::shm::ShmConfig) -> Self {
        self.shm_config = Some(Arc::new(config));
        self
    }

    /// Enable the type description service for this node.
    ///
    /// When enabled, the node will expose a `~get_type_description` service
    /// that allows other nodes to query type descriptions for schemas
    /// registered with this node's publishers.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let node = ctx
    ///     .create_node("my_node")
    ///     .with_type_description_service()
    ///     .build()?;
    ///
    /// // Dynamic publishers auto-register schemas.
    /// // Static publishers also auto-register when their message type provides
    /// // MessageTypeInfo::message_schema() (e.g. generated ros-z messages).
    /// ```
    pub fn with_type_description_service(mut self) -> Self {
        self.enable_type_desc_service = true;
        self
    }

    /// Disable the parameter services for this node.
    ///
    /// By default, every node exposes the standard ROS 2 parameter services
    /// (`~get_parameters`, `~set_parameters`, etc.) and publishes on
    /// `/parameter_events`. Call this to opt out.
    pub fn without_parameters(mut self) -> Self {
        self.enable_parameters = false;
        self
    }

    /// Set initial parameter overrides for this node.
    ///
    /// When a parameter is declared, if an override exists for its name, the
    /// override value replaces the default. This is equivalent to passing
    /// `--ros-args -p name:=value` on the command line in rclcpp.
    pub fn with_parameter_overrides(
        mut self,
        overrides: std::collections::HashMap<String, ParameterValue>,
    ) -> Self {
        self.parameter_overrides = overrides;
        self
    }

    /// Load initial parameter values from a ROS 2-style YAML file.
    ///
    /// The file is parsed for parameters matching this node's fully-qualified
    /// name (`/{namespace}/{node_name}` or `/{node_name}` for root namespace).
    /// Wildcard selectors (`/**`) also match all nodes.
    ///
    /// Values loaded from file are applied as overrides at declaration time.
    /// If both a file and `with_parameter_overrides` are used, the last call wins.
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn with_parameter_file(
        mut self,
        path: &std::path::Path,
    ) -> std::result::Result<Self, String> {
        let node_fqn = if self.namespace.is_empty() || self.namespace == "/" {
            format!("/{}", self.name)
        } else {
            format!("{}/{}", self.namespace, self.name)
        };

        let overrides = crate::parameter::yaml::load_parameter_file(path, &node_fqn)?;
        self.parameter_overrides.extend(overrides);
        Ok(self)
    }
}

impl Builder for ZNodeBuilder {
    type Output = ZNode;

    #[tracing::instrument(name = "node_build", skip(self), fields(
        name = %self.name,
        namespace = %self.namespace,
        id = tracing::field::Empty
    ))]
    fn build(self) -> Result<ZNode> {
        let id = self.counter.increment();
        tracing::Span::current().record("id", id);

        debug!(
            "[NOD] Creating node: {}/{}, id={}",
            self.namespace, self.name, id
        );

        let node = NodeEntity::new(
            self.domain_id,
            self.session.zid(),
            id,
            self.name.clone(),
            self.namespace.clone(),
            self.enclave,
        );
        let lv_token_ke = crate::entity::node_lv_token_key_expr(&node)?;
        debug!("[NOD] Liveliness token KE: {}", lv_token_ke);

        let lv_token = self
            .session
            .liveliness()
            .declare_token(lv_token_ke)
            .wait()?;

        // Create type description service if enabled
        let type_desc_service = if self.enable_type_desc_service {
            debug!("[NOD] Creating type description service");
            let service = TypeDescriptionService::new(
                self.session.clone(),
                &self.name,
                &self.namespace,
                id,
                &self.counter,
                &self.clock,
            )?;

            info!("[NOD] TypeDescriptionService created (callback mode)");

            Some(service)
        } else {
            None
        };

        // Create parameter service if enabled (default)
        let parameter_service = if self.enable_parameters {
            debug!("[NOD] Creating parameter service");
            let service = ParameterService::new(ParameterServiceConfig {
                session: self.session.clone(),
                graph: self.graph.clone(),
                node_name: &self.name,
                namespace: &self.namespace,
                node_id: id,
                counter: &self.counter,
                clock: &self.clock,
                overrides: self.parameter_overrides,
            })?;
            info!("[NOD] ParameterService created");
            Some(service)
        } else {
            None
        };

        debug!("[NOD] Node ready: {}/{}", self.namespace, self.name);

        Ok(ZNode {
            entity: node,
            session: self.session,
            counter: self.counter,
            _lv_token: lv_token,
            graph: self.graph,
            remap_rules: self.remap_rules,
            clock: self.clock,
            shm_config: self.shm_config,
            keyexpr_format: self.keyexpr_format,
            type_desc_service,
            parameter_service,
        })
    }
}

impl ZNode {
    /// Create a publisher for the given topic.
    ///
    /// If `T` implements [`WithTypeInfo`], type information is automatically populated.
    /// If this node has type description service enabled and `T` provides a runtime
    /// schema via [`crate::MessageTypeInfo::message_schema`], that schema is
    /// automatically registered for `GetTypeDescription` discovery.
    ///
    /// The topic name will be qualified according to ROS 2 rules:
    /// - Absolute topics (starting with '/') are used as-is
    /// - Private topics (starting with '~') are expanded to /<namespace>/<node_name>/<topic>
    /// - Relative topics are expanded to /<namespace>/<topic>
    pub fn create_pub<T>(&self, topic: &str) -> ZPubBuilder<T, T::Serdes>
    where
        T: ZMessage + WithTypeInfo,
    {
        debug!("[NOD] Creating publisher: topic={}", topic);
        let mut builder = self.create_pub_impl(topic, Some(T::type_info()));

        // Call with_dyn_schema regardless of type_desc_service: its is_none() guard
        // prevents overwriting the type_info already set by T::type_info() above.
        match T::message_schema() {
            Some(schema) => {
                self.register_schema_with_type_description_service(&schema);
                builder = builder.with_dyn_schema(schema);
            }
            None => {
                debug!(
                    "[NOD] No static schema provided for {}, skipping type description registration",
                    std::any::type_name::<T>()
                );
            }
        }

        builder
    }

    #[doc(hidden)]
    pub fn create_pub_impl<T>(
        &self,
        topic: &str,
        type_info: Option<crate::entity::TypeInfo>,
    ) -> ZPubBuilder<T, T::Serdes>
    where
        T: ZMessage,
    {
        // Note: Topic qualification happens in ZPubBuilder::build()
        // to allow error handling in the Result type
        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            kind: EndpointKind::Publisher,
            topic: topic.to_string(),
            type_info,
            qos: Default::default(),
        };
        ZPubBuilder {
            entity,
            session: self.session.clone(),
            graph: self.graph.clone(),
            clock: self.clock.clone(),
            with_attachment: true,
            shm_config: self.shm_config.clone(),
            keyexpr_format: self.keyexpr_format,
            dyn_schema: None,
            encoding: None,
            _phantom_data: Default::default(),
        }
    }

    /// Create a subscriber for the given topic
    /// If T implements WithTypeInfo, type information will be automatically populated
    ///
    /// The topic name will be qualified according to ROS 2 rules:
    /// - Absolute topics (starting with '/') are used as-is
    /// - Private topics (starting with '~') are expanded to /<namespace>/<node_name>/<topic>
    /// - Relative topics are expanded to /<namespace>/<topic>
    pub fn create_sub<T>(&self, topic: &str) -> ZSubBuilder<T, T::Serdes>
    where
        T: ZMessage + WithTypeInfo,
    {
        debug!("[NOD] Creating subscriber: topic={}", topic);
        self.create_sub_impl(topic, Some(T::type_info()))
    }

    #[doc(hidden)]
    pub fn create_sub_impl<T>(
        &self,
        topic: &str,
        type_info: Option<crate::entity::TypeInfo>,
    ) -> ZSubBuilder<T, T::Serdes>
    where
        T: ZMessage,
    {
        // Note: Topic qualification happens in ZSubBuilder::build()
        // to allow error handling in the Result type
        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            kind: EndpointKind::Subscription,
            topic: topic.to_string(),
            type_info,
            qos: Default::default(),
        };
        ZSubBuilder {
            entity,
            session: self.session.clone(),
            graph: self.graph.clone(),
            keyexpr_format: self.keyexpr_format,
            dyn_schema: None,
            locality: None,
            expected_encoding: None,
            _phantom_data: Default::default(),
        }
    }

    /// Create a timestamp-indexed sliding-window cache subscriber for `topic`,
    /// retaining up to `capacity` messages.
    ///
    /// By default, messages are indexed by the Zenoh transport timestamp
    /// (zero-config, works for any message type). Call
    /// [`.with_stamp(|msg| ...)`](ZCacheBuilder::with_stamp) on the returned
    /// builder to switch to application-level timestamp extraction (e.g.
    /// `header.stamp`).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use ros_z::prelude::*;
    /// use ros_z_msgs::sensor_msgs::Imu;
    /// use std::time::{Duration, SystemTime};
    ///
    /// let ctx = ZContextBuilder::default().build()?;
    /// let node = ctx.create_node("cache_demo").build()?;
    ///
    /// // Zero-config (Zenoh transport timestamp)
    /// let cache = node.create_cache::<Imu>("/imu/data", 200).build()?;
    ///
    /// // Pull messages from the last 100 ms
    /// let now = SystemTime::now();
    /// let msgs = cache.get_interval(now - Duration::from_millis(100), now);
    /// ```
    pub fn create_cache<T>(&self, topic: &str, capacity: usize) -> ZCacheBuilder<T, T::Serdes>
    where
        T: ZMessage + WithTypeInfo,
    {
        debug!(
            "[NOD] Creating cache: topic={}, capacity={}",
            topic, capacity
        );
        let sub_builder = self.create_sub_impl(topic, Some(T::type_info()));
        ZCacheBuilder::new(sub_builder, capacity)
    }

    /// Create a service for the given service name
    /// If T is a tuple (Req, Resp) where both implement WithTypeInfo, type information will be automatically populated
    ///
    /// The service name will be qualified according to ROS 2 rules:
    /// - Absolute service names (starting with '/') are used as-is
    /// - Private service names (starting with '~') are expanded to /<namespace>/<node_name>/<service>
    /// - Relative service names are expanded to /<namespace>/<service>
    pub fn create_service<T>(&self, name: &str) -> ZServerBuilder<T>
    where
        T: ZService + ServiceTypeInfo,
    {
        debug!("[NOD] Creating service: name={}", name);
        self.create_service_impl(name, Some(T::service_type_info()))
    }

    #[doc(hidden)]
    pub fn create_service_impl<T>(
        &self,
        name: &str,
        type_info: Option<crate::entity::TypeInfo>,
    ) -> ZServerBuilder<T> {
        // Note: Service name qualification happens in ZServerBuilder::build()
        // to allow error handling in the Result type
        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            kind: EndpointKind::Service,
            topic: name.to_string(),
            type_info,
            qos: Default::default(),
        };
        ZServerBuilder {
            entity,
            session: self.session.clone(),
            clock: self.clock.clone(),
            keyexpr_format: self.keyexpr_format,
            _phantom_data: Default::default(),
        }
    }

    /// Create a client for the given service name
    /// If T is a tuple (Req, Resp) where both implement WithTypeInfo, type information will be automatically populated
    ///
    /// The service name will be qualified according to ROS 2 rules:
    /// - Absolute service names (starting with '/') are used as-is
    /// - Private service names (starting with '~') are expanded to /<namespace>/<node_name>/<service>
    /// - Relative service names are expanded to /<namespace>/<service>
    pub fn create_client<T>(&self, name: &str) -> ZClientBuilder<T>
    where
        T: ZService + ServiceTypeInfo,
    {
        debug!("[NOD] Creating client: name={}", name);
        self.create_client_impl(name, Some(T::service_type_info()))
    }

    #[doc(hidden)]
    pub fn create_client_impl<T>(
        &self,
        name: &str,
        type_info: Option<crate::entity::TypeInfo>,
    ) -> ZClientBuilder<T> {
        // Note: Service name qualification happens in ZClientBuilder::build()
        // to allow error handling in the Result type
        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            kind: EndpointKind::Client,
            topic: name.to_string(),
            type_info,
            qos: Default::default(),
        };
        ZClientBuilder {
            entity,
            session: self.session.clone(),
            clock: self.clock.clone(),
            keyexpr_format: self.keyexpr_format,
            querier_timeout: Duration::from_secs(10),
            _phantom_data: Default::default(),
        }
    }

    /// Create a raw publisher for FFI (no type safety)
    #[cfg(feature = "ffi")]
    pub fn create_raw_publisher(
        &self,
        topic: &str,
        type_name: &str,
        type_hash: &str,
    ) -> Result<RawPublisher> {
        self.create_raw_publisher_with_qos(topic, type_name, type_hash, None)
    }

    /// Create a raw publisher for FFI with optional QoS
    #[cfg(feature = "ffi")]
    pub fn create_raw_publisher_with_qos(
        &self,
        topic: &str,
        type_name: &str,
        type_hash: &str,
        qos: Option<crate::qos::QosProfile>,
    ) -> Result<RawPublisher> {
        use zenoh::qos::CongestionControl;
        use zenoh_ext::AdvancedPublisherBuilderExt;

        use crate::{
            entity::{EndpointEntity, EndpointKind},
            pubsub::apply_transient_local_pub,
            topic_name,
        };

        let qualified_topic =
            topic_name::qualify_topic_name(topic, &self.entity.namespace, &self.entity.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        let protocol_qos = qos.map(|q| q.to_protocol_qos()).unwrap_or_default();

        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            kind: EndpointKind::Publisher,
            topic: qualified_topic.clone(),
            type_info: Some(TypeInfo {
                name: type_name.to_string(),
                hash: TypeHash::from_rihs_string(type_hash).unwrap_or(TypeHash::zero()),
            }),
            qos: protocol_qos,
        };

        let topic_ke = self.keyexpr_format.topic_key_expr(&entity)?;
        let gid =
            crate::entity::endpoint_gid(&entity).expect("local endpoint always has node identity");
        let publisher = self
            .session
            .declare_publisher((*topic_ke).clone())
            .congestion_control(CongestionControl::Block)
            .advanced();
        let publisher = apply_transient_local_pub(publisher, &entity.qos).wait()?;

        // Declare liveliness token so rmw_zenoh_cpp can discover this publisher.
        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        Ok(RawPublisher::new(publisher, gid, lv_token))
    }

    /// Create a raw subscriber for FFI (no type safety)
    /// Returns a RawSubscriber that must be kept alive as long as the subscription is active
    #[cfg(feature = "ffi")]
    pub fn create_raw_subscriber<F>(
        &self,
        topic: &str,
        type_name: &str,
        type_hash: &str,
        callback: F,
    ) -> Result<crate::ffi::subscriber::RawSubscriber>
    where
        F: Fn(&[u8]) + Send + Sync + 'static,
    {
        self.create_raw_subscriber_with_qos(topic, type_name, type_hash, callback, None)
    }

    /// Create a raw subscriber for FFI with optional QoS
    #[cfg(feature = "ffi")]
    pub fn create_raw_subscriber_with_qos<F>(
        &self,
        topic: &str,
        type_name: &str,
        type_hash: &str,
        callback: F,
        qos: Option<crate::qos::QosProfile>,
    ) -> Result<crate::ffi::subscriber::RawSubscriber>
    where
        F: Fn(&[u8]) + Send + Sync + 'static,
    {
        use crate::{
            entity::{EndpointEntity, EndpointKind},
            pubsub::apply_transient_local_sub,
            topic_name,
        };
        use zenoh_ext::AdvancedSubscriberBuilderExt;

        let qualified_topic =
            topic_name::qualify_topic_name(topic, &self.entity.namespace, &self.entity.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        let protocol_qos = qos.map(|q| q.to_protocol_qos()).unwrap_or_default();

        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            kind: EndpointKind::Subscription,
            topic: qualified_topic.clone(),
            type_info: Some(TypeInfo {
                name: type_name.to_string(),
                hash: TypeHash::from_rihs_string(type_hash).unwrap_or(TypeHash::zero()),
            }),
            qos: protocol_qos,
        };

        let topic_ke = self.keyexpr_format.topic_key_expr(&entity)?;
        let subscriber = self
            .session
            .declare_subscriber((*topic_ke).clone())
            .callback(move |sample| {
                let payload = sample.payload().to_bytes();
                callback(&payload);
            })
            .advanced();
        let subscriber = apply_transient_local_sub(subscriber, &entity.qos).wait()?;

        Ok(crate::ffi::subscriber::RawSubscriber { inner: subscriber })
    }

    /// Create a raw service client for FFI (no type safety)
    #[cfg(feature = "ffi")]
    pub fn create_raw_service_client(
        &self,
        service: &str,
        type_name: &str,
        type_hash: &str,
    ) -> Result<crate::ffi::service::RawServiceClient> {
        use crate::entity::{EndpointEntity, EndpointKind};
        use crate::topic_name;
        use std::sync::atomic::AtomicUsize;

        let qualified_service =
            topic_name::qualify_service_name(service, &self.entity.namespace, &self.entity.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify service: {}", e)))?;

        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            topic: qualified_service.clone(),
            kind: EndpointKind::Client,
            type_info: Some(TypeInfo {
                name: type_name.to_string(),
                hash: TypeHash::from_rihs_string(type_hash).unwrap_or(TypeHash::zero()),
            }),
            qos: Default::default(),
        };

        let topic_ke = self.keyexpr_format.topic_key_expr(&entity)?;
        let key_expr: zenoh::key_expr::KeyExpr<'static> = (*topic_ke).clone();

        let inner = self
            .session
            .declare_querier(key_expr.clone())
            .target(zenoh::query::QueryTarget::AllComplete)
            .consolidation(zenoh::query::ConsolidationMode::None)
            .timeout(std::time::Duration::from_secs(10))
            .wait()?;

        let (tx, rx) = flume::bounded(10);

        // Declare liveliness token so rmw_zenoh_cpp service servers can observe this client.
        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        Ok(crate::ffi::service::RawServiceClient {
            sn: AtomicUsize::new(1),
            gid: crate::entity::endpoint_gid(&entity)
                .expect("service client always has node identity"),
            inner,
            tx,
            rx,
            _key_expr: key_expr,
            _lv_token: lv_token,
            qualified_service,
            graph: self.graph.clone(),
        })
    }

    /// Create a raw service server for FFI (no type safety)
    #[cfg(feature = "ffi")]
    pub fn create_raw_service_server(
        &self,
        service: &str,
        type_name: &str,
        type_hash: &str,
    ) -> Result<crate::ffi::service::RawServiceServer> {
        use crate::common::DataHandler;
        use crate::entity::{EndpointEntity, EndpointKind};
        use crate::topic_name;

        let qualified_service =
            topic_name::qualify_service_name(service, &self.entity.namespace, &self.entity.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify service: {}", e)))?;

        let entity = EndpointEntity {
            id: self.counter.increment(),
            node: Some(self.entity.clone()),
            topic: qualified_service.clone(),
            kind: EndpointKind::Service,
            type_info: Some(TypeInfo {
                name: type_name.to_string(),
                hash: TypeHash::from_rihs_string(type_hash).unwrap_or(TypeHash::zero()),
            }),
            qos: Default::default(),
        };

        let topic_ke = self.keyexpr_format.topic_key_expr(&entity)?;
        let key_expr: zenoh::key_expr::KeyExpr<'static> = (*topic_ke).clone();

        let queue = Arc::new(crate::queue::BoundedQueue::new(256));
        let queue_clone = queue.clone();
        let handler = DataHandler::Queue(queue_clone);

        let inner = self
            .session
            .declare_queryable(&key_expr)
            .complete(true)
            .callback(move |query| {
                handler.handle(query);
            })
            .wait()?;

        // Declare liveliness token so rmw_zenoh_cpp clients can discover this server.
        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        Ok(crate::ffi::service::RawServiceServer {
            key_expr,
            _inner: inner,
            _lv_token: lv_token,
            queue,
            map: std::collections::HashMap::new(),
        })
    }

    /// Create a raw action client for FFI (no type safety).
    /// Creates 3 service clients (SendGoal, GetResult, CancelGoal) + 1 feedback subscriber.
    #[cfg(feature = "ffi")]
    #[allow(clippy::too_many_arguments)]
    pub fn create_raw_action_client(
        &self,
        action_name: &str,
        action_type: &str,
        _goal_type: &str,
        goal_hash: &str,
        _result_type: &str,
        result_hash: &str,
        _feedback_type: &str,
        feedback_hash: &str,
    ) -> Result<crate::ffi::action::RawActionClient> {
        let send_goal_service = format!("{}/_action/send_goal", action_name);
        let get_result_service = format!("{}/_action/get_result", action_name);
        let cancel_goal_service = format!("{}/_action/cancel_goal", action_name);
        let feedback_topic = format!("{}/_action/feedback", action_name);

        // Compute DDS-style type names required by rmw_zenoh_cpp's graph discovery.
        // action_type is e.g. "example_interfaces/action/Fibonacci" → package="example_interfaces", name="Fibonacci"
        let (pkg, aname) = split_action_type(action_type);
        let send_goal_type = format!("{}::action::dds_::{}_SendGoal_", pkg, aname);
        let get_result_type = format!("{}::action::dds_::{}_GetResult_", pkg, aname);
        let cancel_goal_type = "action_msgs::srv::dds_::CancelGoal_";
        let feedback_type_dds = format!("{}::action::dds_::{}_FeedbackMessage_", pkg, aname);

        let send_goal_client =
            self.create_raw_service_client(&send_goal_service, &send_goal_type, goal_hash)?;
        let get_result_client =
            self.create_raw_service_client(&get_result_service, &get_result_type, result_hash)?;
        let cancel_goal_client =
            self.create_raw_service_client(&cancel_goal_service, cancel_goal_type, "")?;

        // Feedback subscriber (no-op callback for now; Go handles via polling or separate mechanism)
        let feedback_sub =
            self.create_raw_subscriber(&feedback_topic, &feedback_type_dds, feedback_hash, |_| {})?;

        Ok(crate::ffi::action::RawActionClient {
            send_goal_client,
            get_result_client,
            cancel_goal_client,
            _feedback_sub: feedback_sub,
        })
    }

    /// Create a raw action server for FFI (no type safety).
    /// Creates 3 service servers (SendGoal, GetResult, CancelGoal) + 2 publishers (Feedback, Status).
    #[cfg(feature = "ffi")]
    #[allow(clippy::too_many_arguments)]
    pub fn create_raw_action_server(
        &self,
        action_name: &str,
        action_type: &str,
        _goal_type: &str,
        goal_hash: &str,
        _result_type: &str,
        result_hash: &str,
        _feedback_type: &str,
        feedback_hash: &str,
    ) -> Result<crate::ffi::action::RawActionServer> {
        let send_goal_service = format!("{}/_action/send_goal", action_name);
        let get_result_service = format!("{}/_action/get_result", action_name);
        let cancel_goal_service = format!("{}/_action/cancel_goal", action_name);
        let feedback_topic = format!("{}/_action/feedback", action_name);
        let status_topic = format!("{}/_action/status", action_name);

        // Compute DDS-style type names required by rmw_zenoh_cpp's graph discovery.
        // action_type is e.g. "example_interfaces/action/Fibonacci" → package="example_interfaces", name="Fibonacci"
        let (pkg, aname) = split_action_type(action_type);
        let send_goal_type = format!("{}::action::dds_::{}_SendGoal_", pkg, aname);
        let get_result_type = format!("{}::action::dds_::{}_GetResult_", pkg, aname);
        let cancel_goal_type = "action_msgs::srv::dds_::CancelGoal_";
        let feedback_type_dds = format!("{}::action::dds_::{}_FeedbackMessage_", pkg, aname);
        let status_type_dds = "action_msgs::msg::dds_::GoalStatusArray_";

        let send_goal_server =
            self.create_raw_service_server(&send_goal_service, &send_goal_type, goal_hash)?;
        let get_result_server =
            self.create_raw_service_server(&get_result_service, &get_result_type, result_hash)?;
        let cancel_goal_server =
            self.create_raw_service_server(&cancel_goal_service, cancel_goal_type, "")?;

        let feedback_pub =
            self.create_raw_publisher(&feedback_topic, &feedback_type_dds, feedback_hash)?;
        let status_pub = self.create_raw_publisher(&status_topic, status_type_dds, "")?;

        Ok(crate::ffi::action::RawActionServer {
            send_goal_server,
            get_result_server,
            cancel_goal_server,
            feedback_pub,
            _status_pub: status_pub,
            pending_results: std::collections::HashMap::new(),
            pending_result_queries: std::collections::HashMap::new(),
        })
    }

    /// Create an action client for the given action name
    pub fn create_action_client<A>(&self, action_name: &str) -> ZActionClientBuilder<'_, A>
    where
        A: crate::action::ZAction,
    {
        ZActionClientBuilder::new(action_name, self)
    }

    /// Create an action server for the given action name
    pub fn create_action_server<A>(&self, action_name: &str) -> ZActionServerBuilder<'_, A>
    where
        A: crate::action::ZAction,
    {
        ZActionServerBuilder::new(action_name, self)
    }

    /// Get a reference to this node's type description service, if enabled.
    ///
    /// Returns `None` if the node was not created with `.with_type_description_service()`.
    pub fn type_description_service(&self) -> Option<&TypeDescriptionService> {
        self.type_desc_service.as_ref()
    }

    /// Get a mutable reference to this node's type description service, if enabled.
    ///
    /// Returns `None` if the node was not created with `.with_type_description_service()`.
    pub fn type_description_service_mut(&mut self) -> Option<&mut TypeDescriptionService> {
        self.type_desc_service.as_mut()
    }

    /// Check if this node has a type description service.
    pub fn has_type_description_service(&self) -> bool {
        self.type_desc_service.is_some()
    }

    /// Get access to the global counter for entity ID generation.
    pub fn counter(&self) -> &Arc<GlobalCounter> {
        &self.counter
    }

    /// Get the name of this node.
    pub fn name(&self) -> &str {
        &self.entity.name
    }

    /// Get the namespace of this node.
    pub fn namespace(&self) -> &str {
        &self.entity.namespace
    }

    /// Get a reference to the graph for this node.
    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }

    /// Get a reference to the node entity (for graph and liveliness operations).
    pub fn node_entity(&self) -> &NodeEntity {
        &self.entity
    }

    /// Apply remapping rules to a topic or action name.
    pub fn apply_remap(&self, name: &str) -> String {
        self.remap_rules.apply(name)
    }

    /// Get a reference to the underlying Zenoh session.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Access this node's clock.
    pub fn clock(&self) -> &crate::time::ZClock {
        &self.clock
    }

    // ========================================================================
    // Parameter API
    // ========================================================================

    /// Declare a parameter with a default value and descriptor.
    ///
    /// Returns the actual initial value (which may differ from `default` if an
    /// override was set via `ZNodeBuilder::with_parameter_overrides`).
    ///
    /// Returns an error if parameter services are disabled or the parameter
    /// is already declared.
    pub fn declare_parameter(
        &self,
        name: &str,
        default: ParameterValue,
        descriptor: ParameterDescriptor,
    ) -> std::result::Result<ParameterValue, String> {
        self.parameter_service
            .as_ref()
            .ok_or_else(|| "parameter services not enabled for this node".to_string())?
            .declare_parameter(name, default, descriptor)
    }

    /// Get the current value of a declared parameter.
    pub fn get_parameter(&self, name: &str) -> Option<ParameterValue> {
        self.parameter_service.as_ref()?.get_parameter(name)
    }

    /// Set the value of a declared parameter.
    ///
    /// Returns the result indicating success or failure with a reason.
    /// The change will be validated against the parameter's descriptor and
    /// any registered `on_set_parameters` callback.
    ///
    /// Setting a parameter to `ParameterValue::NotSet` keeps it declared; use
    /// [`ZNode::undeclare_parameter`] to remove it entirely.
    pub fn set_parameter(&self, param: Parameter) -> std::result::Result<(), String> {
        self.parameter_service
            .as_ref()
            .map(|s| s.set_parameter(param))
            .unwrap_or_else(|| Err("parameter services not enabled".to_string()))
    }

    /// Undeclare a previously declared parameter and publish a deleted event.
    pub fn undeclare_parameter(&self, name: &str) -> std::result::Result<(), String> {
        self.parameter_service
            .as_ref()
            .ok_or_else(|| "parameter services not enabled for this node".to_string())?
            .undeclare_parameter(name)
    }

    /// Get the descriptor for a declared parameter.
    pub fn describe_parameter(&self, name: &str) -> Option<ParameterDescriptor> {
        self.parameter_service.as_ref()?.describe_parameter(name)
    }

    /// Register a callback invoked before each parameter change is committed.
    ///
    /// The callback receives the proposed changes and returns a `SetParametersResult`.
    /// Return `SetParametersResult::failure(reason)` to reject the change.
    ///
    /// Only one callback can be registered; calling this again replaces the previous one.
    pub fn on_set_parameters<F>(&self, callback: F)
    where
        F: Fn(&[Parameter]) -> SetParametersResult + Send + Sync + 'static,
    {
        if let Some(ref svc) = self.parameter_service {
            svc.on_set_parameters(callback);
        }
    }

    /// Check if parameter services are enabled for this node.
    pub fn has_parameter_service(&self) -> bool {
        self.parameter_service.is_some()
    }

    // ========================================================================
    // Dynamic Message API
    // ========================================================================

    /// Create a dynamic publisher for the given topic.
    ///
    /// If this node has a type description service enabled, the schema will be
    /// automatically registered, allowing other nodes to discover it via the
    /// `GetTypeDescription` service.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name to publish on
    /// * `schema` - The message schema for serialization
    ///
    /// # Example
    ///
    /// ```ignore
    /// let schema = MessageSchema::builder("std_msgs/msg/String")
    ///     .field("data", FieldType::String)
    ///     .build()?;
    ///
    /// let publisher = node.create_dyn_pub("chatter", schema).build()?;
    ///
    /// let mut msg = DynamicMessage::new(publisher.schema());
    /// msg.set("data", "Hello, world!")?;
    /// publisher.publish(&msg)?;
    /// ```
    pub fn create_dyn_pub(&self, topic: &str, schema: Arc<MessageSchema>) -> DynPubBuilder {
        self.register_schema_with_type_description_service(&schema);
        self.create_dyn_pub_impl(topic, Some(schema_type_info(&schema)), schema)
    }

    /// Discover the schema that publishers currently expose on a topic.
    ///
    /// The topic name is qualified according to the same ROS 2 rules as the
    /// regular publisher and subscriber builder APIs.
    pub async fn discover_topic_schema(
        &self,
        topic: &str,
        discovery_timeout: Duration,
    ) -> Result<DiscoveredTopicSchema> {
        SchemaDiscovery::new(self, discovery_timeout)
            .discover(topic)
            .await
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Create a dynamic subscriber with automatic schema discovery.
    ///
    /// This method queries publishers on the topic for their type description
    /// and returns a ready-to-use subscriber. This is useful when you don't
    /// know the message type at compile time.
    ///
    /// For fine-grained control (e.g. custom QoS), use [`discover_topic_schema`]
    /// combined with [`create_dyn_sub`] instead.
    ///
    /// The topic name will be qualified according to ROS 2 rules:
    /// - Absolute topics (starting with '/') are used as-is
    /// - Private topics (starting with '~') are expanded to /<namespace>/<node_name>/<topic>
    /// - Relative topics are expanded to /<namespace>/<topic>
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name to subscribe to
    /// * `discovery_timeout` - How long to wait for schema discovery
    ///
    /// # Example
    ///
    /// ```ignore
    /// let subscriber = node.create_dyn_sub_auto("chatter", Duration::from_secs(5)).await?;
    ///
    /// println!("Discovered type: {}", subscriber.schema().unwrap().type_name);
    /// let msg = subscriber.recv()?;
    /// let data: String = msg.get("data")?;
    /// ```
    ///
    /// [`discover_topic_schema`]: ZNode::discover_topic_schema
    /// [`create_dyn_sub`]: ZNode::create_dyn_sub
    pub async fn create_dyn_sub_auto(
        &self,
        topic: &str,
        discovery_timeout: Duration,
    ) -> Result<DynSub> {
        debug!(
            "[NOD] Creating dynamic subscriber with auto-discovery for topic: {}",
            topic
        );

        let discovered = self.discover_topic_schema(topic, discovery_timeout).await?;

        info!(
            "[NOD] Discovered schema for topic {}: {} (hash: {})",
            discovered.qualified_topic, discovered.schema.type_name, discovered.type_hash
        );

        self.create_dyn_sub_impl(
            topic,
            Some(discovered_schema_type_info(&discovered)),
            discovered.schema,
        )
        .build()
    }

    /// Create a dynamic subscriber with a known schema.
    ///
    /// Use this when you already have the schema (e.g., loaded from a file
    /// or built programmatically).
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name to subscribe to
    /// * `schema` - The message schema for deserialization
    ///
    /// The topic name will be qualified according to ROS 2 rules:
    /// - Absolute topics (starting with '/') are used as-is
    /// - Private topics (starting with '~') are expanded to /<namespace>/<node_name>/<topic>
    /// - Relative topics are expanded to /<namespace>/<topic>
    ///
    /// # Example
    ///
    /// ```ignore
    /// let schema = MessageSchema::builder("std_msgs/msg/String")
    ///     .field("data", FieldType::String)
    ///     .build()?;
    ///
    /// let subscriber = node.create_dyn_sub("chatter", schema).build()?;
    /// let msg = subscriber.recv()?;
    /// ```
    pub fn create_dyn_sub(&self, topic: &str, schema: Arc<MessageSchema>) -> DynSubBuilder {
        self.create_dyn_sub_impl(topic, Some(schema_type_info(&schema)), schema)
    }

    fn create_dyn_pub_impl(
        &self,
        topic: &str,
        type_info: Option<crate::entity::TypeInfo>,
        schema: Arc<MessageSchema>,
    ) -> DynPubBuilder {
        self.create_pub_impl::<DynamicMessage>(topic, type_info)
            .with_serdes::<DynamicSerdeCdrSerdes>()
            .with_dyn_schema(schema)
    }

    fn create_dyn_sub_impl(
        &self,
        topic: &str,
        type_info: Option<crate::entity::TypeInfo>,
        schema: Arc<MessageSchema>,
    ) -> DynSubBuilder {
        self.create_sub_impl::<DynamicMessage>(topic, type_info)
            .with_serdes::<DynamicSerdeCdrSerdes>()
            .with_dyn_schema(schema)
    }

    fn register_schema_with_type_description_service(&self, schema: &Arc<MessageSchema>) {
        if let Some(service) = &self.type_desc_service {
            if let Err(error) = service.register_schema(Arc::clone(schema)) {
                warn!(
                    "[NOD] Failed to register schema {} with type description service: {}",
                    schema.type_name, error
                );
            } else {
                debug!(
                    "[NOD] Registered schema {} with type description service",
                    schema.type_name
                );
            }
        }
    }
}

/// Parse an action type string like `"example_interfaces/action/Fibonacci"` into
/// `(package, action_name)` — i.e., `("example_interfaces", "Fibonacci")`.
/// These are used to construct DDS-style type names for rmw_zenoh_cpp graph discovery.
#[cfg(feature = "ffi")]
fn split_action_type(action_type: &str) -> (&str, &str) {
    let pkg = action_type.split('/').next().unwrap_or(action_type);
    let name = action_type.split('/').next_back().unwrap_or(action_type);
    (pkg, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_entity_name_namespace() {
        let entity = NodeEntity::new(
            0,
            "1234567890abcdef1234567890abcdef".parse().unwrap(),
            0,
            "my_node".to_string(),
            "/my_ns".to_string(),
            String::new(),
        );
        assert_eq!(entity.name, "my_node");
        assert_eq!(entity.namespace, "/my_ns");
    }

    #[test]
    fn test_remap_rules_identity_when_empty() {
        let rules = RemapRules::default();
        assert_eq!(rules.apply("/foo"), "/foo");
    }
}
