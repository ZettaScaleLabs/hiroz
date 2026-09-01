use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::{marker::PhantomData, sync::Arc};

use tracing::{debug, trace, warn};
use zenoh::liveliness::LivelinessToken;
use zenoh::{Result, Session, Wait, key_expr::KeyExpr, sample::Sample};

use crate::Builder;
use crate::attachment::{Attachment, GidArray};
use crate::common::DataHandler;
use crate::entity::{EndpointEntity, EndpointKind};
use crate::event::{EventsManager, MessageLossTracker};
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

/// Feed an arriving sample's attachment to the loss tracker.
///
/// A sample whose attachment is missing or undecodable is **ignored, not
/// counted**. The sequence number is the only thing that makes loss detectable,
/// and a publisher that does not send one — a plain zenoh peer rather than a
/// hiroz node — must not be reported as lossy just for being unrecognised.
fn observe_loss(loss: &MessageLossTracker, sample: &Sample) {
    let Some(bytes) = sample.attachment() else {
        return;
    };
    let Ok(attachment) = Attachment::try_from(bytes) else {
        return;
    };
    loss.observe(attachment.source_gid, attachment.sequence_number);
}

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
    local_channel: Arc<crate::local_bus::Channel>,
    /// When set, `publish_shared` and `publish_moved` use the bus alone and never
    /// touch zenoh. The caller asserts this; it is not inferred.
    intra_process_only: bool,
    /// The locality restriction on the wire half, if any. A `Locality::Remote`
    /// wire half cannot reach this session, so the bus and the wire address
    /// disjoint audiences and both run. Otherwise both would deliver twice.
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


/// What may be handed to [`ZPub::publish`], and where each form goes.
///
/// Implemented for `&T`, `Arc<T>` and `T`. The three impls cannot overlap: the
/// trait carries the message type, so `Publishable<U> for &U` and
/// `Publishable<&U> for &U` are different trait references.
pub trait Publishable<T: ZMessage, S: ZSerializer>: Sized {
    /// Publish `self` through `publisher`, by the route its ownership selects.
    fn publish_to(self, publisher: &ZPub<T, S>) -> Result<Published>;
}

impl<T, S> Publishable<T, S> for &T
where
    T: ZMessage + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    fn publish_to(self, publisher: &ZPub<T, S>) -> Result<Published> {
        publisher.publish_ref(self)?;
        Ok(Published::Wire)
    }
}

impl<T, S> Publishable<T, S> for Arc<T>
where
    T: ZMessage + Send + Sync + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    fn publish_to(self, publisher: &ZPub<T, S>) -> Result<Published> {
        // publish_shared returns 0 both when the bus had no taker and when the
        // caller asserted nothing and it went to the wire. The publisher knows
        // which, so ask it rather than guessing from the count.
        let n = publisher.publish_shared(self)?;
        Ok(if publisher.takes_the_bus() {
            Published::Bus(if n == 0 {
                Delivery::NoTaker
            } else {
                Delivery::Sent(n)
            })
        } else {
            Published::Wire
        })
    }
}

impl<T, S> Publishable<T, S> for T
where
    T: ZMessage + Send + Sync + 'static,
    S: for<'a> ZSerializer<Input<'a> = &'a T> + 'static,
{
    fn publish_to(self, publisher: &ZPub<T, S>) -> Result<Published> {
        let n = publisher.publish_moved(self)?;
        Ok(if publisher.takes_the_bus() {
            Published::Bus(if n == 0 {
                Delivery::NoTaker
            } else {
                Delivery::Sent(n)
            })
        } else {
            Published::Wire
        })
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
    /// The restriction selects which matching subscribers receive the samples:
    /// those in the same session, those in other sessions, or both (the default).
    ///
    /// [`Locality::SessionLocal`](zenoh::sample::Locality::SessionLocal) skips the
    /// transport and hands the payload to the matching callbacks in this session.
    /// Every node created from one [`ZContext`] shares one session, so this
    /// reaches sibling nodes in the same process.
    ///
    /// [`ZContext`]: crate::context::ZContext
    ///
    /// # This makes the publisher invisible off-process
    ///
    /// A [`Locality::SessionLocal`](zenoh::sample::Locality::SessionLocal)
    /// publisher is not reachable from any other process, including
    /// `ros2 topic echo`. Set it only where the subscriber is in the same context.
    ///
    /// # Serialization still happens
    ///
    /// The message is still CDR-encoded on publish and decoded on receive. This
    /// setting removes the transport, not the serialization. Publisher SHM is
    /// wasted work under it, so pair it with [`without_shm`](Self::without_shm).
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

    /// **Prototype.** Restrict [`ZPub::publish_shared`] and [`ZPub::publish_moved`]
    /// to the intra-process bus. The message goes to same-session subscribers and
    /// never to zenoh, so it is neither serialized nor transmitted.
    ///
    /// [`ZPub::publish`] keeps its wire behaviour whatever this is set to.
    ///
    /// The caller asserts the audience; it is not inferred. A plain zenoh
    /// subscriber declares no liveliness token, so the graph cannot see it, and a
    /// wrong inference would drop its messages with no report.
    ///
    /// With this set, the message is invisible to every other process, including
    /// `ros2 topic echo`. It is also invisible to any local subscriber that did
    /// not register on the bus with [`ZSubBuilder::build_with_shared_callback`] or
    /// [`ZSubBuilder::build_with_owned_callback`]. Set it only where both ends
    /// are known.
    ///
    /// # This publisher still appears in the ROS graph
    ///
    /// `build()` declares the liveliness token before it consults the locality.
    /// A publisher that reaches nothing outside this process therefore still
    /// advertises itself as an ordinary ROS 2 endpoint. Other nodes count it in
    /// `ros2 topic info -v`, `ros2 node info`, matched-publisher events and
    /// `Graph::count`, and `ros2 topic echo` shows the topic and never a message.
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

        // Set the locality before `.advanced()`: `AdvancedPublisher` forwards its
        // own destination onto this builder, so only this setting survives.
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
    /// Publish, with the argument's ownership choosing the route.
    ///
    /// This is the one publish name, in the shape rclcpp uses: there,
    /// `publish(const&)` copies and `publish(unique_ptr)` moves, and the
    /// overload picks the path. Rust has no overloading, so a trait carries it.
    ///
    /// | argument | route | equivalent to |
    /// |---|---|---|
    /// | `&T` | zenoh | [`ZPub::publish_ref`] |
    /// | `Arc<T>` | the bus, every subscriber sharing one allocation | [`ZPub::publish_shared`] |
    /// | `T` | the bus, a sole receiver owning it | [`ZPub::publish_moved`] |
    ///
    /// **The routing rules are unchanged.** A bus route still requires the
    /// caller to have asserted the audience with
    /// [`ZPubBuilder::with_intra_process_only`] or a `Locality::Remote`; without
    /// one, an `Arc<T>` goes on the wire exactly as before. This method renames
    /// nothing about who decides.
    ///
    /// The named methods remain, and stay the right choice when you want their
    /// exact return type rather than [`Delivery`].
    pub fn publish(&self, msg: impl Publishable<T, S>) -> Result<Published> {
        msg.publish_to(self)
    }

    /// Publish over zenoh, always.
    ///
    /// [`ZPub::publish`] reaches this for a `&T` argument. Call it directly when
    /// you want the wire whatever else is configured, or when you want the
    /// `Result<()>` shape rather than [`Delivery`].
    /// Whether a bus route is available: the caller asserted the audience.
    ///
    /// Mirrors the condition `publish_shared` and `publish_moved` use, so
    /// [`Published`] can say which route ran without inferring it from a count.
    pub(crate) fn takes_the_bus(&self) -> bool {
        self.intra_process_only || self.locality == Some(zenoh::sample::Locality::Remote)
    }

    pub fn publish_ref(&self, msg: &T) -> Result<()> {
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
    /// clone of the `Arc`. The payload is not encoded or copied.
    ///
    /// Returns a [`Published`] that says which routes ran.
    ///
    /// # Which path a message takes
    ///
    /// The route follows what the caller asserted about the audience. It is never
    /// inferred from the graph.
    ///
    /// | the publisher says | route | returns |
    /// |---|---|---|
    /// | `with_intra_process_only()` | the bus alone; nothing goes on the wire | `Bus(Sent(n))`, or `Bus(NoTaker)` when nobody is listening |
    /// | `with_locality(Remote)` | both, to disjoint audiences | `BusAndWire(..)` |
    /// | neither | the wire alone; the bus is not consulted | `Wire` |
    ///
    /// Row three is the default. A subscriber the ROS graph cannot see, such as a
    /// plain `z_sub` or a recorder, is reachable only over the wire. A publisher
    /// that was not told the audience therefore takes the wire.
    ///
    /// `Bus(NoTaker)` is a distinct outcome, not a silent drop: no callback
    /// returned normally. Either no local subscriber matched, or every callback
    /// that ran panicked. A panicked callback may have had effects, so do not
    /// retry or fall back to the wire on `NoTaker`. See [`Delivery`].
    ///
    /// # Ordering against [`publish`](Self::publish)
    ///
    /// Bus delivery is synchronous: the callbacks have run when this returns. A
    /// message sent with `publish` travels the wire and arrives later. The two
    /// have no ordering guarantee between them on one topic.
    ///
    /// # QoS is not carried on the intra-process path
    ///
    /// A message on the bus does not enter the wire publisher's cache and carries
    /// no attachment.
    ///
    /// - **`TRANSIENT_LOCAL` is refused.** The bus keeps no durability cache, so a
    ///   late joiner would be served the surviving samples as if they were a
    ///   complete history. A transient-local publisher with no wire gets an `Err`.
    /// - `RELIABLE` / `BEST_EFFORT` and `KEEP_LAST(n)` have no meaning here:
    ///   delivery is an inline call with no queue.
    /// - The attachment (source gid, sequence number, source timestamp) is absent.
    pub fn publish_shared(&self, msg: Arc<T>) -> Result<Published>
    where
        T: Send + Sync + 'static,
    {
        match self.route() {
            Route::BusOnly => {
                // TRANSIENT_LOCAL lives in the wire publisher's cache, and this
                // publisher has no wire. Refuse rather than serve a partial history.
                if let Some(e) = self.refuse_durable_bus() {
                    return Err(e);
                }
                if let Some(e) = self.refuse_unservable_owned() {
                    return Err(e);
                }
                let d = self.local_channel.publish(msg);
                if d == Delivery::NoTaker {
                    debug!(
                        "[PUB] intra-process only, no callback returned normally on {}: message dropped",
                        self.entity.topic
                    );
                }
                // Keep `DepthExceeded` distinct from `NoTaker`; see [`Delivery`].
                Ok(Published::Bus(d))
            }
            // The audiences are disjoint. TRANSIENT_LOCAL is safe here: the wire
            // publish fills the cache.
            Route::BusAndWire => {
                // Refuse before the wire runs: neither half serves an owned
                // subscriber, and an `Err` after the wire send is a partial success.
                if let Some(e) = self.refuse_unservable_owned() {
                    return Err(e);
                }
                // Wire first, as in `publish_moved`. Bus delivery is synchronous, so
                // a wire failure after it would return `Err` after the local
                // subscribers ran, and a retry would deliver twice locally.
                self.publish_ref(&msg)?;
                Ok(Published::BusAndWire(self.local_channel.publish(msg)))
            }
            Route::WireOnly => {
                self.publish_ref(&msg)?;
                Ok(Published::Wire)
            }
        }
    }

    /// Refuse a shared publish while a by-value subscriber is registered on this
    /// topic.
    ///
    /// The shared path skips that subscriber, and the wire half of a `Remote`
    /// publisher cannot reach a same-session subscriber. Serving the shared
    /// subscribers alone would report success over a message one subscriber never
    /// receives. A clone cannot serve it: `ZMessage` is not `Clone`.
    fn refuse_unservable_owned(&self) -> Option<zenoh::Error>
    where
        T: Send + Sync + 'static,
    {
        let owned = self.local_channel.owned_receivers::<T>();
        if owned == 0 {
            return None;
        }
        Some(
            format!(
                "publish_shared on {}: {owned} subscriber(s) on this topic take the message \
                 by value, and a shared publish cannot serve them. The message would not \
                 reach them. Use publish_moved() with a single owning subscriber, or build \
                 the subscribers with build_with_shared_callback().",
                self.entity.topic
            )
            .into(),
        )
    }

    /// Which routes this publisher's messages take.
    ///
    /// Every publishing method reads this one decision, so they cannot disagree
    /// on the route.
    ///
    /// `with_intra_process_only()` wins over a locality: it asserts that this
    /// publisher has no wire at all.
    fn route(&self) -> Route {
        if self.intra_process_only {
            Route::BusOnly
        } else if self.locality == Some(zenoh::sample::Locality::Remote) {
            // The wire half cannot reach this session, so both halves must run.
            Route::BusAndWire
        } else {
            Route::WireOnly
        }
    }

    /// Refuse a durable publisher the bus, which has no durability cache.
    ///
    /// Every route onto the bus must call this.
    fn refuse_durable_bus(&self) -> Option<zenoh::Error> {
        if !self.intra_process_only {
            // A `Locality::Remote` publisher still sends every message on the
            // wire, so its durability cache is filled.
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
    /// [`publish_shared`](Self::publish_shared) gives every receiver the same
    /// read-only `Arc`. This gives a sole owning subscriber the value itself, so
    /// it can mutate or consume it.
    ///
    /// The route is the one [`publish_shared`](Self::publish_shared) documents.
    /// On a bus route, a sole owning subscriber gets the value. Otherwise the
    /// message is shared, and the shared path refuses when an owned subscriber is
    /// registered.
    ///
    /// On a `Locality::Remote` publisher that refusal does not apply: the wire
    /// runs, the shared subscribers receive, and an owned subscriber receives
    /// nothing unless it is the only subscriber of the type. The result carries
    /// no error. See [`ZSubBuilder::build_with_owned_callback`].
    ///
    /// An `intra_process_only` publisher with no listener returns
    /// `Bus(Delivery::NoTaker)` and drops the message. It has no wire to fall
    /// back to.
    pub fn publish_moved(&self, msg: T) -> Result<Published>
    where
        T: Send + Sync + 'static,
    {
        match self.route() {
            // The wire serializes from a reference and the bus takes ownership,
            // so the wire goes first and the value is given away afterwards.
            Route::BusAndWire => {
                self.publish_ref(&msg)?;
                Ok(Published::BusAndWire(
                    match self.local_channel.publish_moved(msg) {
                        Ok(d) => d,
                        // No sole owning receiver: share it.
                        Err(returned) => self.local_channel.publish(Arc::new(returned)),
                    },
                ))
            }
            Route::BusOnly => {
                if let Some(e) = self.refuse_durable_bus() {
                    return Err(e);
                }
                match self.local_channel.publish_moved(msg) {
                    Ok(d) => Ok(Published::Bus(d)),
                    // Not exactly one owning receiver: share it. `publish_shared`
                    // refuses when only owned subscribers exist.
                    Err(returned) => self.publish_shared(Arc::new(returned)),
                }
            }
            Route::WireOnly => {
                self.publish_ref(&msg)?;
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

/// The subscriptions of a raw-callback subscriber: one on the topic key, and one on the group key
/// when the subscriber joins the group. Dropping the value undeclares both.
pub(crate) struct RawSubscriptions {
    _base: zenoh::pubsub::Subscriber<()>,
    _group: Option<zenoh::pubsub::Subscriber<()>>,
}

/// Declare the group-key subscription of a joined subscriber, or nothing when `group_key` is
/// `None`. Call it before the base subscription and the liveliness token: a publisher relies on
/// that order. `shared` serves both keys.
pub(crate) fn declare_group_subscription<F>(
    session: &Session,
    group_key: Option<&str>,
    locality: Option<zenoh::sample::Locality>,
    shared: &Arc<F>,
) -> Result<Option<zenoh::pubsub::Subscriber<()>>>
where
    F: Fn(Sample) + Send + Sync + 'static,
{
    let Some(group_key) = group_key else {
        return Ok(None);
    };
    let h = shared.clone();
    let mut builder = session
        .declare_subscriber(KeyExpr::try_from(group_key.to_owned())?)
        .callback(move |s: Sample| h(s));
    if let Some(locality) = locality {
        builder = builder.allowed_origin(locality);
    }
    Ok(Some(builder.wait()?))
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
    /// Starts as a copy of the node default and `with_` methods replace single options.
    pub(crate) options: crate::SubscriberOptions,
    /// Whether the message definition has a field that makes a subscriber join the buffer group,
    /// `None` without a definition. See [`BufferGroup::Auto`](crate::BufferGroup::Auto) for the
    /// fields.
    pub(crate) has_buffer_fields: Option<bool>,
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
            options: self.options,
            has_buffer_fields: self.has_buffer_fields,
            _phantom_data: PhantomData,
        }
    }

    /// Overrides the node's buffer-group default. See [`BufferGroup`](crate::BufferGroup).
    pub fn with_buffer_group(mut self, group: crate::BufferGroup) -> Self {
        self.options.buffer_group = group;
        self
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

        self.has_buffer_fields = Some(schema.has_buffer_fields());
        self.dyn_schema = Some(schema);
        self
    }

    /// Build a raw Zenoh subscriber with a sample-level callback. It returns the
    /// [`RawSubscriptions`], whose group subscription is absent when the subscriber does not join,
    /// and the liveliness token.
    ///
    /// This is the canonical subscriber setup path used by [`ZCache`] so that
    /// topic qualification, key-expression construction, and liveliness-token
    /// declaration are not duplicated.
    ///
    /// [`ZCache`]: crate::cache::ZCache
    pub(crate) fn build_raw_subscriber<F>(
        mut self,
        callback: F,
    ) -> Result<(RawSubscriptions, zenoh::liveliness::LivelinessToken)>
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

        let group_key = self.joined_group_key(key_expr.as_str());
        // Both subscriptions come before the token, so a publisher that reads the token finds a
        // subscription. The callback is `Fn`, so the two subscriptions share one `Arc` of it.
        let shared = Arc::new(callback);
        let group_sub =
            declare_group_subscription(&self.session, group_key.as_deref(), None, &shared)?;
        let h = shared.clone();
        let sub = self
            .session
            .declare_subscriber(key_expr.clone())
            .callback(move |s: Sample| h(s))
            .wait()?;

        let token = crate::buffer_group::declare_token(
            &self.session,
            &self.keyexpr_format,
            &self.entity,
            group_key.is_some(),
        )?;

        Ok((
            RawSubscriptions {
                _base: sub,
                _group: group_sub,
            },
            token,
        ))
    }

    fn joined_group_key(&self, base: &str) -> Option<String> {
        crate::buffer_group::joined_group_key(
            self.options.buffer_group,
            self.has_buffer_fields,
            &self.keyexpr_format,
            &self.entity,
            base,
        )
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

        // Built here rather than at `ZSub` construction: the loss tracker needs
        // a handle to it, and the tracker is captured by the callback below.
        let gid = crate::entity::endpoint_gid(&self.entity)
            .expect("local endpoint always has node identity");
        let events_mgr = Arc::new(Mutex::new(EventsManager::new(gid)));
        let loss = Arc::new(MessageLossTracker::new(events_mgr.clone()));

        // Wrap handler with encoding validation if expected encoding is set
        let expected_encoding = self.expected_encoding.clone();
        let validated_handler = move |sample: Sample| {
            observe_loss(&loss, &sample);
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
        let group_key = self.joined_group_key(key_expr.as_str());
        // Both subscriptions come before the token and share one `Arc` of the handler.
        let shared = Arc::new(validated_handler);
        let group_sub = declare_group_subscription(
            &self.session,
            group_key.as_deref(),
            self.locality,
            &shared,
        )?;
        let h = shared.clone();
        let mut sub_builder = self
            .session
            .declare_subscriber(key_expr.clone())
            .callback(move |s: Sample| h(s))
            .advanced();

        // Apply locality restriction if specified
        if let Some(locality) = self.locality {
            sub_builder = sub_builder.allowed_origin(locality);
            debug!("[SUB] Locality restriction: {:?}", locality);
        }

        let sub_builder = apply_transient_local_sub(sub_builder, &self.entity.qos);
        let inner = sub_builder.wait()?;

        let token = crate::buffer_group::declare_token(
            &self.session,
            &self.keyexpr_format,
            &self.entity,
            group_key.is_some(),
        )?;

        debug!("[SUB] Subscriber ready: topic={}", self.entity.topic);

        Ok(ZSub {
            entity: self.entity,
            _inner: inner,
            _lv_token: token,
            _group_sub: group_sub,
            queue,
            events_mgr,
            graph: self.graph,
            dyn_schema: self.dyn_schema,
            expected_encoding: self.expected_encoding,
            _local_sub: None,
            _phantom_data: Default::default(),
        })
    }

    /// Build a subscriber whose callback receives the message **by value**.
    ///
    /// The bus serves it only through [`ZPub::publish_moved`], and only when it is
    /// the sole subscriber of this type on the topic. It also has a wire half, so
    /// an ordinary [`ZPub::publish`] reaches it after deserializing.
    ///
    /// When the bus cannot serve it, the publisher refuses with `Err` on two
    /// routes only: [`ZPub::publish_shared`] on any bus route, and
    /// [`ZPub::publish_moved`] on an intra-process-only publisher.
    ///
    /// # Known limit on a `Locality::Remote` publisher
    ///
    /// [`ZPub::publish_moved`] does not refuse there. If a shared subscriber is
    /// also registered, the shared subscriber receives the message and this
    /// subscriber receives nothing. If only owned subscribers are registered and
    /// there are two or more, none receives it and the result is
    /// `BusAndWire(NoTaker)`. Neither case returns an error.
    ///
    /// Use it where the receiver mutates or consumes the message.
    pub fn build_with_owned_callback<F>(self, callback: F) -> Result<ZSub<T, (), S>>
    where
        T: Send + Sync + 'static,
        F: Fn(T) + Send + Sync + 'static,
        S: for<'a> ZDeserializer<Input<'a> = &'a [u8], Output = T> + 'static,
    {
        let zid = self.session.zid();
        // Both halves run the same callback. Without the wire half this
        // subscriber would miss every message that does not arrive by
        // `publish_moved` on the bus, including all remote traffic.
        //
        // It cannot deliver twice: a publisher uses the bus or the wire for one
        // message, unless its wire half is `Remote`-restricted and the audiences
        // are disjoint.
        let callback = Arc::new(callback);
        let wire_half = callback.clone();
        let bus_is_excluded = self.locality == Some(zenoh::sample::Locality::Remote);
        let mut sub = self.build_with_callback(move |m: T| wire_half(m))?;
        // A `Locality::Remote` subscriber excludes same-session traffic, and the
        // bus is same-session. Skip the bus registration so the bus does not
        // deliver what the wire half's `allowed_origin` filter excludes.
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
    /// same-session publisher without serialization, and still receives remote
    /// traffic over the wire.
    ///
    /// Two registrations are made:
    ///
    /// | path | source | cost |
    /// |---|---|---|
    /// | intra-process bus | a same-session [`ZPub::publish_shared`] for this exact `T` | a refcount bump |
    /// | zenoh subscriber, no origin filter | anything on the wire, near or far | the usual CDR decode |
    ///
    /// The wire half applies no origin filter. A publisher uses the bus or the
    /// wire for one message, unless its wire half is `Locality::Remote` and the
    /// audiences are disjoint, so no subscriber sees a message twice. The
    /// publisher avoids the duplicate because only it knows which routes it used.
    ///
    /// The type match is exact. A publisher of a different Rust type on the same
    /// topic is not delivered here, even if the ROS type name agrees. See
    /// [`crate::local_bus`].
    pub fn build_with_shared_callback<F>(self, callback: F) -> Result<ZSub<T, (), S>>
    where
        T: Send + Sync + 'static,
        F: Fn(Arc<T>) + Send + Sync + 'static,
        S: for<'a> ZDeserializer<Input<'a> = &'a [u8], Output = T> + 'static,
    {
        let zid = self.session.zid();

        let callback = Arc::new(callback);
        let wire_cb = callback.clone();
        let bus_is_excluded = self.locality == Some(zenoh::sample::Locality::Remote);
        let mut sub = self.build_with_callback(move |msg: T| (*wire_cb)(Arc::new(msg)))?;

        // `build_with_callback` qualified the topic. Read it back: the publisher
        // keys the bus on its qualified topic, and the two must match exactly.
        // A `Locality::Remote` subscriber excludes same-session traffic, and the
        // bus is same-session. Skip the bus registration so the bus does not
        // deliver what the wire half's `allowed_origin` filter excludes.
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
    /// The group subscription of a joined subscriber, `None` otherwise.
    _group_sub: Option<zenoh::pubsub::Subscriber<()>>,
    events_mgr: Arc<Mutex<EventsManager>>,
    graph: Arc<Graph>,
    /// Schema for dynamic message deserialization.
    /// Required when using `DynamicMessage` with `DynamicSerdeCdrSerdes`.
    pub dyn_schema: Option<Arc<crate::dynamic::schema::MessageSchema>>,
    /// Expected encoding for validation.
    pub expected_encoding: Option<crate::encoding::Encoding>,
    /// Registration on the intra-process bus, when built with
    /// [`ZSubBuilder::build_with_shared_callback`] or
    /// [`ZSubBuilder::build_with_owned_callback`]. Dropping it unregisters, so a
    /// dropped subscriber receives no further deliveries. It does not quiesce: see
    /// [`LocalSubscription`](crate::local_bus::LocalSubscription).
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

/// Accessors that do not depend on how the subscriber delivers.
///
/// These were on the `ZSub<T, Sample, S>` (queue-mode) impl only, so a callback
/// subscriber could not reach its own events manager — the handle the rmw layer
/// needs to install an event callback, and the only way to observe
/// [`MessageLost`](crate::event::ZenohEventType::MessageLost). Neither field has
/// anything to do with the queue.
impl<T: ZMessage, Q, S: ZDeserializer> ZSub<T, Q, S> {
    /// The event manager this subscriber raises endpoint events on.
    pub fn events_mgr(&self) -> &Arc<Mutex<EventsManager>> {
        &self.events_mgr
    }

    /// Get a reference to the endpoint entity for this subscriber.
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
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
