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
