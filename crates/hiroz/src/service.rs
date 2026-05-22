use std::{
    marker::PhantomData,
    sync::{Arc, Mutex, atomic::AtomicUsize},
    time::Duration,
};

use tracing::{debug, info, trace};
use zenoh::{
    Result, Session, Wait, bytes, key_expr::KeyExpr, liveliness::LivelinessToken, query::Query,
    sample::Sample,
};

use std::sync::atomic::Ordering;

use crate::topic_name;

use crate::{
    Builder,
    attachment::{Attachment, GidArray},
    common::DataHandler,
    entity::EndpointEntity,
    impl_with_type_info,
    msg::{ZDeserializer, ZMessage, ZService},
    queue::BoundedQueue,
};

#[derive(Debug)]
pub struct ZClientBuilder<T> {
    pub(crate) entity: EndpointEntity,
    pub(crate) session: Arc<Session>,
    pub(crate) clock: crate::time::ZClock,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    pub(crate) querier_timeout: Duration,
    pub(crate) _phantom_data: PhantomData<T>,
}

impl_with_type_info!(ZClientBuilder<T>);
impl_with_type_info!(ZServerBuilder<T>);

/// A ROS 2-style reusable service handle for typed request/response calls.
///
/// Create a client via [`ZNode::create_client`](crate::node::ZNode::create_client).
/// Invoke the service with [`call`](ZClient::call) or [`call_with_timeout`](ZClient::call_with_timeout).
///
/// # Example
///
/// ```rust,ignore
/// use hiroz::prelude::*;
/// use std::time::Duration;
///
/// // client: ZClient<MyService>
/// let response = client.call_with_timeout(&request, Duration::from_secs(5)).await?;
/// ```
pub struct ZClient<T: ZService> {
    // TODO: replace this with the sample sn
    sn: AtomicUsize,
    // TODO: replace this with zenoh's global entity id
    gid: GidArray,
    inner: zenoh::query::Querier<'static>,
    #[allow(dead_code)] // RAII: revokes liveliness token on drop
    lv_token: LivelinessToken,
    topic: String,
    clock: crate::time::ZClock,
    #[cfg(feature = "rmw")]
    completed_tx: flume::Sender<Sample>,
    #[cfg(feature = "rmw")]
    completed_rx: flume::Receiver<Sample>,
    _phantom_data: PhantomData<T>,
}

impl<T: ZService> std::fmt::Debug for ZClient<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZClient")
            .field("topic", &self.topic)
            .finish_non_exhaustive()
    }
}

impl<T> Builder for ZClientBuilder<T>
where
    T: ZService,
{
    type Output = ZClient<T>;

    #[tracing::instrument(name = "client_build", skip(self), fields(
        service = %self.entity.topic
    ))]
    fn build(mut self) -> Result<Self::Output> {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from("client build requires node identity"));
        };
        // Qualify the service name according to ROS 2 rules
        let qualified_service =
            topic_name::qualify_service_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify service: {}", e)))?;

        self.entity.topic = qualified_service.clone();
        debug!("[CLN] Qualified service: {}", qualified_service);

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone(); // Deref and clone the KeyExpr
        debug!("[CLN] Key expression: {}", key_expr);

        let inner = self
            .session
            .declare_querier(key_expr)
            .target(zenoh::query::QueryTarget::All)
            .consolidation(zenoh::query::ConsolidationMode::None)
            .timeout(self.querier_timeout)
            .wait()?;
        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;
        #[cfg(feature = "rmw")]
        let (completed_tx, completed_rx) = {
            let depth = match self.entity.qos.history {
                hiroz_protocol::qos::QosHistory::KeepLast(n) => n,
                hiroz_protocol::qos::QosHistory::KeepAll => 1000,
            };
            flume::bounded(depth)
        };
        debug!("[CLN] Client ready: service={}", self.entity.topic);

        Ok(ZClient {
            sn: AtomicUsize::new(1), // Start at 1 for ROS compatibility
            inner,
            lv_token,
            gid: crate::entity::endpoint_gid(&self.entity)
                .expect("local endpoint always has node identity"),
            topic: self.entity.topic.clone(),
            clock: self.clock,
            #[cfg(feature = "rmw")]
            completed_tx,
            #[cfg(feature = "rmw")]
            completed_rx,
            _phantom_data: Default::default(),
        })
    }
}

impl<T> ZClient<T>
where
    T: ZService,
{
    fn new_attachment(&self) -> Attachment {
        Attachment::with_clock(
            self.sn.fetch_add(1, Ordering::AcqRel) as _,
            self.gid,
            &self.clock,
        )
    }

    async fn call_sample(&self, payload: impl Into<bytes::ZBytes>) -> Result<Sample> {
        let attachment = self.new_attachment();
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let response_tx = Arc::new(Mutex::new(Some(response_tx)));

        self.inner
            .get()
            .payload(payload)
            .attachment(attachment)
            .callback(move |reply| match reply.into_result() {
                Ok(sample) => {
                    let sender = response_tx
                        .lock()
                        .expect("service reply sender mutex poisoned")
                        .take();
                    match sender {
                        Some(sender) => {
                            if sender.send(sample).is_err() {
                                tracing::warn!(
                                    "Service call receiver dropped before reply delivery"
                                );
                            }
                        }
                        None => {
                            tracing::warn!("Service call received extra reply after completion");
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!("Service reply error: {error:?}");
                }
            })
            .await?;

        let sample = response_rx.await.map_err(|_| {
            zenoh::Error::from("Service call ended before any response was received")
        })?;

        Ok(sample)
    }

    /// Call the service and wait indefinitely for the first reply.
    pub async fn call(&self, msg: &T::Request) -> Result<T::Response>
    where
        T::Request: ZMessage,
        T::Response: ZMessage,
        for<'a> <T::Response as ZMessage>::Serdes:
            ZDeserializer<Output = T::Response, Input<'a> = &'a [u8]>,
    {
        let sample = self.call_sample(msg.serialize()).await?;
        let payload_bytes = sample.payload().to_bytes();
        let msg = <T::Response as ZMessage>::deserialize(&payload_bytes[..])
            .map_err(|e| zenoh::Error::from(e.to_string()))?;
        Ok(msg)
    }

    /// Call the service and fail if no reply arrives before `timeout` elapses.
    pub async fn call_with_timeout(
        &self,
        msg: &T::Request,
        timeout: Duration,
    ) -> Result<T::Response>
    where
        T::Request: ZMessage,
        T::Response: ZMessage,
        for<'a> <T::Response as ZMessage>::Serdes:
            ZDeserializer<Output = T::Response, Input<'a> = &'a [u8]>,
    {
        // On timeout the call future is dropped. The Zenoh querier callback is still
        // running briefly; it will hit the `None` sender branch and log a warning.
        // This is expected and harmless.
        tokio::time::timeout(timeout, self.call(msg))
            .await
            .map_err(|_| zenoh::Error::from(format!("Service call timed out after {timeout:?}")))?
    }
}

impl<T> ZClient<T>
where
    T: ZService,
{
    #[cfg(feature = "rmw")]
    #[tracing::instrument(name = "rmw_send_request", skip(self, msg, notify), fields(
        service = %self.topic,
        sn = self.sn.load(Ordering::Acquire),
        payload_len = tracing::field::Empty
    ))]
    pub fn rmw_send_request<F>(&self, msg: &T::Request, notify: F) -> Result<i64>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let completed_tx = self.completed_tx.clone();
        let attachment = self.new_attachment();
        let sn = attachment.sequence_number;
        self.inner
            .get()
            .payload(msg.serialize())
            .attachment(attachment)
            .callback(move |reply| {
                match reply.into_result() {
                    Ok(sample) => {
                        if completed_tx.try_send(sample).is_err() {
                            tracing::warn!(
                                "Client response queue full, dropping response (QoS depth enforced)"
                            );
                        }
                        notify();
                    }
                    Err(err) => {
                        // Handle timeout and other reply errors gracefully
                        // This can happen when a service is not available or times out
                        tracing::debug!("Client reply error: {:?}", err);
                    }
                }
            })
            .wait()?;
        Ok(sn)
    }

    #[cfg(feature = "rmw")]
    pub fn rmw_try_take_response_sample(&self) -> Result<Option<Sample>> {
        match self.completed_rx.try_recv() {
            Ok(sample) => Ok(Some(sample)),
            Err(flume::TryRecvError::Empty) => Ok(None),
            Err(flume::TryRecvError::Disconnected) => {
                Err(zenoh::Error::from("Client response channel disconnected"))
            }
        }
    }

    #[cfg(feature = "rmw")]
    pub fn rmw_has_responses(&self) -> bool {
        !self.completed_rx.is_empty()
    }
}

#[derive(Debug)]
pub struct ZServerBuilder<T> {
    pub(crate) entity: EndpointEntity,
    pub(crate) session: Arc<Session>,
    pub(crate) clock: crate::time::ZClock,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    pub(crate) _phantom_data: PhantomData<T>,
}

impl<T> ZClientBuilder<T> {
    /// Set the QoS profile for this client.
    pub fn with_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.entity.qos = qos.to_protocol_qos();
        self
    }

    pub(crate) fn with_querier_timeout(mut self, timeout: Duration) -> Self {
        self.querier_timeout = timeout;
        self
    }

    /// Get a reference to the entity (for internal and rmw use).
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
    }
}

impl<T> ZServerBuilder<T> {
    /// Set the QoS profile for this server.
    pub fn with_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.entity.qos = qos.to_protocol_qos();
        self
    }

    /// Get a reference to the entity (for internal and rmw use).
    pub fn entity(&self) -> &EndpointEntity {
        &self.entity
    }
}

pub struct ZServer<T: ZService, Q = Query> {
    key_expr: KeyExpr<'static>,
    #[allow(dead_code)] // RAII: deregisters the queryable on drop
    inner: zenoh::query::Queryable<()>,
    #[allow(dead_code)] // RAII: revokes liveliness token on drop
    lv_token: LivelinessToken,
    clock: crate::time::ZClock,
    pub(crate) queue: Option<Arc<BoundedQueue<Q>>>,
    _phantom_data: PhantomData<T>,
}

impl<T: ZService, Q> std::fmt::Debug for ZServer<T, Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZServer")
            .field("key_expr", &self.key_expr.as_str())
            .finish_non_exhaustive()
    }
}

impl<T, Q> ZServer<T, Q>
where
    T: ZService,
{
    /// Access the receiver queue.
    ///
    /// # Panics
    ///
    /// Panics if the server was built with `build_with_callback()` and has no queue.
    /// Action servers always have queues and will never panic.
    pub fn queue(&self) -> &Arc<BoundedQueue<Q>> {
        self.queue
            .as_ref()
            .expect("Server was built with callback mode, no queue available")
    }

    /// Access the receiver queue if present (returns `None` in callback mode).
    pub fn try_queue(&self) -> Option<&Arc<BoundedQueue<Q>>> {
        self.queue.as_ref()
    }
}

impl<T> ZServerBuilder<T>
where
    T: ZService,
{
    /// Internal method that all build variants use.
    fn build_internal<Q>(
        mut self,
        handler: DataHandler<Query>,
        queue: Option<Arc<BoundedQueue<Q>>>,
    ) -> Result<ZServer<T, Q>> {
        let Some(node) = self.entity.node.as_ref() else {
            return Err(zenoh::Error::from("service build requires node identity"));
        };
        let qualified_service =
            topic_name::qualify_service_name(&self.entity.topic, &node.namespace, &node.name)
                .map_err(|e| zenoh::Error::from(format!("Failed to qualify service: {}", e)))?;

        self.entity.topic = qualified_service;

        let topic_ke = self.keyexpr_format.topic_key_expr(&self.entity)?;
        let key_expr = (*topic_ke).clone(); // Deref and clone the KeyExpr
        tracing::debug!("[SRV] KE: {key_expr}");

        info!("[SRV] Declaring queryable on key expression: {}", key_expr);

        let inner = self
            .session
            .declare_queryable(&key_expr)
            .complete(true)
            .callback(move |query| {
                info!(
                    "[SRV] Query received: ke={}, selector={}, parameters={}",
                    query.key_expr(),
                    query.selector(),
                    query.parameters()
                );

                if let Some(att) = query.attachment() {
                    info!("[SRV] Query has attachment: {} bytes", att.len());
                } else {
                    info!("[SRV] Query has NO attachment");
                }

                if let Some(payload) = query.payload() {
                    info!("[SRV] Query has payload: {} bytes", payload.len());
                } else {
                    info!("[SRV] Query has NO payload");
                }

                handler.handle(query);
            })
            .wait()?;

        let lv_ke = self
            .keyexpr_format
            .liveliness_key_expr(&self.entity, &self.session.zid())?;
        let lv_token = self
            .session
            .liveliness()
            .declare_token((*lv_ke).clone())
            .wait()?;

        Ok(ZServer {
            key_expr,
            inner,
            lv_token,
            clock: self.clock,
            queue,
            _phantom_data: Default::default(),
        })
    }

    pub fn build_with_callback<F>(self, callback: F) -> Result<ZServer<T, ()>>
    where
        F: Fn(Query) + Send + Sync + 'static,
    {
        self.build_internal(DataHandler::Callback(Arc::new(callback)), None)
    }

    #[cfg(feature = "rmw")]
    pub fn build_with_notifier<F>(self, notify: F) -> Result<ZServer<T>>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let queue_size = match self.entity.qos.history {
            hiroz_protocol::qos::QosHistory::KeepLast(depth) => depth,
            hiroz_protocol::qos::QosHistory::KeepAll => usize::MAX,
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

impl<T> Builder for ZServerBuilder<T>
where
    T: ZService,
{
    type Output = ZServer<T>;

    fn build(self) -> Result<Self::Output> {
        let queue_size = match self.entity.qos.history {
            hiroz_protocol::qos::QosHistory::KeepLast(depth) => depth,
            hiroz_protocol::qos::QosHistory::KeepAll => usize::MAX,
        };
        let queue = Arc::new(BoundedQueue::new(queue_size));
        self.build_internal(DataHandler::Queue(queue.clone()), Some(queue))
    }
}

/// Identifies a service request by the client's GUID and its per-client sequence number.
///
/// `source_timestamp` is metadata (when the request was sent); it is NOT part of the
/// identity and is excluded from `PartialEq`/`Eq`/`Hash` so that `pending` map lookups
/// work correctly when the response header only carries `(writer_guid, sequence_number)`.
#[derive(Debug, Clone)]
pub struct RequestId {
    pub sequence_number: i64,
    pub writer_guid: GidArray,
    pub source_timestamp: i64,
}

impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        self.sequence_number == other.sequence_number && self.writer_guid == other.writer_guid
    }
}

impl Eq for RequestId {}

impl std::hash::Hash for RequestId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.sequence_number.hash(state);
        self.writer_guid.hash(state);
    }
}

impl From<Attachment> for RequestId {
    fn from(value: Attachment) -> Self {
        Self {
            sequence_number: value.sequence_number,
            writer_guid: value.source_gid,
            source_timestamp: value.source_timestamp,
        }
    }
}

pub struct ServiceReply<T: ZService> {
    request_id: RequestId,
    key_expr: KeyExpr<'static>,
    query: Query,
    clock: crate::time::ZClock,
    replied: bool,
    _phantom_data: PhantomData<T>,
}

impl<T: ZService> Drop for ServiceReply<T> {
    fn drop(&mut self) {
        if !self.replied {
            tracing::warn!(
                sn = self.request_id.sequence_number,
                "ServiceReply dropped without sending a reply — client will wait until querier timeout"
            );
        }
    }
}

impl<T: ZService> ServiceReply<T> {
    pub fn id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn reply_blocking(mut self, msg: &T::Response) -> Result<()> {
        self.replied = true;
        let attachment = Attachment::with_clock(
            self.request_id.sequence_number,
            self.request_id.writer_guid,
            &self.clock,
        );
        self.query
            .reply(&self.key_expr, msg.serialize())
            .attachment(attachment)
            .wait()
    }

    pub async fn reply(mut self, msg: &T::Response) -> Result<()> {
        self.replied = true;
        let attachment = Attachment::with_clock(
            self.request_id.sequence_number,
            self.request_id.writer_guid,
            &self.clock,
        );
        self.query
            .reply(&self.key_expr, msg.serialize())
            .attachment(attachment)
            .await
    }
}

#[must_use = "dropping without calling reply leaves the client waiting indefinitely"]
pub struct ServiceRequest<T: ZService> {
    message: T::Request,
    reply: ServiceReply<T>,
}

impl<T: ZService> ServiceRequest<T> {
    pub fn id(&self) -> &RequestId {
        self.reply.id()
    }

    pub fn message(&self) -> &T::Request {
        &self.message
    }

    pub fn into_parts(self) -> (T::Request, ServiceReply<T>) {
        (self.message, self.reply)
    }

    pub fn reply_blocking(self, response: &T::Response) -> Result<()> {
        self.reply.reply_blocking(response)
    }

    pub async fn reply(self, response: &T::Response) -> Result<()> {
        self.reply.reply(response).await
    }
}

impl<T> ZServer<T, Query>
where
    T: ZService,
{
    fn decode_request(&self, query: Query) -> Result<ServiceRequest<T>>
    where
        T::Request: ZMessage + Send + Sync + 'static,
        for<'a> <T::Request as ZMessage>::Serdes:
            ZDeserializer<Output = T::Request, Input<'a> = &'a [u8]>,
    {
        let attachment_bytes = query
            .attachment()
            .ok_or_else(|| zenoh::Error::from("Service request missing attachment"))?;
        let attachment: Attachment = attachment_bytes.try_into()?;
        let request_id: RequestId = attachment.into();

        let payload_bytes = query
            .payload()
            .map(|payload| payload.to_bytes())
            .unwrap_or_default();
        let message = <T::Request as ZMessage>::deserialize(&payload_bytes[..])
            .map_err(|e| zenoh::Error::from(e.to_string()))?;

        Ok(ServiceRequest {
            message,
            reply: ServiceReply {
                request_id,
                key_expr: self.key_expr.clone(),
                query,
                clock: self.clock.clone(),
                replied: false,
                _phantom_data: PhantomData,
            },
        })
    }

    pub fn try_take_request(&mut self) -> Result<Option<ServiceRequest<T>>>
    where
        T::Request: ZMessage + Send + Sync + 'static,
        for<'a> <T::Request as ZMessage>::Serdes:
            ZDeserializer<Output = T::Request, Input<'a> = &'a [u8]>,
    {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Server was built with callback, no queue available")
        })?;
        match queue.try_recv() {
            Some(query) => self.decode_request(query).map(Some),
            None => Ok(None),
        }
    }

    /// Take the next request as raw payload bytes without typed deserialization.
    ///
    /// Used by the RMW layer, which performs its own C FFI deserialization on the raw bytes.
    /// Returns `(payload_bytes, reply_token)` so the caller can fill an existing message buffer.
    #[cfg(feature = "rmw")]
    pub fn try_take_request_raw(&mut self) -> Result<Option<(Vec<u8>, ServiceReply<T>)>> {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Server was built with callback, no queue available")
        })?;
        let Some(query) = queue.try_recv() else {
            return Ok(None);
        };
        let attachment_bytes = query
            .attachment()
            .ok_or_else(|| zenoh::Error::from("Service request missing attachment"))?;
        let attachment: Attachment = attachment_bytes.try_into()?;
        let request_id: RequestId = attachment.into();
        let payload_bytes = query
            .payload()
            .map(|p| p.to_bytes().to_vec())
            .unwrap_or_default();
        let reply = ServiceReply {
            request_id,
            key_expr: self.key_expr.clone(),
            query,
            clock: self.clock.clone(),
            replied: false,
            _phantom_data: PhantomData,
        };
        Ok(Some((payload_bytes, reply)))
    }

    /// Blocks waiting to receive the next request on the service and then deserializes the payload.
    ///
    /// This method may fail if the message does not deserialize as the requested type.
    #[tracing::instrument(name = "take_request", skip(self), fields(
        service = %self.key_expr,
        sn = tracing::field::Empty,
        payload_len = tracing::field::Empty
    ))]
    pub fn take_request(&mut self) -> Result<ServiceRequest<T>>
    where
        T::Request: ZMessage + Send + Sync + 'static,
        for<'a> <T::Request as ZMessage>::Serdes:
            ZDeserializer<Output = T::Request, Input<'a> = &'a [u8]>,
    {
        trace!("[SRV] Waiting for request");

        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Server was built with callback, no queue available")
        })?;
        let query = queue.recv();
        self.decode_request(query)
    }

    /// Awaits the next request on the service and then deserializes the payload.
    ///
    /// This method may fail if the message does not deserialize as the requested type.
    pub async fn async_take_request(&mut self) -> Result<ServiceRequest<T>>
    where
        T::Request: ZMessage + Send + Sync + 'static,
        for<'a> <T::Request as ZMessage>::Serdes:
            ZDeserializer<Output = T::Request, Input<'a> = &'a [u8]>,
    {
        let queue = self.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Server was built with callback, no queue available")
        })?;
        let query = queue.recv_async().await;
        self.decode_request(query)
    }
}

#[cfg(test)]
mod tests {
    // -----------------------------------------------------------------------
    // Topic name qualification for service names
    // Service names follow the same rules as topic names
    // -----------------------------------------------------------------------

    #[test]
    fn test_qualify_service_absolute_unchanged() {
        let result = crate::topic_name::qualify_service_name("/add_two_ints", "/", "node").unwrap();
        assert_eq!(result, "/add_two_ints");
    }

    #[test]
    fn test_qualify_service_relative_adds_slash() {
        let result = crate::topic_name::qualify_service_name("add_two_ints", "/", "node").unwrap();
        assert_eq!(result, "/add_two_ints");
    }

    #[test]
    fn test_qualify_service_with_namespace() {
        let result =
            crate::topic_name::qualify_service_name("add_two_ints", "/ns", "node").unwrap();
        assert_eq!(result, "/ns/add_two_ints");
    }

    #[test]
    fn test_qualify_service_multipart_name() {
        let result =
            crate::topic_name::qualify_service_name("/my/service/name", "/", "node").unwrap();
        assert_eq!(result, "/my/service/name");
    }

    // -----------------------------------------------------------------------
    // QoS stored in builder entity reflects the protocol values
    // -----------------------------------------------------------------------

    #[test]
    fn test_protocol_qos_default_is_reliable() {
        let qos = crate::qos::QosProfile::default();
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.reliability,
            hiroz_protocol::qos::QosReliability::Reliable
        );
    }

    #[test]
    fn test_protocol_qos_default_is_volatile() {
        let qos = crate::qos::QosProfile::default();
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.durability,
            hiroz_protocol::qos::QosDurability::Volatile
        );
    }

    #[test]
    fn test_request_id_clone_and_eq() {
        let key = crate::service::RequestId {
            writer_guid: [1u8; 16],
            sequence_number: 42,
            source_timestamp: 0,
        };
        let key2 = key.clone();
        assert_eq!(key.sequence_number, key2.sequence_number);
        assert_eq!(key.writer_guid, key2.writer_guid);
        assert_eq!(key.source_timestamp, key2.source_timestamp);
    }

    #[test]
    fn test_zclientbuilder_with_qos_sets_reliability() {
        use crate::qos::{QosProfile, QosReliability};
        let qos = QosProfile {
            reliability: QosReliability::BestEffort,
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.reliability,
            hiroz_protocol::qos::QosReliability::BestEffort
        );
    }
}
