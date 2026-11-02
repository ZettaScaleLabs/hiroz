use std::{
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tracing::{debug, info, trace};
use zenoh::{
    Result, Session, Wait, bytes, key_expr::KeyExpr, liveliness::LivelinessToken, query::Query,
    sample::Sample,
};

use crate::{
    Builder,
    attachment::{Attachment, GidArray},
    common::DataHandler,
    entity::EndpointEntity,
    impl_with_type_info,
    msg::{ZDeserializer, ZMessage, ZService},
    queue::BoundedQueue,
    topic_name,
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
    #[cfg(test)]
    callback_lifetimes: Mutex<Vec<std::sync::Weak<()>>>,
    #[cfg(test)]
    callback_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(feature = "rmw")]
    completed_tx: flume::Sender<Sample>,
    #[cfg(feature = "rmw")]
    completed_rx: flume::Receiver<Sample>,
    _phantom_data: PhantomData<T>,
    // Querier/token cleanup must finish before the session runtime closes.
    // Fields drop in declaration order; this guard must remain last.
    _session: Arc<Session>,
}

/// The transport query belongs to the call future, not its reusable querier.
/// Cancel before releasing the call's state, including when an outer timeout
/// drops an unlimited-duration action result request.
struct ServiceCallCancellation(zenoh::cancellation::CancellationToken);

impl Drop for ServiceCallCancellation {
    fn drop(&mut self) {
        // Zenoh's async cancellation removes registered queries before its
        // first await; only already-running callback clones can remain after
        // that poll. Those callbacks own their Arc/oneshot state and can finish
        // safely without this future or its runtime. Do not use cancel().wait():
        // its blocking adapter panics on Tokio's current-thread runtime.
        let mut cancellation = std::future::IntoFuture::into_future(self.0.cancel());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let _ = cancellation.as_mut().poll(&mut context);
    }
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
            #[cfg(test)]
            callback_lifetimes: Mutex::default(),
            #[cfg(test)]
            callback_hook: Mutex::default(),
            #[cfg(feature = "rmw")]
            completed_tx,
            #[cfg(feature = "rmw")]
            completed_rx,
            _phantom_data: Default::default(),
            _session: self.session,
        })
    }
}

impl<T> ZClient<T>
where
    T: ZService,
{
    // A graph/liveliness announcement is independent of the queryable route.
    // Wait before dispatching a single request. The caller's outer timeout
    // covers both route discovery and response receipt.
    pub(crate) async fn wait_for_queryable(&self) -> Result<()> {
        let ready = Arc::new(tokio::sync::Notify::new());
        let notify = ready.clone();
        let _listener = self
            .inner
            .matching_listener()
            .callback(move |status| {
                if status.matching() {
                    notify.notify_one();
                }
            })
            .await?;
        // Construction emits true when already matched. Latch that positive
        // event: Zenoh coalesces later false/true transitions, so consuming it
        // and then checking a transient false snapshot can lose the only wakeup.
        ready.notified().await;
        Ok(())
    }

    fn new_attachment(&self) -> Attachment {
        Attachment::with_clock(
            self.sn.fetch_add(1, Ordering::AcqRel) as _,
            self.gid,
            &self.clock,
        )
    }

    async fn call_sample(&self, payload: impl Into<bytes::ZBytes>) -> Result<Sample> {
        self.wait_for_queryable().await?;
        let cancellation = ServiceCallCancellation(Default::default());
        let attachment = self.new_attachment();
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let response_tx = Arc::new(Mutex::new(Some(response_tx)));
        #[cfg(test)]
        let callback_lifetime = {
            let lifetime = Arc::new(());
            self.callback_lifetimes
                .lock()
                .unwrap()
                .push(Arc::downgrade(&lifetime));
            lifetime
        };
        #[cfg(test)]
        let callback_hook = self.callback_hook.lock().unwrap().clone();

        self.inner
            .get()
            .payload(payload)
            .attachment(attachment)
            .cancellation_token(cancellation.0.clone())
            .callback(move |reply| {
                #[cfg(test)]
                let _callback_lifetime = &callback_lifetime;
                #[cfg(test)]
                if let Some(hook) = &callback_hook {
                    hook();
                }
                let reply = reply.into_result();
                let sender = response_tx
                    .lock()
                    .expect("service reply sender mutex poisoned")
                    .take();
                match sender {
                    Some(sender) => {
                        if sender.send(reply).is_err() {
                            tracing::warn!("Service call receiver dropped before reply delivery");
                        }
                    }
                    None => {
                        tracing::warn!("Service call received extra reply after completion");
                    }
                }
            })
            .await?;

        let reply = response_rx
            .await
            .map_err(|_| crate::error::Error::no_service_reply())?;
        reply.map_err(|error| Box::new(error) as zenoh::Error)
    }

    /// Call the service and wait indefinitely for the first reply.
    ///
    /// The first reply determines the outcome, whether it is a response or an
    /// explicit server error. This also applies when multiple queryables match.
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

    /// Call the service within one timeout budget, including route readiness,
    /// response processing and caller scheduling. A queued response cannot turn
    /// an expired call into success when the caller resumes after its deadline.
    /// A zero budget expires without dispatching a request.
    ///
    /// As with [`call`](Self::call), the first response or explicit server error
    /// determines the outcome. A timeout does not prove that the request was not
    /// executed and does not make retrying it safe.
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
        // Dropping the call future cancels its query while preserving the
        // reusable client's other pending calls. Any forwarding callback already
        // executing owns its state and can finish independently of the caller.
        // The error carries a structured [`crate::error::Error::Timeout`] so callers
        // (and language bindings) can detect the timeout via
        // [`crate::error::is_timeout`] instead of sniffing the message string.
        if timeout.is_zero() {
            return Err(crate::error::Error::timeout(timeout));
        }
        // Match Tokio's overflow handling: an unrepresentable deadline keeps
        // the timeout wrapper's effectively unlimited duration semantics.
        let deadline = tokio::time::Instant::now().checked_add(timeout);
        let response = tokio::time::timeout(timeout, self.call(msg))
            .await
            .map_err(|_| crate::error::Error::timeout(timeout))??;
        // Tokio polls the operation before its timer. Under scheduler pressure
        // both can be ready, so enforce the original budget before accepting
        // success; do not reset it after waiting for route readiness.
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(crate::error::Error::timeout(timeout));
        }
        Ok(response)
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
    // Queryable cleanup must finish before the last session closes its runtime.
    // Fields drop in declaration order; this guard must remain last.
    _session: Arc<Session>,
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
            _session: self.session,
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
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Condvar, Mutex},
        time::Duration,
    };

    use crate::{
        Builder,
        context::{ZContext, ZContextBuilder},
        node::ZNode,
        parameter::wire_types::{GetParametersRequest, GetParametersResponse, GetParametersSrv},
    };

    fn native_call_fixture() -> (
        ZContext,
        ZNode,
        super::ZServer<GetParametersSrv>,
        super::ZClient<GetParametersSrv>,
    ) {
        let context = ZContextBuilder::default()
            .disable_multicast_scouting()
            .with_json("listen/endpoints", Vec::<String>::new())
            .with_json("scouting/gossip/enabled", false)
            .build()
            .unwrap();
        let node = context
            .create_node("native_call_cancellation")
            .without_parameters()
            .build()
            .unwrap();
        let server = node
            .create_service_impl::<GetParametersSrv>(
                "/native_call_cancellation",
                Some(<GetParametersSrv as crate::ServiceTypeInfo>::service_type_info()),
            )
            .build()
            .unwrap();
        let client = node
            .create_client::<GetParametersSrv>("/native_call_cancellation")
            .with_querier_timeout(Duration::from_millis(u64::MAX))
            .build()
            .unwrap();
        (context, node, server, client)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_native_calls_release_actual_transport_callbacks() {
        let (_context, _node, mut server, client) = native_call_fixture();
        exercise_native_call_cancellation(&mut server, &client).await;
    }

    #[test]
    fn current_thread_calls_release_callbacks_before_runtime_shutdown() {
        let (_context, _node, mut server, client) = native_call_fixture();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(exercise_native_call_cancellation(&mut server, &client));
        drop(runtime);
        assert!(
            client
                .callback_lifetimes
                .lock()
                .unwrap()
                .iter()
                .all(|callback| callback.upgrade().is_none())
        );
    }

    #[test]
    fn expired_native_call_rejects_queued_response_after_polling_gap() {
        let (_context, _node, mut server, client) = native_call_fixture();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let message = GetParametersRequest { names: Vec::new() };
            // Warm the actual route before exercising the short deadline.
            let (response, reply) = tokio::join!(
                client.call_with_timeout(&message, Duration::from_secs(1)),
                async {
                    server
                        .async_take_request()
                        .await
                        .unwrap()
                        .reply_blocking(&GetParametersResponse { values: Vec::new() })
                }
            );
            reply.unwrap();
            response.unwrap();

            let budget = Duration::from_millis(20);
            let started = std::time::Instant::now();
            let mut call = Box::pin(client.call_with_timeout(&message, budget));
            std::future::poll_fn(|cx| {
                assert!(call.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            let request = tokio::time::timeout(Duration::from_secs(1), server.async_take_request())
                .await
                .unwrap()
                .unwrap();
            let callback = client
                .callback_lifetimes
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .clone();
            assert!(callback.upgrade().is_some());

            // The caller is deliberately not polled while its deadline and
            // then the server's 100ms delay elapse. The real transport callback
            // must finish before resuming it: both result and timer are ready.
            std::thread::sleep(Duration::from_millis(100));
            std::thread::spawn(move || {
                request.reply_blocking(&GetParametersResponse { values: Vec::new() })
            })
            .join()
            .unwrap()
            .unwrap();
            let delivery_deadline = std::time::Instant::now() + Duration::from_secs(1);
            while callback.upgrade().is_some() && std::time::Instant::now() < delivery_deadline {
                std::thread::yield_now();
            }
            assert!(
                callback.upgrade().is_none(),
                "real response callback did not finish"
            );
            let result = call.await;
            eprintln!(
                "service result after unpolled {:.3}ms (budget 20ms): {result:?}",
                started.elapsed().as_secs_f64() * 1000.0
            );
            let error = result.expect_err("expired call accepted a queued late response");
            assert!(
                crate::error::is_timeout(error.as_ref()),
                "wrong error: {error}"
            );
            assert!(
                server.try_take_request().unwrap().is_none(),
                "request was retried"
            );

            // Timing out one call does not poison its reusable client.
            let (response, reply) = tokio::join!(
                client.call_with_timeout(&message, Duration::from_secs(1)),
                async {
                    server
                        .async_take_request()
                        .await
                        .unwrap()
                        .reply_blocking(&GetParametersResponse { values: Vec::new() })
                }
            );
            reply.unwrap();
            assert!(response.unwrap().values.is_empty());
        });
    }

    #[test]
    fn native_call_timeout_zero_and_unrepresentable_deadline() {
        let (_context, _node, mut server, client) = native_call_fixture();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let message = GetParametersRequest { names: Vec::new() };
            let error = client
                .call_with_timeout(&message, Duration::ZERO)
                .await
                .unwrap_err();
            assert!(crate::error::is_timeout(error.as_ref()));
            assert!(
                client.callback_lifetimes.lock().unwrap().is_empty(),
                "zero budget dispatched a query"
            );
            assert!(server.try_take_request().unwrap().is_none());

            let (response, reply) =
                tokio::join!(client.call_with_timeout(&message, Duration::MAX), async {
                    server
                        .async_take_request()
                        .await
                        .unwrap()
                        .reply_blocking(&GetParametersResponse { values: Vec::new() })
                });
            reply.unwrap();
            assert!(response.unwrap().values.is_empty());
        });
    }

    async fn exercise_native_call_cancellation(
        server: &mut super::ZServer<GetParametersSrv>,
        client: &super::ZClient<GetParametersSrv>,
    ) {
        let message = GetParametersRequest { names: Vec::new() };

        for timeout in [false, true] {
            let mut call: Pin<Box<dyn Future<Output = zenoh::Result<GetParametersResponse>>>> =
                if timeout {
                    Box::pin(client.call_with_timeout(&message, Duration::from_millis(100)))
                } else {
                    Box::pin(client.call(&message))
                };
            let held_request = tokio::time::timeout(Duration::from_secs(1), async {
                tokio::select! {
                    result = &mut call => panic!("call completed before response: {result:?}"),
                    request = server.async_take_request() => request.unwrap(),
                }
            })
            .await
            .unwrap();
            // This observer belongs to the actual callback stored by Zenoh,
            // not to the future's receiver or the server's pending request.
            let callback = client
                .callback_lifetimes
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .clone();
            assert!(
                callback.upgrade().is_some(),
                "query callback was not retained while pending"
            );
            if timeout {
                let error = call.await.unwrap_err();
                assert!(crate::error::is_timeout(error.as_ref()));
            } else {
                drop(call);
            }
            assert!(
                callback.upgrade().is_none(),
                "canceled future left its actual transport query callback alive"
            );
            drop(held_request);
        }

        let (response, reply) = tokio::join!(
            client.call_with_timeout(&message, Duration::from_secs(1)),
            async {
                let request = server.async_take_request().await.unwrap();
                request.reply_blocking(&GetParametersResponse { values: Vec::new() })
            }
        );
        reply.unwrap();
        assert!(response.unwrap().values.is_empty());
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if client
                    .callback_lifetimes
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|callback| callback.upgrade().is_none())
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completed forwarding callback retained its state");
    }

    #[test]
    fn active_callback_can_finish_after_current_thread_caller_runtime_is_gone() {
        let (_context, _node, mut server, client) = native_call_fixture();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let callback_release = release.clone();
        *client.callback_hook.lock().unwrap() = Some(Arc::new(move || {
            entered_tx.send(()).unwrap();
            let (lock, changed) = &*callback_release;
            let (released, _) = changed
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |released| {
                    !*released
                })
                .unwrap();
            assert!(
                *released,
                "fixture did not release the active transport callback"
            );
        }));
        // Even an assertion failure must release the callback before endpoint
        // teardown joins its transport thread.
        struct ReleaseOnDrop(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                *self.0.0.lock().unwrap() = true;
                self.0.1.notify_all();
            }
        }
        let release_on_drop = ReleaseOnDrop(release);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (reply, callback) = runtime.block_on(async {
            let message = GetParametersRequest { names: Vec::new() };
            let mut call = Box::pin(client.call(&message));
            let request = tokio::time::timeout(Duration::from_secs(1), async {
                tokio::select! {
                    result = &mut call => panic!("call completed before the server replied: {result:?}"),
                    request = server.async_take_request() => request.unwrap(),
                }
            }).await.unwrap();
            let callback = client.callback_lifetimes.lock().unwrap().last().unwrap().clone();
            let reply = std::thread::spawn(move || request.reply_blocking(&GetParametersResponse { values: Vec::new() }));
            entered_rx.recv_timeout(Duration::from_secs(1)).expect("transport callback did not start");
            drop(call);
            assert!(callback.upgrade().is_some(), "active callback must own its captured state");
            (reply, callback)
        });
        // No cleanup task may depend on this runtime surviving cancellation.
        drop(runtime);
        assert!(callback.upgrade().is_some());
        drop(release_on_drop);
        reply.join().unwrap().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while callback.upgrade().is_some() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            callback.upgrade().is_none(),
            "transport retained the completed callback after caller runtime shutdown"
        );
    }

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
