//! Timestamp-indexed, capacity-bounded message cache.
//!
//! [`ZCache`](crate::cache::ZCache) provides the core functionality of ROS 2's
//! `message_filters::Cache<T>`: retain a sliding window of received messages
//! and query them by time.
//!
//! # Stamp strategies
//!
//! Two indexing strategies are available, selected at build time:
//!
//! - **[`ZenohStamp`](crate::cache::ZenohStamp)** (default) — indexes by the
//!   Zenoh transport timestamp (`uhlc::Timestamp` → `SystemTime`). Zero-config;
//!   works for any message type as long as timestamping is enabled in the Zenoh
//!   config (already enabled in the hiroz default config).
//! - **[`ExtractorStamp`](crate::cache::ExtractorStamp)** — indexes by a
//!   user-supplied closure that extracts a `SystemTime` from each deserialized
//!   message. Required for `header.stamp` / sensor capture time alignment.
//!
//! # Example
//!
//! ```rust,ignore
//! use hiroz::prelude::*;
//! use hiroz_msgs::sensor_msgs::Imu;
//! use std::time::{Duration, SystemTime};
//!
//! let ctx = ZContextBuilder::default().build()?;
//! let node = ctx.create_node("cache_demo").build()?;
//!
//! // Zero-config: indexed by Zenoh transport timestamp
//! let cache = node.create_cache::<Imu>("/imu/data", 200).build()?;
//!
//! let now = SystemTime::now();
//! let window = cache.get_interval(now - Duration::from_millis(100), now);
//!
//! // Application timestamp: indexed by header.stamp
//! let cache = node
//!     .create_cache::<Imu>("/imu/data", 200)
//!     .with_stamp(|msg: &Imu| {
//!         let sec = msg.header.stamp.sec as u64;
//!         let nsec = msg.header.stamp.nanosec;
//!         SystemTime::UNIX_EPOCH + Duration::new(sec, nsec)
//!     })
//!     .build()?;
//! ```

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use tracing::{debug, warn};
use zenoh::Result;
use zenoh::liveliness::LivelinessToken;

use crate::Builder;
use crate::msg::{SerdeCdrSerdes, ZDeserializer, ZMessage};
use crate::pubsub::ZSubBuilder;

// ---------------------------------------------------------------------------
// Stamp strategy markers
// ---------------------------------------------------------------------------

/// Index by the Zenoh transport timestamp (`uhlc::Timestamp` → `SystemTime`).
///
/// This is the default stamp strategy. It works for any message type without
/// any configuration. If the incoming [`zenoh::sample::Sample`] carries no
/// timestamp (timestamping disabled on the peer), the cache falls back to
/// `SystemTime::now()` at receive time and logs a one-time warning.
pub struct ZenohStamp;

/// Index by an application-supplied extractor closure.
///
/// The closure receives a reference to the deserialized message and returns a
/// `SystemTime` representing its logical timestamp (e.g. `header.stamp`).
pub struct ExtractorStamp<T, F: Fn(&T) -> SystemTime>(pub(crate) F, pub(crate) PhantomData<T>);

// ---------------------------------------------------------------------------
// CacheInner — shared mutable state
// ---------------------------------------------------------------------------

/// Internal cache storage — public for benchmarks only.
#[doc(hidden)]
pub struct CacheInner<T> {
    pub entries: BTreeMap<SystemTime, Arc<T>>,
    capacity: usize,
    /// Guards against logging the missing-timestamp warning more than once.
    warned_no_ts: bool,
}

impl<T> CacheInner<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity,
            warned_no_ts: false,
        }
    }

    pub fn insert(&mut self, stamp: SystemTime, msg: T) {
        self.entries.insert(stamp, Arc::new(msg));
        // Evict the oldest entry when over capacity.
        while self.entries.len() > self.capacity {
            self.entries.pop_first();
        }
    }
}

// ---------------------------------------------------------------------------
// ZCache
// ---------------------------------------------------------------------------

/// A timestamp-indexed, capacity-bounded sliding-window cache of received
/// messages.
///
/// Built via [`ZCacheBuilder`], created through
/// [`ZNode::create_cache`](crate::node::ZNode::create_cache).
///
/// Messages are stored as [`Arc<T>`] so query methods return shared references
/// without deep-copying the message payload.
///
/// Dropping `ZCache` automatically deregisters the underlying Zenoh subscriber.
pub struct ZCache<T: ZMessage> {
    inner: Arc<RwLock<CacheInner<T>>>,
    _sub: zenoh::pubsub::Subscriber<()>,
    _lv_token: LivelinessToken,
}

impl<T: ZMessage> ZCache<T> {
    /// All messages with timestamp in `[t_start, t_end]`, inclusive, ordered
    /// by timestamp ascending.
    ///
    /// Returns `Arc<T>` handles — no deep copy of message payload. If
    /// `t_start > t_end` the result is always empty (no panic).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let window = cache.get_interval(
    ///     SystemTime::now() - Duration::from_millis(100),
    ///     SystemTime::now(),
    /// );
    /// ```
    pub fn get_interval(&self, t_start: SystemTime, t_end: SystemTime) -> Vec<Arc<T>> {
        if t_start > t_end {
            return Vec::new();
        }
        let inner = self.inner.read();
        inner
            .entries
            .range(t_start..=t_end)
            .map(|(_, v)| Arc::clone(v))
            .collect()
    }

    /// The most recent message with timestamp ≤ `t`, or `None` if the cache is
    /// empty or all messages are strictly after `t`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let latest = cache.get_before(SystemTime::now());
    /// ```
    pub fn get_before(&self, t: SystemTime) -> Option<Arc<T>> {
        let inner = self.inner.read();
        inner
            .entries
            .range(..=t)
            .next_back()
            .map(|(_, v)| Arc::clone(v))
    }

    /// The earliest message with timestamp ≥ `t`, or `None` if the cache is
    /// empty or all messages are strictly before `t`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let next = cache.get_after(camera_timestamp);
    /// ```
    pub fn get_after(&self, t: SystemTime) -> Option<Arc<T>> {
        let inner = self.inner.read();
        inner.entries.range(t..).next().map(|(_, v)| Arc::clone(v))
    }

    /// The message whose timestamp is nearest to `t` (either side).
    ///
    /// When two messages are equidistant, the one with the earlier (before)
    /// timestamp is returned.
    ///
    /// Returns `None` if the cache is empty.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let nearest_imu = cache.get_nearest(camera_stamp);
    /// ```
    pub fn get_nearest(&self, t: SystemTime) -> Option<Arc<T>> {
        let inner = self.inner.read();
        if inner.entries.is_empty() {
            return None;
        }

        let before = inner
            .entries
            .range(..=t)
            .next_back()
            .map(|(k, v)| (*k, Arc::clone(v)));
        let after = inner
            .entries
            .range(t..)
            .next()
            .map(|(k, v)| (*k, Arc::clone(v)));

        match (before, after) {
            (None, Some((_, v))) => Some(v),
            (Some((_, v)), None) => Some(v),
            (Some((kb, vb)), Some((ka, va))) => {
                let dist_before = t.duration_since(kb).unwrap_or_default();
                let dist_after = ka.duration_since(t).unwrap_or_default();
                // On a tie prefer earlier (before) timestamp.
                if dist_after < dist_before {
                    Some(va)
                } else {
                    Some(vb)
                }
            }
            (None, None) => None,
        }
    }

    /// Timestamp of the oldest cached message, or `None` if empty.
    pub fn oldest_stamp(&self) -> Option<SystemTime> {
        self.inner.read().entries.keys().next().copied()
    }

    /// Timestamp of the newest cached message, or `None` if empty.
    pub fn newest_stamp(&self) -> Option<SystemTime> {
        self.inner.read().entries.keys().next_back().copied()
    }

    /// Number of messages currently in the cache.
    pub fn len(&self) -> usize {
        self.inner.read().entries.len()
    }

    /// `true` if the cache holds no messages.
    pub fn is_empty(&self) -> bool {
        self.inner.read().entries.is_empty()
    }

    /// Remove all messages from the cache.
    pub fn clear(&self) {
        self.inner.write().entries.clear();
    }
}

// ---------------------------------------------------------------------------
// ZCacheBuilder
// ---------------------------------------------------------------------------

/// Builder for [`ZCache<T>`].
///
/// Created by [`ZNode::create_cache`](crate::node::ZNode::create_cache).
/// Use [`with_stamp`](ZCacheBuilder::with_stamp) to switch from the default
/// Zenoh transport timestamp to an application-level extractor.
pub struct ZCacheBuilder<T, S = SerdeCdrSerdes<T>, Stamp = ZenohStamp> {
    pub(crate) sub_builder: ZSubBuilder<T, S>,
    capacity: usize,
    stamp: Stamp,
}

impl<T: ZMessage, S> ZCacheBuilder<T, S, ZenohStamp> {
    pub(crate) fn new(sub_builder: ZSubBuilder<T, S>, capacity: usize) -> Self {
        Self {
            sub_builder,
            capacity,
            stamp: ZenohStamp,
        }
    }

    /// Switch to application-level timestamp extraction.
    ///
    /// The extractor receives a reference to the deserialized message and
    /// returns a `SystemTime` representing its logical timestamp (e.g.
    /// `header.stamp`).
    pub fn with_stamp<F>(self, extractor: F) -> ZCacheBuilder<T, S, ExtractorStamp<T, F>>
    where
        F: Fn(&T) -> SystemTime + Send + Sync + 'static,
    {
        ZCacheBuilder {
            sub_builder: self.sub_builder,
            capacity: self.capacity,
            stamp: ExtractorStamp(extractor, PhantomData),
        }
    }

    /// Maximum number of messages to retain. Oldest are evicted when full.
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Apply a QoS profile to the underlying subscriber.
    pub fn with_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.sub_builder = self.sub_builder.with_qos(qos);
        self
    }
}

impl<T: ZMessage, S, F> ZCacheBuilder<T, S, ExtractorStamp<T, F>>
where
    F: Fn(&T) -> SystemTime + Send + Sync + 'static,
{
    /// Maximum number of messages to retain. Oldest are evicted when full.
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Apply a QoS profile to the underlying subscriber.
    pub fn with_qos(mut self, qos: crate::qos::QosProfile) -> Self {
        self.sub_builder = self.sub_builder.with_qos(qos);
        self
    }
}

// ---------------------------------------------------------------------------
// Builder impl — ZenohStamp variant
// ---------------------------------------------------------------------------

impl<T, S> Builder for ZCacheBuilder<T, S, ZenohStamp>
where
    T: ZMessage + Send + Sync + 'static,
    S: for<'a> ZDeserializer<Input<'a> = &'a [u8], Output = T> + 'static,
{
    type Output = ZCache<T>;

    fn build(self) -> Result<ZCache<T>> {
        let ZCacheBuilder {
            sub_builder,
            capacity,
            ..
        } = self;
        let inner = Arc::new(RwLock::new(CacheInner::<T>::new(capacity)));
        let inner_cb = inner.clone();

        let (sub, lv_token) =
            sub_builder.build_raw_subscriber(move |sample: zenoh::sample::Sample| {
                let payload = sample.payload().to_bytes();
                match S::deserialize(&payload) {
                    Ok(msg) => {
                        let stamp = match sample.timestamp() {
                            Some(ts) => ts.get_time().to_system_time(),
                            None => {
                                let mut guard = inner_cb.write();
                                if !guard.warned_no_ts {
                                    warn!(
                                        "[CACHE] Incoming sample has no Zenoh timestamp; \
                                         falling back to SystemTime::now(). \
                                         Enable timestamping in the Zenoh config to avoid this."
                                    );
                                    guard.warned_no_ts = true;
                                }
                                drop(guard);
                                SystemTime::now()
                            }
                        };
                        inner_cb.write().insert(stamp, msg);
                    }
                    Err(e) => tracing::error!("[CACHE] Failed to deserialize message: {}", e),
                }
            })?;

        debug!("[CACHE] ZenohStamp cache ready");
        Ok(ZCache {
            inner,
            _sub: sub,
            _lv_token: lv_token,
        })
    }
}

// ---------------------------------------------------------------------------
// Builder impl — ExtractorStamp variant
// ---------------------------------------------------------------------------

impl<T, S, F> Builder for ZCacheBuilder<T, S, ExtractorStamp<T, F>>
where
    T: ZMessage + Send + Sync + 'static,
    S: for<'a> ZDeserializer<Input<'a> = &'a [u8], Output = T> + 'static,
    F: Fn(&T) -> SystemTime + Send + Sync + 'static,
{
    type Output = ZCache<T>;

    fn build(self) -> Result<ZCache<T>> {
        let ZCacheBuilder {
            sub_builder,
            capacity,
            stamp: ExtractorStamp(extractor, _),
        } = self;
        let inner = Arc::new(RwLock::new(CacheInner::<T>::new(capacity)));
        let inner_cb = inner.clone();

        let (sub, lv_token) =
            sub_builder.build_raw_subscriber(move |sample: zenoh::sample::Sample| {
                let payload = sample.payload().to_bytes();
                match S::deserialize(&payload) {
                    Ok(msg) => {
                        let stamp = extractor(&msg);
                        inner_cb.write().insert(stamp, msg);
                    }
                    Err(e) => tracing::error!("[CACHE] Failed to deserialize message: {}", e),
                }
            })?;

        debug!("[CACHE] ExtractorStamp cache ready");
        Ok(ZCache {
            inner,
            _sub: sub,
            _lv_token: lv_token,
        })
    }
}
