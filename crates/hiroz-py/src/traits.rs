use anyhow::Result;
use hiroz::pubsub::{ZPub, ZSub};
use std::time::Duration;
use zenoh::bytes::ZBytes;
use zenoh::sample::Sample;

use crate::raw_bytes::{RawBytesCdrSerdes, RawBytesMessage, RawBytesService};

/// Type-erased publisher trait for Python interop
pub(crate) trait RawPublisher: Send + Sync {
    /// Publish pre-serialized data
    fn publish(&self, data: ZBytes) -> Result<()>;
}

/// Type-erased subscriber trait for Python interop
pub(crate) trait RawSubscriber: Send + Sync {
    /// Receive a Sample (zero-copy path - preferred)
    fn recv_sample(&self, timeout: Option<Duration>) -> Result<Sample>;
    /// Try to receive a Sample without blocking (zero-copy path - preferred)
    fn try_recv_sample(&self) -> Result<Option<Sample>>;
    /// Receive pre-serialized bytes (legacy method)
    fn recv_serialized(&self, timeout: Option<Duration>) -> Result<Vec<u8>>;
    /// Try to receive pre-serialized bytes without blocking (legacy method)
    fn try_recv_serialized(&self) -> Result<Option<Vec<u8>>>;
}

// -- Generic wrappers using RawBytesMessage (universal, no per-type code needed) --

/// Generic publisher wrapper using RawBytesMessage
pub struct GenericPubWrapper {
    inner: ZPub<RawBytesMessage, RawBytesCdrSerdes>,
}

impl GenericPubWrapper {
    pub fn new(inner: ZPub<RawBytesMessage, RawBytesCdrSerdes>) -> Self {
        Self { inner }
    }
}

impl RawPublisher for GenericPubWrapper {
    fn publish(&self, data: ZBytes) -> Result<()> {
        self.inner
            .publish_serialized(data)
            .map_err(|e| anyhow::anyhow!(e))
    }
}

/// Generic subscriber wrapper using RawBytesMessage
pub struct GenericSubWrapper {
    inner: ZSub<RawBytesMessage, Sample, RawBytesCdrSerdes>,
}

impl GenericSubWrapper {
    pub fn new(inner: ZSub<RawBytesMessage, Sample, RawBytesCdrSerdes>) -> Self {
        Self { inner }
    }
}

impl RawSubscriber for GenericSubWrapper {
    fn recv_sample(&self, timeout: Option<Duration>) -> Result<Sample> {
        if let Some(t) = timeout {
            let queue = self.inner.queue.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Subscriber was built with callback, no queue available")
            })?;
            queue
                .recv_timeout(t)
                .ok_or_else(|| anyhow::anyhow!("Receive timeout"))
        } else {
            self.inner.recv_serialized().map_err(|e| anyhow::anyhow!(e))
        }
    }

    fn try_recv_sample(&self) -> Result<Option<Sample>> {
        let queue = self.inner.queue.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Subscriber was built with callback, no queue available")
        })?;

        Ok(queue.try_recv())
    }

    fn recv_serialized(&self, timeout: Option<Duration>) -> Result<Vec<u8>> {
        let sample = self.recv_sample(timeout)?;
        Ok(sample.payload().to_bytes().to_vec())
    }

    fn try_recv_serialized(&self) -> Result<Option<Vec<u8>>> {
        match self.try_recv_sample()? {
            Some(sample) => Ok(Some(sample.payload().to_bytes().to_vec())),
            None => Ok(None),
        }
    }
}

// -- Generic service wrappers using RawBytesService --

use hiroz::service::{RequestId, ServiceReply, ZClient, ZServer};

/// Type-erased client trait for Python interop
pub(crate) trait RawClient: Send + Sync {
    fn call_serialized(&self, data: &[u8], timeout: Option<Duration>) -> Result<Vec<u8>>;
}

/// Type-erased server trait for Python interop
pub(crate) trait RawServer: Send + Sync {
    fn take_request_serialized(&self) -> Result<(RequestId, Vec<u8>)>;
    fn send_response_serialized(&self, data: &[u8], request_id: &RequestId) -> Result<()>;
}

/// Generic client wrapper using RawBytesService
pub struct GenericClientWrapper {
    inner: ZClient<RawBytesService>,
}

impl GenericClientWrapper {
    pub fn new(inner: ZClient<RawBytesService>) -> Self {
        Self { inner }
    }
}

impl RawClient for GenericClientWrapper {
    fn call_serialized(&self, data: &[u8], timeout: Option<Duration>) -> Result<Vec<u8>> {
        let request = RawBytesMessage(data.to_vec());
        let timeout = timeout.unwrap_or(Duration::from_secs(3600));

        let fut = async {
            self.inner
                .call_with_timeout(&request, timeout)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to call service: {}", e))
        };

        let response = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(fut),
            Err(_) => tokio::runtime::Runtime::new()?.block_on(fut),
        }?;

        Ok(response.0)
    }
}

/// Generic server wrapper using RawBytesService
pub struct GenericServerWrapper {
    inner: std::sync::Mutex<ZServer<RawBytesService>>,
    pending: std::sync::Mutex<std::collections::HashMap<RequestId, ServiceReply<RawBytesService>>>,
}

impl GenericServerWrapper {
    pub fn new(inner: ZServer<RawBytesService>) -> Self {
        Self {
            inner: std::sync::Mutex::new(inner),
            pending: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl RawServer for GenericServerWrapper {
    fn take_request_serialized(&self) -> Result<(RequestId, Vec<u8>)> {
        let mut server = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock server: {}", e))?;

        let request = server
            .take_request()
            .map_err(|e| anyhow::anyhow!("Failed to receive request: {}", e))?;
        let (request, reply) = request.into_parts();
        let request_id = reply.id().clone();

        self.pending
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock pending replies: {}", e))?
            .insert(request_id.clone(), reply);

        Ok((request_id, request.0))
    }

    fn send_response_serialized(&self, data: &[u8], request_id: &RequestId) -> Result<()> {
        let response = RawBytesMessage(data.to_vec());

        let reply = self
            .pending
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock pending replies: {}", e))?
            .remove(request_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown request id"))?;

        reply
            .reply_blocking(&response)
            .map_err(|e| anyhow::anyhow!("Failed to send response: {}", e))
    }
}
