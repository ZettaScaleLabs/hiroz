use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::{marker::PhantomData, sync::Arc};

use tracing::{debug, trace, warn};
use zenoh::liveliness::LivelinessToken;
use zenoh::{Result, Session, Wait, sample::Sample};

use crate::Builder;
use crate::attachment::{Attachment, GidArray};
use crate::common::DataHandler;
use crate::entity::{EndpointEntity, EndpointKind};
use crate::event::EventsManager;
use crate::graph::Graph;
use crate::local_bus::{Delivery, Published};
use crate::impl_with_type_info;
use crate::queue::BoundedQueue;
use crate::topic_name;

use crate::msg::{SerdeCdrSerdes, ZDeserializer, ZMessage, ZSerializer};
use crate::qos::QosProfile;
use hiroz_protocol::qos::{QosDurability, QosHistory, QosReliability};
use std::sync::Mutex;
use zenoh_ext::{
    AdvancedPublisher, AdvancedPublisherBuilder, AdvancedPublisherBuilderExt, AdvancedSubscriber,
    AdvancedSubscriberBuilder, AdvancedSubscriberBuilderExt, CacheConfig, HistoryConfig,
    MissDetectionConfig, RecoveryConfig,
};

/// Sporadic heartbeat period for TransientLocal+Reliable publishers.
/// Matches rmw_zenoh_cpp's `SAMPLE_MISS_DETECTION_HEARTBEAT_PERIOD`.
const SAMPLE_MISS_HEARTBEAT_PERIOD: Duration = Duration::from_millis(500);

/// Query timeout for TransientLocal subscribers' initial history fetch.
/// Matches rmw_zenoh_cpp's `query_timeout_ms = u64::max()` literally
/// (`Duration::from_millis(u64::MAX)`, not `Duration::MAX`, to avoid any
/// truncation surprises inside zenoh-ext's internal `as_millis()` paths).
const TRANSIENT_LOCAL_QUERY_TIMEOUT: Duration = Duration::from_millis(u64::MAX);

fn cache_depth_from_history(history: QosHistory) -> usize {
    // Mirrors rmw_zenoh_cpp's `QoS::best_available_qos` (`qos.cpp:107`):
    // a zero-valued depth (the rmw representation of "unspecified" or
    // KEEP_ALL) is rewritten to `RMW_ZENOH_DEFAULT_HISTORY_DEPTH` (42)
    // before being passed to `cache.max_samples`. The cap value lives
    // alongside hiroz's other QoS constants — see `crate::qos`.
    match history {
        QosHistory::KeepLast(d) if d > 0 => d,
        QosHistory::KeepLast(_) => crate::qos::KEEP_ALL_CACHE_DEPTH,
        QosHistory::KeepAll => crate::qos::KEEP_ALL_CACHE_DEPTH,
    }
}

/// Apply hiroz TransientLocal QoS to an `AdvancedPublisherBuilder`.
///
/// Mirrors rmw_zenoh_cpp's `rmw_publisher_data.cpp`: enables
/// `publisher_detection` + `cache`, and `sample_miss_detection` when
/// `Reliable`. No-op for `Volatile`.
pub(crate) fn apply_transient_local_pub<'a, 'b, 'c>(
    mut builder: AdvancedPublisherBuilder<'a, 'b, 'c>,
    qos: &hiroz_protocol::qos::QosProfile,
) -> AdvancedPublisherBuilder<'a, 'b, 'c> {
    if !matches!(qos.durability, QosDurability::TransientLocal) {
        return builder;
    }
    let depth = cache_depth_from_history(qos.history);
    builder = builder
        .publisher_detection()
        .cache(CacheConfig::default().max_samples(depth));
    if matches!(qos.reliability, QosReliability::Reliable) {
        builder = builder.sample_miss_detection(
            MissDetectionConfig::default().sporadic_heartbeat(SAMPLE_MISS_HEARTBEAT_PERIOD),
        );
    }
    builder
}

/// Apply hiroz TransientLocal QoS to an `AdvancedSubscriberBuilder`.
///
/// Mirrors rmw_zenoh_cpp's `rmw_subscription_data.cpp`: enables history
/// query with late-publisher detection, `subscriber_detection`, a
/// `u64::MAX`-millisecond query timeout, and heartbeat recovery when
/// `Reliable`. No-op for `Volatile`.
pub(crate) fn apply_transient_local_sub<'a, 'b, 'c, H>(
    mut builder: AdvancedSubscriberBuilder<'a, 'b, 'c, H>,
    qos: &hiroz_protocol::qos::QosProfile,
) -> AdvancedSubscriberBuilder<'a, 'b, 'c, H> {
    if !matches!(qos.durability, QosDurability::TransientLocal) {
        return builder;
    }
    let depth = cache_depth_from_history(qos.history);
    builder = builder
        .history(
            HistoryConfig::default()
                .detect_late_publishers()
                .max_samples(depth),
        )
        .query_timeout(TRANSIENT_LOCAL_QUERY_TIMEOUT)
        .subscriber_detection();
    if matches!(qos.reliability, QosReliability::Reliable) {
        builder = builder.recovery(RecoveryConfig::default().heartbeat());
    }
    builder
}

/// Which routes a publisher's messages take, resolved from what the caller has
/// asserted about the audience. See [`ZPub::route`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The bus alone. The publisher has declared it has no wire.
    BusOnly,
    /// Both, to disjoint audiences: the bus for this session, the wire for the rest.
    BusAndWire,
    /// The wire alone. The bus is not consulted.
    WireOnly,
}

/// A typed ROS 2-style publisher. Send messages with [`publish`](ZPub::publish)
/// (synchronous) or [`async_publish`](ZPub::async_publish) (async).
///
/// Create a publisher via [`ZNode::create_pub`](crate::node::ZNode::create_pub).
pub struct ZPub<T: ZMessage, S: ZSerializer> {
    pub entity: EndpointEntity,
    // TODO: replace this with the sample sn
    sn: AtomicUsize,
    // TODO: replace this with zenoh's global entity id
    gid: GidArray,
    inner: AdvancedPublisher<'static>,
    _lv_token: LivelinessToken,
    with_attachment: bool,
    clock: crate::time::ZClock,
    events_mgr: Arc<Mutex<EventsManager>>,
    shm_config: Option<Arc<crate::shm::ShmConfig>>,
    /// Schema for dynamic message publishing.
    pub dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Cached Zenoh encoding for this publisher (performance optimization).
    /// If set, this encoding will be used for all published messages.
    encoding: Option<Arc<zenoh::bytes::Encoding>>,
    graph: Arc<Graph>,
    /// Intra-process bus channel for this topic, resolved once at build time.
    /// See [`crate::local_bus`].
    local_channel: Arc<crate::local_bus::Channel>,
    /// When set, `publish_shared` delivers only to same-session subscribers and
    /// never touches zenoh. The caller's assertion that this publisher has no
    /// off-session audience — not inferred, because the graph cannot see a
    /// plain zenoh subscriber.
    intra_process_only: bool,
    /// The locality restriction applied to this publisher's wire half, if any.
    ///
    /// Routing consults it: a `Locality::Remote` wire half cannot reach this
    /// session, so the bus and the wire address disjoint audiences and both are
    /// used. Without it they overlap, and using both would deliver twice.
    locality: Option<zenoh::sample::Locality>,
    _phantom_data: PhantomData<(T, S)>,
}

impl<T: ZMessage, S: ZSerializer> std::fmt::Debug for ZPub<T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZPub")
            .field("entity", &self.entity)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ZPubBuilder<T, S = SerdeCdrSerdes<T>> {
    pub(crate) entity: EndpointEntity,
    pub(crate) session: Arc<Session>,
    pub(crate) graph: Arc<Graph>,
    pub(crate) clock: crate::time::ZClock,
    pub(crate) with_attachment: bool,
    pub(crate) shm_config: Option<Arc<crate::shm::ShmConfig>>,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    /// Schema for dynamic message publishing.
    /// When set, the schema will be registered with the type description service.
    pub(crate) dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Encoding format for this publisher.
    /// If set, all published messages will use this encoding.
    pub(crate) encoding: Option<crate::encoding::Encoding>,
    /// Locality restriction for published samples.
    /// `None` leaves zenoh's default (`Locality::Any`) in place.
    pub(crate) locality: Option<zenoh::sample::Locality>,
    /// Restrict `publish_shared` to the intra-process bus. See
    /// [`ZPubBuilder::with_intra_process_only`].
    pub(crate) intra_process_only: bool,
    pub(crate) _phantom_data: PhantomData<(T, S)>,
}

impl_with_type_info!(ZPubBuilder<T, S>);
impl_with_type_info!(ZSubBuilder<T, S>);

impl<T, S> ZPubBuilder<T, S> {
    pub fn with_qos(mut self, qos: QosProfile) -> Self {
        self.entity.qos = qos.to_protocol_qos();
        self
    }

    pub fn with_attachment(mut self, with_attachment: bool) -> Self {
        self.with_attachment = with_attachment;
        self
    }

    /// Override SHM configuration for this publisher only.
    ///
    /// This overrides any SHM configuration inherited from the node or context.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::shm::{ShmConfig, ShmProviderBuilder};
    /// use hiroz::Builder;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// let provider = Arc::new(ShmProviderBuilder::new(20 * 1024 * 1024).build()?);
    /// let config = ShmConfig::new(provider).with_threshold(5_000);
    ///
    /// let publisher = node.create_pub::<hiroz_msgs::std_msgs::String>("topic")
    ///     .with_shm_config(config)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_config(mut self, config: crate::shm::ShmConfig) -> Self {
        self.shm_config = Some(Arc::new(config));
        self
    }

    /// Disable SHM for this publisher.
    ///
    /// Even if SHM is enabled at the node or context level, this publisher
    /// will not use shared memory.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().with_shm_enabled()?.build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// // Context has SHM enabled, but disable for this publisher
    /// let publisher = node.create_pub::<hiroz_msgs::std_msgs::String>("small_messages")
    ///     .without_shm()
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn without_shm(mut self) -> Self {
        self.shm_config = None;
        self
    }

    /// Set the locality restriction for this publisher.
    ///
    /// This restricts which matching subscribers receive the published samples:
    /// those in the same session, those in other sessions, or both (the
    /// default).
    ///
    /// [`Locality::SessionLocal`](zenoh::sample::Locality::SessionLocal) is the intra-process fast path. Zenoh skips
    /// the transport entirely for it — no link, no wire encode, no shared
    /// memory — and hands the payload straight to the matching subscriber
    /// callbacks in this session. Every node created from one [`ZContext`]
    /// shares one zenoh session, so this reaches sibling nodes in the same
    /// process.
    ///
    /// [`ZContext`]: crate::context::ZContext
    ///
    /// # This makes the publisher invisible off-process
    ///
    /// A [`Locality::SessionLocal`](zenoh::sample::Locality::SessionLocal) publisher is not reachable from any other
    /// process, including `ros2 topic echo`. Set it only where the subscriber
    /// is known to live in the same context.
    ///
    /// # Serialization still happens
    ///
    /// Zenoh payloads are bytes, so the message is still CDR-encoded on publish
    /// and decoded on receive. This setting removes the transport, not the
    /// serialization. Publisher SHM is wasted work under it — pair it with
    /// [`without_shm`](Self::without_shm).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zenoh::sample::Locality;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// let publisher = node
    ///     .create_pub::<hiroz_msgs::std_msgs::String>("topic")
    ///     .with_locality(Locality::SessionLocal)
    ///     .without_shm()
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_locality(mut self, locality: zenoh::sample::Locality) -> Self {
        self.locality = Some(locality);
        self
    }

    /// **Prototype.** Restrict [`ZPub::publish_shared`] to the intra-process
    /// bus: the message is handed to same-session subscribers as an `Arc<T>`
    /// and never goes near zenoh, so it is neither serialized nor transmitted.
    ///
    /// This affects `publish_shared` only. [`ZPub::publish`] keeps its normal
    /// wire behaviour whatever this is set to.
    ///
    /// # Why the flag exists, and why it should not
    ///
    /// The caller asserts the audience; it is not inferred. Reading it off the
/// ROS graph was tried and withdrawn: a plain zenoh subscriber declares no
/// liveliness token, so the graph cannot see it, and a publisher that
/// concluded "everyone is local" from the graph dropped that subscriber's
/// messages with nothing reporting the loss.
    /// every other process, including `ros2 topic echo`, and to any local
    /// subscriber that did not register on the bus with
    /// [`ZSubBuilder::build_with_shared_callback`]. Set it only where both ends
    /// are known.
    /// # This publisher still appears in the ROS graph
    ///
    /// The liveliness token is declared in `build()` before any locality is
    /// consulted, so a publisher that can reach nothing outside this process
    /// still advertises itself as an ordinary ROS 2 endpoint. Every other node
    /// counts it: `ros2 topic info -v`, `ros2 node info`, a remote subscriber's
    /// matched-publisher event, and any `Graph::count` derived from them.
    /// `ros2 topic echo` shows the topic and never a message.
    ///
    /// Suppressing the token would make the counts honest and remove the
    /// publisher from `ros2 node info`, which is a change to graph visibility
    /// and belongs in its own change with its own test. Until then this is a
    /// disclosed limitation, not an oversight: a node debugging "the publisher
    /// is there but nothing arrives" has no signal separating this from a QoS
    /// mismatch.
    pub fn with_intra_process_only(mut self) -> Self {
        self.intra_process_only = true;
        self
    }

    pub fn with_serdes<S2>(self) -> ZPubBuilder<T, S2> {
        ZPubBuilder {
            entity: self.entity,
            session: self.session,
            graph: self.graph,
            clock: self.clock,
            with_attachment: self.with_attachment,
            shm_config: self.shm_config,
            keyexpr_format: self.keyexpr_format.clone(),
            dyn_schema: self.dyn_schema,
            encoding: self.encoding,
            locality: self.locality,
            intra_process_only: self.intra_process_only,
            _phantom_data: PhantomData,
        }
    }

    /// Set the encoding format for published messages.
    ///
    /// This encoding will be transmitted with each message, allowing subscribers
    /// to determine the serialization format at runtime.
    ///
    /// # Performance
    ///
    /// The Zenoh encoding is cached during `build()` to avoid repeated conversion
    /// overhead on every `publish()` call.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use hiroz::encoding::Encoding;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// // Publish with Protobuf encoding
    /// let publisher = node.create_pub::<hiroz_msgs::geometry_msgs::Point>("/topic")
    ///     .with_encoding(Encoding::protobuf().with_schema("geometry_msgs/msg/Point"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_encoding(mut self, encoding: crate::encoding::Encoding) -> Self {
        self.encoding = Some(encoding);
        self
    }

    /// Set the dynamic message schema for runtime-typed publishers.
    ///
    /// When a schema is set and the node has a type description service enabled,
    /// the schema will be automatically registered with the service during build.
    /// This allows other nodes to query for this type's description.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let publisher = node
    ///     .create_pub_impl::<DynamicMessage>("topic", None)
    ///     .with_serdes::<DynamicSerdeCdrSerdes>()
    ///     .with_dyn_schema(schema)
    ///     .build()?;
    /// ```
    pub fn with_dyn_schema(mut self, schema: Arc<crate::dynamic::schema::MessageSchema>) -> Self {
        // Only compute and set type_info if it hasn't been set already.
        // Typed publishers (create_pub) already have type_info set via T::type_info();
        // don't overwrite it with the schema-derived value.
        if self.entity.type_info.is_none() {
            self.entity.type_info = Some(crate::dynamic::schema_type_info(&schema));
        }

        self.dyn_schema = Some(schema);
        self
    }
}

impl<T, S> Builder for ZPubBuilder<T, S>
where
    T: ZMessage + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    type Output = ZPub<T, S>;

    #[tracing::instrument(name = "pub_build", skip(self), fields(
        topic = %self.entity.topic,
        qos_reliability = ?self.entity.qos.reliability,
        qos_durability = ?self.entity.qos.durability
    ))]
    fn build(mut self) -> Result<Self::Output> {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from("publisher build requires node identity"));
        };
        // Qualify the topic name according to ROS 2 rules
        let qualified_topic =
            topic_name::qualify_topic_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        self.entity.topic = qualified_topic.clone();
        debug!("[PUB] Qualified topic: {}", qualified_topic);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone(); // Deref and clone the KeyExpr
        debug!("[PUB] Key expression: {}", key_expr);

        // Map QoS to Zenoh publisher settings
        let mut pub_builder = self.session.declare_publisher(key_expr);

        // Map reliability: Reliable uses Block, BestEffort uses Drop
        match self.entity.qos.reliability {
            QosReliability::Reliable => {
                pub_builder = pub_builder.congestion_control(zenoh::qos::CongestionControl::Block);
                debug!("[PUB] QoS: Reliable (Block)");
            }
            QosReliability::BestEffort => {
                pub_builder = pub_builder.congestion_control(zenoh::qos::CongestionControl::Drop);
                debug!("[PUB] QoS: BestEffort (Drop)");
            }
        }

        // Apply the locality restriction before .advanced(); AdvancedPublisher
        // forwards its own destination onto this inner builder, so setting it
        // here is what survives.
        if let Some(locality) = self.locality {
            pub_builder = pub_builder.allowed_destination(locality);
            debug!("[PUB] Locality restriction: {:?}", locality);
        }

        // Build an AdvancedPublisher and apply TransientLocal config if needed.
        let pub_builder = pub_builder.advanced();
        debug!(
            "[PUB] Durability: {:?}, history: {:?}",
            self.entity.qos.durability, self.entity.qos.history
        );
        let pub_builder = apply_transient_local_pub(pub_builder, &self.entity.qos);
        let inner = pub_builder.wait()?;
        debug!("[PUB] Publisher ready: topic={}", self.entity.topic);

        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;
        let gid = crate::entity::endpoint_gid(&self.entity)
            .expect("local endpoint always has node identity");

        // Cache the Zenoh encoding if specified (performance optimization)
        let encoding = self.encoding.map(|enc| Arc::new(enc.to_zenoh_encoding()));

        if let Some(ref enc) = encoding {
            debug!("[PUB] Using encoding: {}", enc);
        }

        let local_channel = crate::local_bus::channel(self.session.zid(), &qualified_topic);

        Ok(ZPub {
            entity: self.entity,
            sn: AtomicUsize::new(0),
            inner,
            _lv_token: lv_token,
            gid,
            clock: self.clock,
            events_mgr: Arc::new(Mutex::new(EventsManager::new(gid))),
            with_attachment: self.with_attachment,
            shm_config: self.shm_config,
            dyn_schema: self.dyn_schema,
            encoding,
            graph: self.graph,
            local_channel,
            intra_process_only: self.intra_process_only,
            locality: self.locality,
            _phantom_data: Default::default(),
        })
    }
}

impl<T, S> ZPub<T, S>
where
    T: ZMessage + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    /// Wait until at least `count` subscribers are matched on this publisher's topic,
    /// or until `timeout` elapses.
    ///
    /// Returns `true` if the required number of subscribers appeared within the
    /// timeout, `false` otherwise.
    ///
    /// This mirrors rclcpp's `rcl_wait_for_subscribers()` pattern: the publisher
    /// registers a graph-change notification *before* sampling the subscriber count,
    /// so no arrival is missed between the check and the wait.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Ensure at least one subscriber is ready before publishing.
    /// assert!(publisher.wait_for_subscription(1, Duration::from_secs(5)).await);
    /// ```
    pub async fn wait_for_subscription(&self, count: usize, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Arm the notification *before* reading the count to avoid a TOCTOU
            // race where a subscriber arrives between the count check and the await.
            let notified = self.graph.change_notify.notified();
            tokio::pin!(notified);

            let n = self
                .graph
                .get_entities_by_topic(EndpointKind::Subscription, &self.entity.topic)
                .len();
            if n >= count {
                return true;
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }

            // Sleep until either a graph change fires or the deadline passes.
            if tokio::time::timeout(remaining, &mut notified)
                .await
                .is_err()
            {
                // Timeout — do one final check in case a late notification was missed.
                return self
                    .graph
                    .get_entities_by_topic(EndpointKind::Subscription, &self.entity.topic)
                    .len()
                    >= count;
            }
        }
    }

    fn new_attachment(&self) -> Attachment {
        let sn = self.sn.fetch_add(1, Ordering::Relaxed);
        trace!(
            "[PUB] Creating attachment: sn={}, gid={:02x?}",
            sn,
            &self.gid[..4]
        );
        Attachment::with_clock(sn as _, self.gid, &self.clock)
    }

    /// Serialize and publish `msg` on the topic. Blocks until the put completes.
    ///
    /// Use [`async_publish`](ZPub::async_publish) when calling from async code to
    /// avoid blocking the executor.
    #[tracing::instrument(name = "publish", skip(self, msg), fields(
        topic = %self.entity.topic,
        sn = self.sn.load(Ordering::Acquire),
        payload_len = tracing::field::Empty,
        used_shm = tracing::field::Empty
    ))]
    pub fn publish(&self, msg: &T) -> Result<()> {
        use zenoh_buffers::buffer::Buffer;

        // Try direct SHM serialization if configured
        let (zbuf, actual_size) = if let Some(ref shm_cfg) = self.shm_config {
            let estimated_size = msg.estimated_serialized_size();

            // Only use SHM if estimated size meets threshold
            if estimated_size >= shm_cfg.threshold() {
                match S::serialize_to_shm(msg, estimated_size, shm_cfg.provider()) {
                    Ok((zbuf, actual_size)) => {
                        tracing::Span::current().record("used_shm", true);
                        debug!(
                            "[PUB] Serialized {}B directly to SHM (estimated: {}B)",
                            actual_size, estimated_size
                        );
                        (zbuf, actual_size)
                    }
                    Err(e) => {
                        tracing::Span::current().record("used_shm", false);
                        warn!(
                            "[PUB] Direct SHM serialization failed: {}. Using regular memory",
                            e
                        );
                        let zbuf = S::serialize_to_zbuf(msg);
                        let size = zbuf.len();
                        (zbuf, size)
                    }
                }
            } else {
                tracing::Span::current().record("used_shm", false);
                trace!(
                    "[PUB] Estimated size {}B < threshold {}B, using regular memory",
                    estimated_size,
                    shm_cfg.threshold()
                );
                let zbuf = S::serialize_to_zbuf(msg);
                let size = zbuf.len();
                (zbuf, size)
            }
        } else {
            tracing::Span::current().record("used_shm", false);
            let zbuf = S::serialize_to_zbuf(msg);
            let size = zbuf.len();
            (zbuf, size)
        };

        tracing::Span::current().record("payload_len", actual_size);

        let zbytes = zenoh::bytes::ZBytes::from(zbuf);

        let mut put_builder = self.inner.put(zbytes);

        // Set encoding if configured (performance: uses cached Arc to avoid clone overhead)
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            let att = self.new_attachment();
            let sn = att.sequence_number;
            put_builder = put_builder.attachment(att);
            trace!("[PUB] Attached sn={}", sn);
        }

        put_builder.wait()
    }

    /// Serialize and publish `msg` on the topic. Yields to the async executor
    /// while the put is in progress, making this safe to call from within
    /// a Tokio task without blocking the thread.
    pub async fn async_publish(&self, msg: &T) -> Result<()> {
        // Try direct SHM serialization if configured
        let zbuf = if let Some(ref shm_cfg) = self.shm_config {
            let estimated_size = msg.estimated_serialized_size();

            if estimated_size >= shm_cfg.threshold() {
                match S::serialize_to_shm(msg, estimated_size, shm_cfg.provider()) {
                    Ok((zbuf, _actual_size)) => zbuf,
                    Err(_) => S::serialize_to_zbuf(msg),
                }
            } else {
                S::serialize_to_zbuf(msg)
            }
        } else {
            S::serialize_to_zbuf(msg)
        };

        let zbytes = zenoh::bytes::ZBytes::from(zbuf);
        let mut put_builder = self.inner.put(zbytes);

        // Set encoding if configured
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            put_builder = put_builder.attachment(self.new_attachment());
        }
        put_builder.await
    }

    /// **Prototype.** Publish an already-shared message without serializing it.
    ///
    /// Every subscriber in the same zenoh session that registered through
    /// [`ZSubBuilder::build_with_shared_callback`] for this exact `T` receives a
    /// clone of the `Arc` — a refcount bump. The payload is not encoded and its
    /// bytes are not copied.
    ///
    /// Reports which routes ran, as a [`Published`].
    ///
    /// # Which path a message takes
    ///
    /// The route follows what the **caller has asserted about the audience**.
    /// It is never inferred from the graph.
    ///
    /// | the publisher says | route | returns |
    /// |---|---|---|
    /// | `with_intra_process_only()` | the bus alone; nothing goes on the wire | `Bus(Sent(n))`, or `Bus(NoTaker)` when nobody is listening |
    /// | `with_locality(Remote)` | both, to disjoint audiences | `BusAndWire(..)` |
    /// | neither | the wire alone; the bus is not consulted | `Wire` |
    ///
    /// Row three is the default and is deliberately conservative. A subscriber
    /// that the ROS graph cannot see — a plain `z_sub`, a storage plugin, a
    /// recorder — is reachable only over the wire, so a publisher that has not
    /// been told the audience takes it.
    ///
    /// `Bus(NoTaker)` is a distinct outcome rather than a silent drop: the
    /// caller asserted there was a local audience and there was none, which is
    /// worth acting on. See [`crate::local_bus`].
    ///
    /// # Ordering against [`publish`](Self::publish)
    ///
    /// Bus delivery is synchronous on the calling thread: the subscriber
    /// callbacks have run by the time this returns. A message sent with
    /// `publish` travels the wire and arrives later. Interleaving the two on one
    /// topic gives no ordering guarantee between them.
    ///
    /// # QoS is not carried on the intra-process path
    ///
    /// A message that goes to the bus does not enter the wire publisher's
    /// cache and carries no attachment. Concretely:
    ///
    /// - **`TRANSIENT_LOCAL` is refused, not silently violated.** Nothing on
    ///   the bus enters the durability cache, so a late joiner would be served
    ///   the surviving samples *as if they were a complete history*. A
    ///   transient-local publisher that has no wire is rejected instead.
    /// - `RELIABLE` / `BEST_EFFORT` and `KEEP_LAST(n)` have no meaning here:
    ///   delivery is a synchronous inline call with no queue, so nothing is
    ///   ever dropped for congestion and no depth is ever displaced.
    /// - The attachment — source gid, sequence number, source timestamp — is
    ///   absent, so no bus message carries ROS message-info.
    ///
    pub fn publish_shared(&self, msg: Arc<T>) -> Result<Published>
    where
        T: Send + Sync + 'static,
    {
        match self.route() {
            Route::BusOnly => {
                // TRANSIENT_LOCAL lives in the wire publisher's cache and this
                // publisher has no wire. Serving the bus would hand a late
                // joiner whatever predates this call *as if it were the
                // promised history* — wrong, rather than absent. Refuse, and
                // let the caller choose. See #133.
                if let Some(e) = self.refuse_durable_bus() {
                    return Err(e);
                }
                let d = self.local_channel.publish(msg);
                if d == Delivery::NoTaker {
                    // Distinguish "nobody is listening" from "somebody is
                    // listening whom this path cannot serve". The shared path
                    // filters on is_shared(), so an owned subscriber is skipped
                    // — and this publisher has no wire to catch the message.
                    // Reporting Ok here loses it silently.
                    let owned = self.local_channel.owned_receivers::<T>();
                    if owned > 0 {
                        return Err(format!(
                            "publish_shared on an intra-process-only publisher for {}: \
                             {owned} subscriber(s) on this topic take the message by value, \
                             and a shared publish cannot serve them. The message would be \
                             dropped with no wire behind it. Use publish_owned(), or build \
                             the subscriber with build_with_shared_callback().",
                            self.entity.topic
                        )
                        .into());
                    }
                    debug!(
                        "[PUB] intra-process only, no local subscriber on {}: message dropped",
                        self.entity.topic
                    );
                }
                // DepthExceeded is kept distinct from "nothing delivered": a
                // caller may fall back to the wire on NoTaker and must not on
                // DepthExceeded.
                Ok(Published::Bus(d))
            }
            // Both audiences, and no subscriber is on both. TRANSIENT_LOCAL is
            // safe here: the wire publish populates the cache as it always did.
            Route::BusAndWire => {
                // Wire first, matching publish_owned. Bus delivery is
                // synchronous, so running it first and then failing on the wire
                // returns Err *after* every local subscriber has already been
                // called — and Result<Published> has no partial-success value
                // to say so. A caller that retries then delivers twice locally.
                self.publish(&msg)?;
                Ok(Published::BusAndWire(self.local_channel.publish(msg)))
            }
            Route::WireOnly => {
                self.publish(&msg)?;
                Ok(Published::Wire)
            }
        }
    }

    /// Which routes this publisher's messages take.
    ///
    /// Resolved in one place because it is asserted by the caller and read by
    /// every publishing method. When each method decided for itself they drifted:
    /// `publish_owned` on a `Locality::Remote` publisher took the bus and
    /// returned, losing the message for every off-session subscriber, while
    /// `publish_shared` in the identical configuration served both.
    ///
    /// `with_intra_process_only()` wins over a locality, because it is the
    /// stronger assertion: it says this publisher has no wire at all.
    fn route(&self) -> Route {
        if self.intra_process_only {
            Route::BusOnly
        } else if self.locality == Some(zenoh::sample::Locality::Remote) {
            // The wire half cannot reach this session, so the two halves address
            // disjoint audiences and both must run.
            Route::BusAndWire
        } else {
            Route::WireOnly
        }
    }

    /// Refuse a durable publisher the bus, which has no durability cache.
    ///
    /// This lives in one place because it must guard **every** route onto the
    /// bus. It was originally written inline in `publish_shared`, and
    /// `publish_owned` reached the bus around it — the same violation, through
    /// the sibling method. See #133.
    fn refuse_durable_bus(&self) -> Option<zenoh::Error> {
        if !self.intra_process_only {
            // A `Locality::Remote` publisher still puts every message on the
            // wire, so its durability cache is populated as it always was.
            return None;
        }
        if !matches!(self.entity.qos.durability, QosDurability::TransientLocal) {
            return None;
        }
        Some(
            format!(
                "publish on a TRANSIENT_LOCAL, intra-process-only publisher for {}: \
                 the intra-process path has no durability cache, so a late-joining \
                 subscriber would be served a history this message is missing from. \
                 Use publish(), or drop the transient-local durability.",
                self.entity.topic
            )
            .into(),
        )
    }

    /// Publish `msg` by value, giving it away when exactly one receiver wants it.
    ///
    /// Where [`publish_shared`](Self::publish_shared) hands every receiver the
    /// same read-only `Arc`, this hands a sole owning subscriber the value
    /// itself — free to mutate it, consume it, or put it back in a pool. It is
    /// the move rclcpp performs when a topic has exactly one taker.
    ///
    /// The route is the same one [`publish_shared`](Self::publish_shared)
    /// documents, resolved from what the caller asserted. Within a bus route,
    /// a sole owning subscriber is given the value; with more than one taker
    /// there is nothing to give away, so it is shared instead.
    ///
    /// Returns a [`Published`] saying which routes ran — not a count. An
    /// `intra_process_only` publisher with no listener reports
    /// `Bus(Delivery::NoTaker)` and the message is dropped; it does **not**
    /// fall through to the wire, because such a publisher has none.
    ///
    pub fn publish_owned(&self, msg: T) -> Result<Published>
    where
        T: Send + Sync + 'static,
    {
        match self.route() {
            // The wire needs the value in order to serialize it and the bus
            // wants to own it, so the wire goes first from a reference and the
            // value is given away afterwards. Sharing instead would reach the
            // wire but stop serving owned subscribers, because the shared path
            // filters on `is_shared()`.
            Route::BusAndWire => {
                self.publish(&msg)?;
                Ok(Published::BusAndWire(
                    match self.local_channel.publish_owned(msg) {
                        Ok(d) => d,
                        // No sole owning receiver; the shared half may want it.
                        Err(returned) => self.local_channel.publish(Arc::new(returned)),
                    },
                ))
            }
            Route::BusOnly => {
                if let Some(e) = self.refuse_durable_bus() {
                    return Err(e);
                }
                match self.local_channel.publish_owned(msg) {
                    Ok(d) => Ok(Published::Bus(d)),
                    // Handed back untouched: not exactly one owning receiver.
                    // Share it instead — publish_shared refuses rather than
                    // dropping if the only subscribers are owned ones.
                    Err(returned) => self.publish_shared(Arc::new(returned)),
                }
            }
            Route::WireOnly => {
                self.publish(&msg)?;
                Ok(Published::Wire)
            }
        }
    }


    /// Publish pre-serialized data directly
    ///
    /// Accepts any type that implements `Into<ZBytes>`:
    /// - `&[u8]` - byte slice
    /// - `Vec<u8>` - owned bytes
    /// - `ZBuf` - zero-copy buffer (preferred for performance)
    /// - `ZBytes` - zenoh bytes
    pub fn publish_serialized(&self, data: impl Into<zenoh::bytes::ZBytes>) -> Result<()> {
        let mut put_builder = self.inner.put(data);

        // Set encoding if configured
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            put_builder = put_builder.attachment(self.new_attachment());
        }
        put_builder.wait()
    }

    pub fn publish_sample(&self, msg: &Sample) -> Result<()> {
        let payload = msg.payload().to_bytes();
        // NOTE: pass by reference to avoid copy
        let mut put_builder = self.inner.put(&payload);

        // Set encoding if configured
        if let Some(ref enc) = self.encoding {
            put_builder = put_builder.encoding((**enc).clone());
        }

        if self.with_attachment {
            put_builder = put_builder.attachment(self.new_attachment());
        }
        put_builder.wait()
    }

    pub fn events_mgr(&self) -> &Arc<Mutex<EventsManager>> {
        &self.events_mgr
    }

    /// Get a reference to the endpoint entity for this publisher.
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
    }
}

// Specialized implementation for DynamicMessage publisher
impl ZPub<crate::dynamic::DynamicMessage, crate::dynamic::DynamicSerdeCdrSerdes> {
    /// Get the dynamic schema used by this publisher.
    ///
    /// Returns `None` if the publisher was not created with `.with_dyn_schema()`.
    pub fn schema(&self) -> Option<&crate::dynamic::schema::MessageSchema> {
        self.dyn_schema.as_ref().map(|s| s.as_ref())
    }
}

pub struct ZSubBuilder<T, S = SerdeCdrSerdes<T>> {
    pub(crate) entity: EndpointEntity,
    pub(crate) session: Arc<Session>,
    pub(crate) graph: Arc<Graph>,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    pub(crate) dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    pub(crate) locality: Option<zenoh::sample::Locality>,
    /// Expected encoding for received messages.
    /// If set, the subscriber will validate that received samples match this encoding.
    pub(crate) expected_encoding: Option<crate::encoding::Encoding>,
    pub(crate) _phantom_data: PhantomData<(T, S)>,
}

impl<T, S> ZSubBuilder<T, S>
where
    T: ZMessage,
{
    pub fn with_qos(mut self, qos: QosProfile) -> Self {
        self.entity.qos = qos.to_protocol_qos();
        self
    }

    pub fn with_serdes<S2>(self) -> ZSubBuilder<T, S2> {
        ZSubBuilder {
            entity: self.entity,
            session: self.session,
            graph: self.graph,
            keyexpr_format: self.keyexpr_format.clone(),
            dyn_schema: self.dyn_schema,
            locality: self.locality,
            expected_encoding: self.expected_encoding,
            _phantom_data: PhantomData,
        }
    }

    /// Set the locality restriction for this subscription.
    ///
    /// This restricts the subscription to only receive samples from publishers
    /// with the specified locality (local/remote/any).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use zenoh::sample::Locality;
    ///
    /// let subscriber = node
    ///     .create_sub::<String>("/topic")
    ///     .with_locality(Locality::Remote)  // Only receive from remote publishers
    ///     .build()?;
    /// ```
    pub fn with_locality(mut self, locality: zenoh::sample::Locality) -> Self {
        self.locality = Some(locality);
        self
    }

    /// Set the expected encoding for received messages.
    ///
    /// When set, the subscriber will validate that incoming samples have matching
    /// encoding metadata. If the encoding doesn't match, a warning is logged.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use hiroz::encoding::Encoding;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// # let ctx = hiroz::context::ZContextBuilder::default().build()?;
    /// # let node = ctx.create_node("test").build()?;
    /// // Expect Protobuf encoding
    /// let sub = node.create_sub::<hiroz_msgs::geometry_msgs::Point>("/topic")
    ///     .with_encoding(Encoding::protobuf().with_schema("geometry_msgs/msg/Point"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_encoding(mut self, encoding: crate::encoding::Encoding) -> Self {
        self.expected_encoding = Some(encoding);
        self
    }

    /// Set the dynamic message schema for runtime-typed messages.
    ///
    /// This is required when using `DynamicMessage` with `DynamicSerdeCdrSerdes`.
    /// The schema will be used to deserialize incoming messages.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let subscriber = node
    ///     .create_sub::<DynamicMessage>("/topic")
    ///     .with_serdes::<DynamicSerdeCdrSerdes>()
    ///     .with_dyn_schema(schema)
    ///     .build()?;
    /// ```
    pub fn with_dyn_schema(mut self, schema: Arc<crate::dynamic::schema::MessageSchema>) -> Self {
        // Only compute and set type_info if it hasn't been set already
        // (e.g., from create_dyn_sub_auto which provides the publisher's hash)
        if self.entity.type_info.is_none() {
            self.entity.type_info = Some(crate::dynamic::schema_type_info(&schema));
        }

        self.dyn_schema = Some(schema);
        self
    }

    /// Build a raw Zenoh subscriber with a sample-level callback, returning
    /// the subscriber and liveliness token.
    ///
    /// This is the canonical subscriber setup path used by [`ZCache`] so that
    /// topic qualification, key-expression construction, and liveliness-token
    /// declaration are not duplicated.
    ///
    /// [`ZCache`]: crate::cache::ZCache
    pub(crate) fn build_raw_subscriber<F>(
        mut self,
        callback: F,
    ) -> Result<(
        zenoh::pubsub::Subscriber<()>,
        zenoh::liveliness::LivelinessToken,
    )>
    where
        F: Fn(Sample) + Send + Sync + 'static,
    {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from(
                "subscriber build requires node identity",
            ));
        };
        let qualified_topic =
            crate::topic_name::qualify_topic_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        self.entity.topic = qualified_topic.clone();
        debug!("[CACHE] Qualified topic: {}", qualified_topic);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone();
        debug!("[CACHE] Key expression: {}", key_expr);

        let sub = self
            .session
            .declare_subscriber(key_expr)
            .callback(callback)
            .wait()?;

        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        Ok((sub, lv_token))
    }

    /// Internal method that all build variants use.
    fn build_internal<Q>(
        mut self,
        handler: DataHandler<Sample>,
        queue: Option<Arc<BoundedQueue<Q>>>,
    ) -> Result<ZSub<T, Q, S>>
    where
        S: ZDeserializer,
    {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from(
                "subscriber build requires node identity",
            ));
        };
        let qualified_topic =
            topic_name::qualify_topic_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify topic: {}", e)))?;

        self.entity.topic = qualified_topic.clone();
        debug!("[SUB] Qualified topic: {}", qualified_topic);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone(); // Deref and clone the KeyExpr
        debug!(
            "[SUB] Key expression: {}, qos={:?}",
            key_expr, self.entity.qos
        );

        // Wrap handler with encoding validation if expected encoding is set
        let expected_encoding = self.expected_encoding.clone();
        let validated_handler = move |sample: Sample| {
            // Validate encoding if expected encoding is set
            if let Some(ref expected) = expected_encoding {
                let encoding_str = sample.encoding().to_string();
                if let Some(received) =
                    crate::encoding::Encoding::from_zenoh_encoding(&encoding_str)
                {
                    if &received != expected {
                        tracing::warn!(
                            "Encoding mismatch: expected {:?}, received {:?}",
                            expected,
                            received
                        );
                    }
                } else {
                    tracing::debug!("Unknown encoding format: {}", encoding_str);
                }
            }
            handler.handle(sample)
        };

        // Build an AdvancedSubscriber and configure based on durability
        let mut sub_builder = self
            .session
            .declare_subscriber(key_expr)
            .callback(validated_handler)
            .advanced();

        // Apply locality restriction if specified
        if let Some(locality) = self.locality {
            sub_builder = sub_builder.allowed_origin(locality);
            debug!("[SUB] Locality restriction: {:?}", locality);
        }

        let sub_builder = apply_transient_local_sub(sub_builder, &self.entity.qos);
        let inner = sub_builder.wait()?;

        let gid = crate::entity::endpoint_gid(&self.entity)
            .expect("local endpoint always has node identity");
        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        debug!("[SUB] Subscriber ready: topic={}", self.entity.topic);

        Ok(ZSub {
            entity: self.entity,
            _inner: inner,
            _lv_token: lv_token,
            queue,
            events_mgr: Arc::new(Mutex::new(EventsManager::new(gid))),
            graph: self.graph,
            dyn_schema: self.dyn_schema,
            expected_encoding: self.expected_encoding,
            _local_sub: None,
            _phantom_data: Default::default(),
        })
    }

    /// Build a subscriber whose callback receives the message **by value**.
    ///
    /// Served only by [`ZPub::publish_owned`], and only when this is the sole
    /// subscriber on the topic for this type — with a second receiver the
    /// message cannot be given away. Otherwise the publisher falls back to a
    /// shared or wire delivery, which this subscriber does not receive.
    ///
    /// Use it where the receiver wants to mutate or consume the message, which
    /// the shared path cannot offer.
    pub fn build_with_owned_callback<F>(self, callback: F) -> Result<ZSub<T, (), S>>
    where
        T: Send + Sync + 'static,
        F: Fn(T) + Send + Sync + 'static,
        S: for<'a> ZDeserializer<Input<'a> = &'a [u8], Output = T> + 'static,
    {
        let zid = self.session.zid();
        // Both halves run the same callback. The wire half is not decoration:
        // without it this subscriber discards every message that does not
        // arrive by `publish_owned` on the bus — including from every remote
        // publisher — while still advertising a live subscription.
        //
        // It cannot deliver twice: a publisher takes the bus or the wire for
        // any one message, never both, unless its wire half is `Remote`-
        // restricted, in which case the two audiences are disjoint.
        let callback = Arc::new(callback);
        let wire_half = callback.clone();
        // Read before `self` is consumed below.
        let bus_is_excluded = self.locality == Some(zenoh::sample::Locality::Remote);
        let mut sub = self.build_with_callback(move |m: T| wire_half(m))?;
        // A `Locality::Remote` subscriber has asked not to see same-session
        // traffic, and the bus is same-session by definition. Registering it
        // here would deliver exactly what its own `allowed_origin` filter
        // excludes — the wire half honours the restriction while the bus half
        // silently ignored it. Skip the bus registration; the wire half keeps
        // serving the remote traffic this subscriber asked for.
        if bus_is_excluded {
            return Ok(sub);
        }

        let topic = sub.entity.topic.clone();
        let channel = crate::local_bus::channel(zid, &topic);
        sub._local_sub = Some(crate::local_bus::subscribe_owned::<T, _>(
            channel,
            move |m: T| callback(m),
        ));
        Ok(sub)
    }

    /// **Prototype.** Build a subscriber that receives an `Arc<T>` from a
    /// same-session publisher without any serialization, and still receives
    /// remote traffic over the wire as usual.
    ///
    /// Two registrations are made:
    ///
    /// | path | source | cost |
    /// |---|---|---|
    /// | intra-process bus | a same-session [`ZPub::publish_shared`] for this exact `T` | a refcount bump |
    /// | zenoh subscriber, no origin filter | anything on the wire, near or far | the usual CDR decode |
    ///
    /// The wire half applies no origin filter, and does not need one. A
    /// publisher takes the bus or the wire for any one message, never both —
    /// unless its wire half is `Locality::Remote`, in which case the two
    /// audiences are disjoint and no subscriber sees it twice. Suppressing the
    /// duplicate is the publisher's job, because it is the only side that knows
    /// which routes it used.
    ///
    /// The type match is exact: a publisher of a different concrete Rust type
    /// on the same topic is not delivered here, even if the ROS type name
    /// agrees. See [`crate::local_bus`].
    pub fn build_with_shared_callback<F>(self, callback: F) -> Result<ZSub<T, (), S>>
    where
        T: Send + Sync + 'static,
        F: Fn(Arc<T>) + Send + Sync + 'static,
        S: for<'a> ZDeserializer<Input<'a> = &'a [u8], Output = T> + 'static,
    {
        let zid = self.session.zid();

        let callback = Arc::new(callback);
        let wire_cb = callback.clone();
        // Read before `self` is consumed below.
        let bus_is_excluded = self.locality == Some(zenoh::sample::Locality::Remote);
        let mut sub = self.build_with_callback(move |msg: T| (*wire_cb)(Arc::new(msg)))?;

        // build_with_callback qualified the topic, so read it back rather than
        // re-deriving it — the publisher keys the bus on its own qualified topic
        // and the two must agree exactly.
        // A `Locality::Remote` subscriber has asked not to see same-session
        // traffic, and the bus is same-session by definition. Registering it
        // here would deliver exactly what its own `allowed_origin` filter
        // excludes — the wire half honours the restriction while the bus half
        // silently ignored it. Skip the bus registration; the wire half keeps
        // serving the remote traffic this subscriber asked for.
        if bus_is_excluded {
            return Ok(sub);
        }

        let topic = sub.entity.topic.clone();
        let channel = crate::local_bus::channel(zid, &topic);
        sub._local_sub = Some(crate::local_bus::subscribe::<T, _>(
            channel,
            move |arc: Arc<T>| (*callback)(arc),
        ));
        debug!("[SUB] intra-process bus registered: topic={topic}");
        Ok(sub)
    }

    /// Build a subscriber with a callback that processes deserialized messages directly.
    ///
    /// This method creates a subscriber that invokes the provided callback for each
    /// received message, bypassing the internal queue. The callback receives the
    /// deserialized message directly. Liveliness tokens and event management are
    /// preserved.
    ///
    /// # Ownership
    ///
    /// The returned [`ZSub`] **must be kept alive** for as long as the subscription
    /// should remain active. Dropping it undeclares the Zenoh subscriber and the
    /// liveliness token (removing the node from the ROS graph).
    ///
    /// **Binding layers handle this automatically**: `hiroz-py` and `hiroz-go` store
    /// the handle inside the node (matching `rmw_zenoh_cpp`'s `NodeData::subs_`
    /// pattern), so Python/Go callers do not need to assign the return value.
    /// Rust callers must store the `ZSub` in their node or context.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called with each deserialized message
    ///
    /// # Returns
    ///
    /// A `ZSub` with no internal queue (callback-only mode)
    pub fn build_with_callback<F>(self, callback: F) -> Result<ZSub<T, (), S>>
    where
        F: Fn(S::Output) + Send + Sync + 'static,
        S: for<'a> ZDeserializer<Input<'a> = &'a [u8]> + 'static,
    {
        let expected_encoding = self.expected_encoding.clone();
        let callback = Arc::new(move |sample: Sample| {
            // Validate encoding if expected encoding is set
            if let Some(ref expected) = expected_encoding {
                let encoding_str = sample.encoding().to_string();
                if let Some(received) =
                    crate::encoding::Encoding::from_zenoh_encoding(&encoding_str)
                {
                    if &received != expected {
                        tracing::warn!(
                            "Encoding mismatch: expected {:?}, received {:?}",
                            expected,
                            received
                        );
                    }
                } else {
                    tracing::debug!("Unknown encoding format: {}", encoding_str);
                }
            }

            let payload = sample.payload().to_bytes();
            match S::deserialize(&payload) {
                Ok(msg) => callback(msg),
                Err(e) => tracing::error!("Failed to deserialize message: {}", e),
            }
        });

        self.build_internal(DataHandler::Callback(callback), None)
    }

    #[cfg(feature = "rmw")]
    pub fn build_with_notifier<F>(self, notify: F) -> Result<ZSub<T, Sample, S>>
    where
        F: Fn() + Send + Sync + 'static,
        S: ZDeserializer,
    {
        let queue_size = match self.entity.qos.history {
            QosHistory::KeepLast(depth) => depth,
            QosHistory::KeepAll => usize::MAX,
        };
        let queue = Arc::new(BoundedQueue::new(queue_size));

        self.build_internal(
            DataHandler::QueueWithNotifier {
                queue: queue.clone(),
                notifier: Arc::new(notify),
            },
            Some(queue),
        )
    }
}

impl<T, S> Builder for ZSubBuilder<T, S>
where
    T: ZMessage + 'static + Sync + Send,
    S: ZDeserializer,
{
    type Output = ZSub<T, Sample, S>;

    fn build(self) -> Result<Self::Output> {
        let queue_size = match self.entity.qos.history {
            QosHistory::KeepLast(depth) => depth,
            QosHistory::KeepAll => usize::MAX,
        };
        let queue = Arc::new(BoundedQueue::new(queue_size));

        self.build_internal(DataHandler::Queue(queue.clone()), Some(queue))
    }
}

pub struct ZSub<T: ZMessage, Q, S: ZDeserializer> {
    pub entity: EndpointEntity,
    pub queue: Option<Arc<BoundedQueue<Q>>>,
    _inner: AdvancedSubscriber<()>,
    _lv_token: LivelinessToken,
    events_mgr: Arc<Mutex<EventsManager>>,
    graph: Arc<Graph>,
    /// Schema for dynamic message deserialization.
    /// Required when using `DynamicMessage` with `DynamicSerdeCdrSerdes`.
    pub dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Expected encoding for validation.
    pub expected_encoding: Option<crate::encoding::Encoding>,
    /// Registration on the intra-process bus, when built with
    /// [`ZSubBuilder::build_with_shared_callback`]. Dropping it unregisters, so
    /// a dropped subscriber receives no *further* deliveries.
    /// It does not quiesce: a delivery already in flight on another thread runs to
    /// completion against the snapshot it loaded, so the callback can still run after
    /// `drop` has returned. That is memory-safe, because the snapshot owns the closure
    /// and everything it captured. It is not a barrier, so do not tear down a resource
    /// the callback merely observes on the strength of having dropped the subscriber.
    _local_sub: Option<crate::local_bus::LocalSubscription>,
    _phantom_data: PhantomData<(T, Q, S)>,
}

impl<T: ZMessage, Q, S: ZDeserializer> std::fmt::Debug for ZSub<T, Q, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZSub")
            .field("entity", &self.entity)
            .finish_non_exhaustive()
    }
}

impl<T, S> ZSub<T, Sample, S>
where
    T: ZMessage,
    S: ZDeserializer,
{
    /// Receive the next serialized message (raw sample)
    pub fn recv_serialized(&self) -> Result<Sample> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        Ok(queue.recv())
    }

    /// Async receive the next serialized message (raw sample)
    pub async fn async_recv_serialized(&self) -> Result<Sample> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        Ok(queue.recv_async().await)
    }

    /// Receive the next serialized message with timeout
    pub fn recv_serialized_timeout(&self, timeout: Duration) -> Result<Sample> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        queue
            .recv_timeout(timeout)
            .ok_or_else(|| crate::error::Error::timeout(timeout))
    }

    pub fn events_mgr(&self) -> &Arc<Mutex<EventsManager>> {
        &self.events_mgr
    }

    /// Get a reference to the endpoint entity for this subscriber.
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
    }

    /// Check if there are messages available in the queue
    pub fn is_ready(&self) -> bool {
        self.queue.as_ref().map(|q| !q.is_empty()).unwrap_or(false)
    }

    /// Wait until at least `count` publishers are matched on this subscriber's topic,
    /// or until `timeout` elapses.
    ///
    /// Returns `true` if the required number of publishers appeared within the
    /// timeout, `false` otherwise.
    ///
    /// This mirrors `ZPub::wait_for_subscription` but from the subscriber side.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Ensure at least one publisher is ready before receiving.
    /// assert!(subscriber.wait_for_publisher(1, Duration::from_secs(5)).await);
    /// ```
    pub async fn wait_for_publisher(&self, count: usize, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.graph.change_notify.notified();
            tokio::pin!(notified);

            let n = self
                .graph
                .get_entities_by_topic(EndpointKind::Publisher, &self.entity.topic)
                .len();
            if n >= count {
                return true;
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }

            if tokio::time::timeout(remaining, &mut notified)
                .await
                .is_err()
            {
                return self
                    .graph
                    .get_entities_by_topic(EndpointKind::Publisher, &self.entity.topic)
                    .len()
                    >= count;
            }
        }
    }
}

impl<T, S> ZSub<T, Sample, S>
where
    T: ZMessage,
    S: for<'a> ZDeserializer<Input<'a> = &'a [u8]>,
{
    /// Receive and deserialize the next message (aligned with ROS behavior)
    #[tracing::instrument(name = "recv", skip(self), fields(
        topic = %self.entity.topic,
        payload_len = tracing::field::Empty
    ))]
    pub fn recv(&self) -> Result<S::Output> {
        trace!("[SUB] Waiting for message");

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        let sample = queue.recv();
        let payload = sample.payload().to_bytes();

        tracing::Span::current().record("payload_len", payload.len());
        debug!("[SUB] Received message");

        S::deserialize(&payload).map_err(|e| zenoh::Error::from(e.to_string()))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<S::Output> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        let sample = queue
            .recv_timeout(timeout)
            .ok_or_else(|| crate::error::Error::timeout(timeout))?;
        let payload = sample.payload().to_bytes();
        S::deserialize(&payload).map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Async receive and deserialize the next message
    pub async fn async_recv(&self) -> Result<S::Output> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        let sample = queue.recv_async().await;
        let payload = sample.payload().to_bytes();
        S::deserialize(&payload).map_err(|e| zenoh::Error::from(e.to_string()))
    }
}

// Specialized implementation for DynamicMessage
impl ZSub<crate::dynamic::DynamicMessage, Sample, crate::dynamic::DynamicSerdeCdrSerdes> {
    /// Receive and deserialize the next dynamic message.
    ///
    /// This method requires that the subscriber was built with `.with_dyn_schema()`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The subscriber was built with a callback (no queue available)
    /// - The `dyn_schema` was not set via `.with_dyn_schema()`
    /// - Deserialization fails
    #[tracing::instrument(name = "recv_dynamic", skip(self), fields(
        topic = %self.entity.topic,
        payload_len = tracing::field::Empty
    ))]
    pub fn recv(&self) -> Result<crate::dynamic::DynamicMessage> {
        let schema = self.dyn_schema.as_ref().ok_or_else(|| {
            zenoh::Error::from(
                "dyn_schema required for DynamicMessage (use .with_dyn_schema() when building)",
            )
        })?;

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        trace!("[SUB] Waiting for dynamic message");
        let sample = queue.recv();
        let payload = sample.payload().to_bytes();

        tracing::Span::current().record("payload_len", payload.len());
        debug!("[SUB] Received dynamic message");

        crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Receive a dynamic message with timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<crate::dynamic::DynamicMessage> {
        let schema = self
            .dyn_schema
            .as_ref()
            .ok_or_else(|| zenoh::Error::from("dyn_schema required for DynamicMessage"))?;

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        let sample = queue
            .recv_timeout(timeout)
            .ok_or_else(|| crate::error::Error::timeout(timeout))?;
        let payload = sample.payload().to_bytes();

        crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Async receive a dynamic message.
    pub async fn async_recv(&self) -> Result<crate::dynamic::DynamicMessage> {
        let schema = self
            .dyn_schema
            .as_ref()
            .ok_or_else(|| zenoh::Error::from("dyn_schema required for DynamicMessage"))?;

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        let sample = queue.recv_async().await;
        let payload = sample.payload().to_bytes();

        crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
            .map_err(|e| zenoh::Error::from(e.to_string()))
    }

    /// Try to receive a dynamic message without blocking.
    pub fn try_recv(&self) -> Option<Result<crate::dynamic::DynamicMessage>> {
        let schema = self.dyn_schema.as_ref()?;
        let queue = self.queue.as_ref()?;

        match queue.try_recv() {
            Some(sample) => {
                let payload = sample.payload().to_bytes();
                let result = crate::dynamic::DynamicSerdeCdrSerdes::deserialize((&payload, schema))
                    .map_err(|e| zenoh::Error::from(e.to_string()));
                Some(result)
            }
            None => None,
        }
    }

    /// Get the dynamic schema.
    pub fn schema(&self) -> Option<&crate::dynamic::schema::MessageSchema> {
        self.dyn_schema.as_ref().map(|s| s.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Topic name qualification (leading '/' is added when missing)
    // -----------------------------------------------------------------------

    #[test]
    fn test_qualify_absolute_topic_unchanged() {
        let result = crate::topic_name::qualify_topic_name("/chatter", "/", "node").unwrap();
        assert_eq!(result, "/chatter");
    }

    #[test]
    fn test_qualify_relative_topic_adds_leading_slash() {
        let result = crate::topic_name::qualify_topic_name("chatter", "/", "node").unwrap();
        assert_eq!(result, "/chatter");
    }

    #[test]
    fn test_qualify_topic_with_namespace() {
        let result = crate::topic_name::qualify_topic_name("chatter", "/ns", "node").unwrap();
        assert_eq!(result, "/ns/chatter");
    }

    #[test]
    fn test_qualify_topic_nested_ns() {
        let result = crate::topic_name::qualify_topic_name("/ns/sub/topic", "/", "node").unwrap();
        assert_eq!(result, "/ns/sub/topic");
    }

    // -----------------------------------------------------------------------
    // QoS override is stored in builder entity.qos
    // QoS defaults: Reliable, Volatile, KeepLast(10)
    // -----------------------------------------------------------------------

    #[test]
    fn test_qos_reliability_encoding() {
        // Reliable is the default, BestEffort maps to protocol value
        let best_effort = QosProfile {
            reliability: crate::qos::QosReliability::BestEffort,
            ..Default::default()
        };
        let proto = best_effort.to_protocol_qos();
        assert_eq!(
            proto.reliability,
            hiroz_protocol::qos::QosReliability::BestEffort
        );
    }

    #[test]
    fn test_qos_durability_encoding() {
        let transient = QosProfile {
            durability: crate::qos::QosDurability::TransientLocal,
            ..Default::default()
        };
        let proto = transient.to_protocol_qos();
        assert_eq!(
            proto.durability,
            hiroz_protocol::qos::QosDurability::TransientLocal
        );
    }

    #[test]
    fn test_qos_keep_last_depth_preserved_in_protocol() {
        use std::num::NonZeroUsize;
        let qos = QosProfile {
            history: crate::qos::QosHistory::KeepLast(NonZeroUsize::new(5).unwrap()),
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(proto.history, hiroz_protocol::qos::QosHistory::KeepLast(5));
    }

    #[test]
    fn test_endpoint_entity_topic_field() {
        let entity = hiroz_protocol::entity::EndpointEntity {
            id: 0,
            node: None,
            kind: hiroz_protocol::entity::EndpointKind::Publisher,
            topic: "/my_topic".to_string(),
            type_info: None,
            qos: Default::default(),
        };
        assert_eq!(entity.topic, "/my_topic");
    }
}
