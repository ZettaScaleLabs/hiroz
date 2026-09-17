use std::sync::Arc;

use crate::queue::BoundedQueue;

/// Core abstraction for handling incoming data in subscribers and servers
pub(crate) enum DataHandler<T> {
    /// Queue-based: store for later retrieval (drops oldest when full)
    Queue(Arc<BoundedQueue<T>>),

    /// Direct callback: process immediately
    Callback(Arc<dyn Fn(T) + Send + Sync>),

    /// Queue + notify: store and trigger notification (drops oldest when full)
    #[allow(dead_code)]
    QueueWithNotifier {
        queue: Arc<BoundedQueue<T>>,
        notifier: Arc<dyn Fn() + Send + Sync>,
    },
}

impl<T> DataHandler<T> {
    /// Whether `handle` runs *user* code on the delivering thread.
    ///
    /// Only [`DataHandler::Callback`] does. The queue variants enqueue and
    /// return. zenoh's own `FifoChannel` handler has the same structure. The
    /// user's code therefore runs on whatever thread calls `recv()`. Nothing on
    /// the delivery thread can re-enter hiroz.
    ///
    /// This function deliberately does not count `QueueWithNotifier`'s notifier.
    /// The notifier is the rmw layer's wait-set wake. It must run promptly on
    /// the delivery thread. It does not call back into hiroz. Issue #290 tracks
    /// that nothing enforces this last claim.
    pub(crate) fn runs_user_code(&self) -> bool {
        matches!(self, DataHandler::Callback(_))
    }

    pub(crate) fn handle(&self, data: T) {
        match self {
            DataHandler::Queue(queue) => {
                if queue.push(data) {
                    tracing::debug!("Queue full, dropped oldest message");
                }
            }
            DataHandler::Callback(cb) => cb(data),
            DataHandler::QueueWithNotifier { queue, notifier } => {
                if queue.push(data) {
                    tracing::debug!("Queue full, dropped oldest message");
                }
                notifier();
            }
        }
    }
}
