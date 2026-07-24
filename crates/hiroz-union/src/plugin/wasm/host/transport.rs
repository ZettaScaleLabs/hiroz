//! raw_transport::Host resource impls + session::Host and HostSessionHandle.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use wasmtime::component::Resource;
use zenoh::Wait;

use super::super::state::{
    LivelinessSubData, LivelinessTokenData, PluginState, QuerierData, QueryableData, RawPubData,
    RawSubData,
};
use super::hu;

// ─── types host impl ─────────────────────────────────────────────────────────

impl hu::plugin::types::Host for PluginState {}

// ─── raw-transport host impl ─────────────────────────────────────────────────

impl hu::plugin::raw_transport::Host for PluginState {}

impl hu::plugin::raw_transport::HostRawSubscription for PluginState {
    fn try_recv(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawSubscription>,
    ) -> Option<Vec<u8>> {
        self.raw_subs
            .get(&res.rep())
            .and_then(|s| s.rx.try_recv().ok())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawSubscription>,
    ) -> wasmtime::Result<()> {
        self.raw_subs.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostRawPublisher for PluginState {
    fn publish(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawPublisher>,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        let rep = res.rep();
        let Some(data) = self.raw_pubs.get(&rep) else {
            return Err("publisher not found".to_string());
        };
        let session = data.session.clone();
        let ke = data.ke.clone();
        session
            .put(&ke, zenoh::bytes::ZBytes::from(payload))
            .wait()
            .map_err(|e| e.to_string())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawPublisher>,
    ) -> wasmtime::Result<()> {
        self.raw_pubs.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostLivelinessToken for PluginState {
    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::LivelinessToken>,
    ) -> wasmtime::Result<()> {
        self.lv_tokens.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostLivelinessSub for PluginState {
    fn try_recv(
        &mut self,
        res: Resource<hu::plugin::raw_transport::LivelinessSub>,
    ) -> Option<(String, bool)> {
        self.lv_subs
            .get(&res.rep())
            .and_then(|s| s.rx.try_recv().ok())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::LivelinessSub>,
    ) -> wasmtime::Result<()> {
        self.lv_subs.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostQueryable for PluginState {
    fn try_recv_query(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Queryable>,
    ) -> Option<(u64, Vec<u8>)> {
        self.queryables
            .get(&res.rep())
            .and_then(|q| q.rx.try_recv().ok())
    }

    fn reply(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Queryable>,
        query_id: u64,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        let rep = res.rep();
        let Some(qdata) = self.queryables.get(&rep) else {
            return Err("queryable not found".to_string());
        };
        let query = qdata
            .pending
            .lock()
            .remove(&query_id)
            .ok_or_else(|| format!("query {query_id} not found or already replied"))?;
        query
            .reply(
                query.key_expr().clone(),
                zenoh::bytes::ZBytes::from(payload),
            )
            .wait()
            .map_err(|e| e.to_string())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Queryable>,
    ) -> wasmtime::Result<()> {
        self.queryables.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostQuerier for PluginState {
    fn call(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Querier>,
        payload: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, String> {
        let rep = res.rep();
        let Some(qdata) = self.queriers.get(&rep) else {
            return Err("querier not found".to_string());
        };
        let session = qdata.session.clone();
        let ke = qdata.ke.clone();
        let timeout = Duration::from_millis(timeout_ms as u64);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(payload))
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;
        match replies.recv() {
            Ok(reply) => match reply.result() {
                Ok(sample) => Ok(sample.payload().to_bytes().into_owned()),
                Err(e) => Err(e.to_string()),
            },
            Err(_) => Err("no reply received within timeout".to_string()),
        }
    }

    fn drop(&mut self, res: Resource<hu::plugin::raw_transport::Querier>) -> wasmtime::Result<()> {
        self.queriers.remove(&res.rep());
        Ok(())
    }
}

// ─── session host impl ───────────────────────────────────────────────────────

impl hu::plugin::session::Host for PluginState {
    fn get_session(
        &mut self,
        name: String,
    ) -> Result<Resource<hu::plugin::session::SessionHandle>, String> {
        self.require_perm(hu::plugin::types::Permission::OpenSession)?;
        if !self.sessions.contains_key(&name) {
            return Err(format!(
                "session '{name}' not declared in manifest or failed to open"
            ));
        }
        let rep = self.alloc_rep();
        self.session_handle_names.insert(rep, name);
        Ok(Resource::new_own(rep))
    }
}

impl hu::plugin::session::HostSessionHandle for PluginState {
    fn raw_subscribe(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::RawSubscription>, String> {
        self.require_perm(hu::plugin::types::Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let (tx, rx) = flume::bounded::<Vec<u8>>(256);
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let sub = match session.declare_subscriber(&ke_clone).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("raw-subscribe failed on {ke_clone}: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let bytes = sample.payload().to_bytes().into_owned();
                if tx.send_async(bytes).await.is_err() {
                    break;
                }
            }
        });
        let rep = self.alloc_rep();
        self.raw_subs.insert(
            rep,
            RawSubData {
                rx,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn raw_publisher(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::RawPublisher>, String> {
        self.require_perm(hu::plugin::types::Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let rep = self.alloc_rep();
        self.raw_pubs.insert(rep, RawPubData { session, ke });
        Ok(Resource::new_own(rep))
    }

    fn declare_liveliness(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::LivelinessToken>, String> {
        self.require_perm(hu::plugin::types::Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let handle = tokio::spawn(async move {
            let _token = match session.liveliness().declare_token(&ke).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("declare-liveliness failed on {ke}: {e}");
                    return;
                }
            };
            tokio::time::sleep(Duration::MAX).await;
        });
        let rep = self.alloc_rep();
        self.lv_tokens.insert(
            rep,
            LivelinessTokenData {
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn subscribe_liveliness(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::LivelinessSub>, String> {
        self.require_perm(hu::plugin::types::Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let (tx, rx) = flume::bounded::<(String, bool)>(256);
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let sub = match session.liveliness().declare_subscriber(&ke_clone).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("liveliness-sub failed on {ke_clone}: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let key = sample.key_expr().to_string();
                let appeared = sample.kind() == zenoh::sample::SampleKind::Put;
                if tx.send_async((key, appeared)).await.is_err() {
                    break;
                }
            }
        });
        let rep = self.alloc_rep();
        self.lv_subs.insert(
            rep,
            LivelinessSubData {
                rx,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn declare_queryable(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::Queryable>, String> {
        self.require_perm(hu::plugin::types::Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let (tx, rx) = flume::bounded::<(u64, Vec<u8>)>(64);
        let pending: Arc<Mutex<HashMap<u64, zenoh::query::Query>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_task = pending.clone();
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let queryable = match session.declare_queryable(&ke_clone).await {
                Ok(q) => q,
                Err(e) => {
                    tracing::warn!("declare-queryable failed on {ke_clone}: {e}");
                    return;
                }
            };
            let mut next_qid: u64 = 0;
            while let Ok(query) = queryable.recv_async().await {
                let id = next_qid;
                next_qid += 1;
                let payload = query
                    .payload()
                    .map(|p| p.to_bytes().into_owned())
                    .unwrap_or_default();
                pending_task.lock().insert(id, query);
                if tx.send_async((id, payload)).await.is_err() {
                    break;
                }
            }
        });
        let rep = self.alloc_rep();
        self.queryables.insert(
            rep,
            QueryableData {
                rx,
                pending,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn open_querier(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::Querier>, String> {
        self.require_perm(hu::plugin::types::Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let rep = self.alloc_rep();
        self.queriers.insert(rep, QuerierData { session, ke });
        Ok(Resource::new_own(rep))
    }

    fn drop(&mut self, res: Resource<hu::plugin::session::SessionHandle>) -> wasmtime::Result<()> {
        self.session_handle_names.remove(&res.rep());
        Ok(())
    }
}
