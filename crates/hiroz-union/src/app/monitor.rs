//! Multi-topic measurement: per-topic Hz/BW tracking backed by Zenoh subscribers.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use crate::core::engine::Backend;

use super::state::{
    BYTES_PER_KB, EMA_SMOOTHING_FACTOR, HISTORY_LENGTH, TopicMetrics, TopicRateCache,
};

pub struct TopicMonitor {
    session: Arc<zenoh::Session>,
    domain_id: usize,
    backend: Backend,
    pub measuring_topics: HashSet<String>,
    pub topic_metrics: Arc<Mutex<HashMap<String, TopicMetrics>>>,
    subscribers: Vec<zenoh::pubsub::Subscriber<()>>,
    pub rate_cache: HashMap<String, TopicRateCache>,
    pub rate_cache_ttl: Duration,
}

impl TopicMonitor {
    pub fn new(
        session: Arc<zenoh::Session>,
        domain_id: usize,
        backend: Backend,
        rate_cache_ttl: Duration,
    ) -> Self {
        Self {
            session,
            domain_id,
            backend,
            measuring_topics: HashSet::new(),
            topic_metrics: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Vec::new(),
            rate_cache: HashMap::new(),
            rate_cache_ttl,
        }
    }

    fn topic_key_expr(&self, topic: &str) -> String {
        let topic_name = topic.trim_start_matches('/');
        match self.backend {
            Backend::RmwZenoh => format!("{}/{}/**", self.domain_id, topic_name),
        }
    }

    pub async fn add_topic(&mut self, topic: &str) {
        if !self.measuring_topics.contains(topic) {
            self.measuring_topics.insert(topic.to_string());
            self.topic_metrics
                .lock()
                .insert(topic.to_string(), TopicMetrics::default());
        }
        self.restart_subscribers().await;
    }

    pub async fn remove_topic(&mut self, topic: &str) {
        self.measuring_topics.remove(topic);
        self.topic_metrics.lock().remove(topic);
        self.restart_subscribers().await;
    }

    pub fn clear(&mut self) {
        self.measuring_topics.clear();
        self.topic_metrics.lock().clear();
        self.subscribers.clear();
    }

    pub async fn quick_measure_rate(
        &self,
        topic: &str,
        duration: Duration,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let key_expr = self.topic_key_expr(topic);
        let counter = Arc::new(Mutex::new(0usize));
        let counter_clone = counter.clone();
        let subscriber = self
            .session
            .declare_subscriber(&key_expr)
            .callback(move |_sample| {
                *counter_clone.lock() += 1;
            })
            .await?;
        tokio::time::sleep(duration).await;
        let final_count = *counter.lock();
        let rate = final_count as f64 / duration.as_secs_f64();
        drop(subscriber);
        Ok(rate)
    }

    pub fn update(&mut self) {
        let mut metrics = self.topic_metrics.lock();
        for (_topic, tm) in metrics.iter_mut() {
            let elapsed = tm.counter_reset_at.elapsed().as_secs_f64().max(1e-6);
            let instant_rate = tm.msg_count as f64 / elapsed;
            let instant_bw = tm.byte_count as f64 / BYTES_PER_KB / elapsed;

            if tm.rate_history.len() >= HISTORY_LENGTH {
                tm.rate_history.pop_front();
            }
            tm.rate_history.push_back(instant_rate as u64);

            if tm.bandwidth_history.len() >= HISTORY_LENGTH {
                tm.bandwidth_history.pop_front();
            }
            tm.bandwidth_history.push_back((instant_bw * 10.0) as u64);

            tm.samples_collected += 1;
            if tm.samples_collected == 1 {
                tm.current_rate = instant_rate;
                tm.current_bandwidth = instant_bw;
            } else {
                tm.current_rate = EMA_SMOOTHING_FACTOR * instant_rate
                    + (1.0 - EMA_SMOOTHING_FACTOR) * tm.current_rate;
                tm.current_bandwidth = EMA_SMOOTHING_FACTOR * instant_bw
                    + (1.0 - EMA_SMOOTHING_FACTOR) * tm.current_bandwidth;
            }

            tm.msg_count = 0;
            tm.byte_count = 0;
            tm.counter_reset_at = Instant::now();
        }
    }

    async fn restart_subscribers(&mut self) {
        self.subscribers.clear();
        for topic in self.measuring_topics.clone() {
            let key_expr = self.topic_key_expr(&topic);
            let metrics = self.topic_metrics.clone();
            let topic_clone = topic.clone();
            if let Ok(sub) = self
                .session
                .declare_subscriber(&key_expr)
                .callback(move |sample| {
                    let payload_len = sample.payload().len();
                    let mut m = metrics.lock();
                    if let Some(tm) = m.get_mut(&topic_clone) {
                        tm.msg_count += 1;
                        tm.byte_count += payload_len as u64;
                        tm.last_msg_time = Some(Instant::now());
                    }
                })
                .await
            {
                self.subscribers.push(sub);
            }
        }
    }

    pub fn cleanup_rate_cache(&mut self) {
        let now = Instant::now();
        self.rate_cache
            .retain(|_, cache| now.duration_since(cache.last_updated) < self.rate_cache_ttl);
    }

    pub fn cached_rate(&self, topic: &str) -> Option<f64> {
        self.rate_cache.get(topic).map(|c| c.rate)
    }

    pub fn set_rate_cache(&mut self, topic: &str, rate: f64) {
        self.rate_cache.insert(
            topic.to_string(),
            TopicRateCache {
                rate,
                last_updated: Instant::now(),
            },
        );
    }
}
