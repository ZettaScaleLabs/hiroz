//! Per-plugin state: resource handle structs and PluginState.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wasmtime::component::Resource;
use wasmtime_wasi::{WasiCtxView, WasiView};

use crate::core::engine::CoreEngine;

use super::host::hu;
use hu::plugin::types::Permission;

// ─── Subscription tracking (ros interface) ────────────────────────────────────

pub(crate) struct SubscriptionData {
    #[allow(dead_code)]
    pub topic: String,
    pub rx: flume::Receiver<String>,
    pub _abort: tokio::task::AbortHandle,
}

// ─── Raw transport resource state ────────────────────────────────────────────

pub(crate) struct RawSubData {
    pub rx: flume::Receiver<Vec<u8>>,
    pub _abort: tokio::task::AbortHandle,
}

pub(crate) struct RawPubData {
    pub session: Arc<zenoh::Session>,
    pub ke: String,
}

pub(crate) struct LivelinessTokenData {
    pub _abort: tokio::task::AbortHandle,
}

pub(crate) struct LivelinessSubData {
    pub rx: flume::Receiver<(String, bool)>,
    pub _abort: tokio::task::AbortHandle,
}

pub(crate) struct QueryableData {
    pub rx: flume::Receiver<(u64, Vec<u8>)>,
    pub pending: Arc<Mutex<HashMap<u64, zenoh::query::Query>>>,
    pub _abort: tokio::task::AbortHandle,
}

pub(crate) struct QuerierData {
    pub session: Arc<zenoh::Session>,
    pub ke: String,
}

/// Per-topic rate/bandwidth tracker for measure-hz and measure-bw.
pub(crate) struct RateTrackerData {
    pub rx: flume::Receiver<(Instant, usize)>,
    pub arrivals: VecDeque<(Instant, usize)>,
    pub _abort: tokio::task::AbortHandle,
}

impl RateTrackerData {
    pub fn drain_and_trim(&mut self, window_ms: u32) {
        while let Ok(sample) = self.rx.try_recv() {
            self.arrivals.push_back(sample);
        }
        let cutoff = Instant::now() - Duration::from_millis(window_ms as u64);
        while self.arrivals.front().is_some_and(|(t, _)| *t < cutoff) {
            self.arrivals.pop_front();
        }
    }
}

pub(crate) struct ServiceClientData {
    pub session: Arc<zenoh::Session>,
    pub ke: String,
    pub type_name: String,
}

// ─── Per-plugin state ────────────────────────────────────────────────────────

pub struct PluginState {
    pub wasi: wasmtime_wasi::WasiCtx,
    pub table: wasmtime_wasi::ResourceTable,
    pub engine: Arc<CoreEngine>,
    pub subscriptions: HashMap<u32, SubscriptionData>,
    pub next_sub_rep: u32,
    pub sessions: HashMap<String, Arc<zenoh::Session>>,
    pub session_handle_names: HashMap<u32, String>,
    pub raw_subs: HashMap<u32, RawSubData>,
    pub raw_pubs: HashMap<u32, RawPubData>,
    pub lv_tokens: HashMap<u32, LivelinessTokenData>,
    pub lv_subs: HashMap<u32, LivelinessSubData>,
    pub queryables: HashMap<u32, QueryableData>,
    pub queriers: HashMap<u32, QuerierData>,
    pub next_raw_rep: u32,
    pub rate_trackers: HashMap<String, RateTrackerData>,
    pub service_clients: HashMap<u32, ServiceClientData>,
    pub output_lines: Arc<Mutex<Vec<String>>>,
    pub title: Arc<Mutex<String>>,
    pub exit_code: Option<u32>,
    pub permissions: Vec<Permission>,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl PluginState {
    pub fn alloc_rep(&mut self) -> u32 {
        let r = self.next_raw_rep;
        self.next_raw_rep += 1;
        r
    }

    pub fn ensure_rate_tracker(&mut self, topic: &str) -> Result<(), String> {
        if self.rate_trackers.contains_key(topic) {
            return Ok(());
        }
        let domain_id = self.engine.domain_id;
        let topic_stripped = topic.trim_start_matches('/').to_string();
        let ke = format!("{domain_id}/{topic_stripped}/**");
        let session = self.engine.session.clone();
        let (tx, rx) = flume::unbounded::<(Instant, usize)>();
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let sub = match session.declare_subscriber(&ke_clone).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("rate-tracker subscribe failed on {ke_clone}: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let size = sample.payload().to_bytes().len();
                let _ = tx.send((Instant::now(), size));
            }
        });
        self.rate_trackers.insert(
            topic.to_string(),
            RateTrackerData {
                rx,
                arrivals: VecDeque::new(),
                _abort: handle.abort_handle(),
            },
        );
        Ok(())
    }

    pub fn require_perm(&self, p: Permission) -> Result<(), String> {
        if self.permissions.contains(&p) {
            Ok(())
        } else {
            Err(format!(
                "permission denied: {:?} not declared in plugin manifest",
                p
            ))
        }
    }

    pub fn session_for_handle(
        &self,
        res: &Resource<hu::plugin::session::SessionHandle>,
    ) -> Result<Arc<zenoh::Session>, String> {
        let name = self
            .session_handle_names
            .get(&res.rep())
            .ok_or_else(|| "session handle not found".to_string())?;
        self.sessions
            .get(name)
            .cloned()
            .ok_or_else(|| format!("session '{name}' not open"))
    }
}
