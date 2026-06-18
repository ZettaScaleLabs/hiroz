//! ZDdsBridge — automatic DDS↔Zenoh discovery and route management.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{Result, anyhow};
use hiroz::node::ZNode;
use hiroz_protocol::entity::{EndpointEntity, EndpointKind, Entity, TypeHash};
use regex::Regex;
use zenoh::{key_expr::OwnedKeyExpr, sample::SampleKind};

use crate::limits;

use super::{
    discovery::{DiscoveredEndpoint, DiscoveryEvent},
    gid::Gid,
    names::{
        dds_topic_to_ros2_name, dds_type_to_ros2_action_type, dds_type_to_ros2_service_type,
        dds_type_to_ros2_type, is_action_get_result_topic, is_pubsub_topic, is_request_topic,
        type_info_from_user_data,
    },
    participant::{BridgeQos, DdsParticipant},
    pubsub::{ZDdsPubBridge, ZDdsSubBridge},
    qos::qos_profile_to_bridge_qos,
    ros_discovery::RosDiscoveryPublisher,
    service::{ZDdsClientBridge, ZDdsServiceBridge},
};

// ─── Filter ───────────────────────────────────────────────────────────────────

struct Filter {
    allow: Option<Regex>,
    deny: Option<Regex>,
}

impl Filter {
    fn new(allow: Option<&str>, deny: Option<&str>) -> Result<Self> {
        let allow = allow
            .map(Regex::new)
            .transpose()
            .map_err(|e| anyhow!("invalid allow regex: {e}"))?;
        let deny = deny
            .map(Regex::new)
            .transpose()
            .map_err(|e| anyhow!("invalid deny regex: {e}"))?;
        Ok(Self { allow, deny })
    }

    fn allows(&self, name: &str) -> bool {
        if let Some(deny) = &self.deny
            && deny.is_match(name)
        {
            return false;
        }
        if let Some(allow) = &self.allow {
            return allow.is_match(name);
        }
        true
    }
}

// ─── RouteEntry ───────────────────────────────────────────────────────────────

/// A bridge route tracked by both local DDS GIDs and remote bridge Z IDs.
///
/// The route is kept alive as long as at least one local GID or remote ZID is registered.
/// When both sets are empty, the caller should drop the route.
struct RouteEntry<R> {
    _route: R,
    /// GIDs of local DDS endpoints that triggered this route.
    local_gids: HashSet<Gid>,
    /// Zenoh session IDs of remote bridges that announced a complementary liveliness token.
    remote_zids: HashSet<String>,
}

impl<R> RouteEntry<R> {
    fn from_local(route: R, gid: Gid) -> Self {
        Self {
            _route: route,
            local_gids: std::iter::once(gid).collect(),
            remote_zids: HashSet::new(),
        }
    }

    fn from_remote(route: R, zid: String) -> Self {
        Self {
            _route: route,
            local_gids: HashSet::new(),
            remote_zids: std::iter::once(zid).collect(),
        }
    }

    fn is_unused(&self) -> bool {
        self.local_gids.is_empty() && self.remote_zids.is_empty()
    }
}

// GIDs of the DDS reader(s) and writer(s) created for a single named route.
// Stored alongside routes so they can be unregistered from ros_discovery_info on removal.
#[derive(Default)]
struct RouteGids {
    readers: Vec<Gid>,
    writers: Vec<Gid>,
}

// ─── ZDdsBridgeBuilder ────────────────────────────────────────────────────────

/// Builder for `ZDdsBridge` — configure filters before starting discovery.
pub struct ZDdsBridgeBuilder<P: DdsParticipant> {
    node: ZNode,
    participant: P,
    allow_topics: Option<String>,
    deny_topics: Option<String>,
    service_timeout: Duration,
    action_get_result_timeout: Duration,
    transient_local_cache_multiplier: usize,
}

impl<P: DdsParticipant> ZDdsBridgeBuilder<P> {
    /// Allow only topics/services whose DDS topic name matches `regex`.
    pub fn allow_topics_regex(mut self, regex: Option<&str>) -> Self {
        self.allow_topics = regex.map(str::to_owned);
        self
    }

    /// Deny topics/services whose DDS topic name matches `regex`.
    pub fn deny_topics_regex(mut self, regex: Option<&str>) -> Self {
        self.deny_topics = regex.map(str::to_owned);
        self
    }

    /// Timeout for service queryable replies (default 10 s).
    pub fn service_timeout(mut self, d: Duration) -> Self {
        self.service_timeout = d;
        self
    }

    /// Timeout for action get_result queryable replies (default 300 s).
    pub fn action_get_result_timeout(mut self, d: Duration) -> Self {
        self.action_get_result_timeout = d;
        self
    }

    /// TRANSIENT_LOCAL AdvancedPublisher cache depth multiplier (default 10).
    pub fn transient_local_cache_multiplier(mut self, n: usize) -> Self {
        self.transient_local_cache_multiplier = n;
        self
    }

    /// Start the discovery loop and run until all discovery channels close.
    pub async fn run(self) -> Result<()> {
        let filter = Filter::new(self.allow_topics.as_deref(), self.deny_topics.as_deref())?;
        let ros_discovery =
            RosDiscoveryPublisher::new(&self.participant, self.node.namespace(), self.node.name())
                .map_err(|e| tracing::warn!("ros_discovery_info publisher failed to start: {e}"))
                .ok();

        // Set up federation liveliness subscriber before entering run_loop.
        let (lv_tx, lv_rx) = flume::bounded::<(OwnedKeyExpr, SampleKind)>(256);
        let _lv_sub = self
            .node
            .session()
            .liveliness()
            .declare_subscriber(liveliness_subscription_pattern())
            .history(true)
            .callback(move |sample| {
                let ke = OwnedKeyExpr::from(sample.key_expr().clone());
                let _ = lv_tx.try_send((ke, sample.kind()));
            })
            .await
            .map_err(|e| anyhow!("liveliness subscriber failed: {e}"))?;

        let own_zid = self.node.session().zid().to_string();
        let bridge = ZDdsBridge {
            node: self.node,
            participant: self.participant,
            pub_routes: HashMap::new(),
            sub_routes: HashMap::new(),
            srv_routes: HashMap::new(),
            cli_routes: HashMap::new(),
            gid_to_name: HashMap::new(),
            route_gids: HashMap::new(),
            ros_discovery,
            filter,
            service_timeout: self.service_timeout,
            action_get_result_timeout: self.action_get_result_timeout,
            cache_multiplier: self.transient_local_cache_multiplier,
            own_zid,
            lv_rx,
        };
        bridge.run_loop().await
    }
}

// ─── ZDdsBridge ──────────────────────────────────────────────────────────────

/// Full auto-discovery DDS↔Zenoh bridge.
///
/// Watches one DDS participant for discovered publications and subscriptions and
/// creates the appropriate `ZDdsPubBridge`, `ZDdsSubBridge`, `ZDdsServiceBridge`,
/// or `ZDdsClientBridge` for each.
///
/// Construct via `ZDdsBridge::new(node, participant)` which returns a builder;
/// call `.run().await` to start the discovery loop.
pub struct ZDdsBridge<P: DdsParticipant> {
    node: ZNode,
    participant: P,

    /// DDS publisher → Zenoh publisher bridges, keyed by ros2_name.
    pub_routes: HashMap<String, RouteEntry<ZDdsPubBridge<P>>>,
    /// Zenoh subscriber → DDS writer bridges, keyed by ros2_name.
    sub_routes: HashMap<String, RouteEntry<ZDdsSubBridge<P>>>,
    /// DDS service server → Zenoh queryable bridges, keyed by ros2_name.
    srv_routes: HashMap<String, RouteEntry<ZDdsServiceBridge<P>>>,
    /// DDS service client → Zenoh querier bridges, keyed by ros2_name.
    cli_routes: HashMap<String, RouteEntry<ZDdsClientBridge<P>>>,

    /// Reverse map: gid → ros2_name, for O(1) removal on undiscovery.
    gid_to_name: HashMap<Gid, String>,

    /// GIDs of DDS readers/writers per route name — for ros_discovery_info unregistration.
    route_gids: HashMap<String, RouteGids>,

    /// Publishes `ros_discovery_info` so `ros2 topic/node/service list` can see bridge endpoints.
    ros_discovery: Option<RosDiscoveryPublisher>,

    filter: Filter,
    service_timeout: Duration,
    action_get_result_timeout: Duration,
    cache_multiplier: usize,

    /// Our own Zenoh session ID string — used to skip our own liveliness tokens.
    own_zid: String,
    /// Federation liveliness events from the Zenoh liveliness subscriber.
    lv_rx: flume::Receiver<(OwnedKeyExpr, SampleKind)>,
}

impl<P: DdsParticipant> ZDdsBridge<P> {
    /// Create a new bridge builder for `node` and `participant`.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(node: ZNode, participant: P) -> ZDdsBridgeBuilder<P> {
        ZDdsBridgeBuilder {
            node,
            participant,
            allow_topics: None,
            deny_topics: None,
            service_timeout: Duration::from_secs(10),
            action_get_result_timeout: Duration::from_secs(300),
            transient_local_cache_multiplier: 10,
        }
    }

    fn total_routes(&self) -> usize {
        self.pub_routes.len()
            + self.sub_routes.len()
            + self.srv_routes.len()
            + self.cli_routes.len()
    }

    async fn run_loop(mut self) -> Result<()> {
        let discovery_rx = self.participant.run_discovery();
        tracing::info!("ZDdsBridge: discovery loop started");
        loop {
            tokio::select! {
                event = discovery_rx.recv_async() => {
                    match event {
                        Ok(ev) => {
                            if let Err(e) = self.handle_event(ev).await {
                                tracing::warn!("ZDdsBridge: route error: {e}");
                            }
                        }
                        Err(_) => {
                            tracing::info!("ZDdsBridge: discovery channel closed, shutting down");
                            break;
                        }
                    }
                }
                lv_event = self.lv_rx.recv_async() => {
                    match lv_event {
                        Ok((ke, kind)) => {
                            if let Err(e) = self.handle_liveliness(ke, kind).await {
                                tracing::debug!("ZDdsBridge: liveliness error: {e}");
                            }
                        }
                        Err(_) => {
                            tracing::debug!("ZDdsBridge: liveliness channel closed");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_event(&mut self, event: DiscoveryEvent) -> Result<()> {
        match event {
            DiscoveryEvent::DiscoveredPublication(ep) => {
                self.on_publication(ep).await?;
            }
            DiscoveryEvent::UndiscoveredPublication(gid) => {
                if let Some(name) = self.gid_to_name.remove(&gid) {
                    self.remove_local_pub_gid(&name, gid);
                    self.remove_local_cli_gid(&name, gid);
                    tracing::debug!("ZDdsBridge: removed local gid for pub/cli route {name}");
                }
            }
            DiscoveryEvent::DiscoveredSubscription(ep) => {
                self.on_subscription(ep).await?;
            }
            DiscoveryEvent::UndiscoveredSubscription(gid) => {
                if let Some(name) = self.gid_to_name.remove(&gid) {
                    self.remove_local_sub_gid(&name, gid);
                    self.remove_local_srv_gid(&name, gid);
                    tracing::debug!("ZDdsBridge: removed local gid for sub/srv route {name}");
                }
            }
        }
        Ok(())
    }

    async fn handle_liveliness(&mut self, ke: OwnedKeyExpr, kind: SampleKind) -> Result<()> {
        let ke_ref: zenoh::key_expr::KeyExpr<'_> = (&ke).into();
        let entity = self
            .node
            .keyexpr_format()
            .parse_liveliness(&ke_ref)
            .map_err(|e| anyhow!("parse_liveliness failed for {ke}: {e}"))?;

        let ep = match entity {
            Entity::Endpoint(ep) => ep,
            Entity::Node(_) => return Ok(()),
        };

        // Skip our own liveliness tokens.
        let remote_zid = match ep.node.as_ref().map(|n| n.z_id.to_string()) {
            Some(zid) => zid,
            // Tokens without a node identity (old ros2dds format) — extract from key.
            None => ke.as_str().split('/').nth(1).unwrap_or("").to_string(),
        };
        if remote_zid == self.own_zid {
            return Ok(());
        }

        match kind {
            SampleKind::Put => self.on_federation_announce(ep, remote_zid).await,
            SampleKind::Delete => self.on_federation_retire(ep, &remote_zid).await,
        }
    }

    async fn on_federation_announce(
        &mut self,
        ep: EndpointEntity,
        remote_zid: String,
    ) -> Result<()> {
        let ros2_name = ep.topic.clone();
        if !self.filter.allows(&ros2_name) {
            return Ok(());
        }

        let type_name = match ep.type_info.as_ref() {
            Some(ti) if !ti.name.is_empty() => ti.name.clone(),
            _ => return Ok(()), // can't create DDS entity without a type name
        };
        // Zero hash means hash was unknown — pass None to avoid type matching issues.
        let type_hash = ep
            .type_info
            .as_ref()
            .filter(|ti| ti.hash != TypeHash::zero())
            .map(|ti| ti.hash.clone());

        let qos = qos_profile_to_bridge_qos(&ep.qos);

        match ep.kind {
            // Remote Publisher → create local ZDdsSubBridge (Zenoh→DDS relay)
            EndpointKind::Publisher => {
                if let Some(entry) = self.sub_routes.get_mut(&ros2_name) {
                    entry.remote_zids.insert(remote_zid);
                    tracing::debug!(
                        "ZDdsBridge: federation: added remote pub ZID to existing sub route {ros2_name}"
                    );
                    return Ok(());
                }
                limits::check_topic_cap(self.total_routes())?;
                let bridge = ZDdsSubBridge::new(
                    &self.node,
                    &ros2_name,
                    &type_name,
                    type_hash,
                    &self.participant,
                    qos,
                    true,
                )
                .await?;
                tracing::info!(
                    "ZDdsBridge: federation sub route {ros2_name} (remote pub from {remote_zid})"
                );
                self.sub_routes
                    .insert(ros2_name, RouteEntry::from_remote(bridge, remote_zid));
            }
            // Remote Subscription → create local ZDdsPubBridge (DDS→Zenoh relay)
            EndpointKind::Subscription => {
                if let Some(entry) = self.pub_routes.get_mut(&ros2_name) {
                    entry.remote_zids.insert(remote_zid);
                    tracing::debug!(
                        "ZDdsBridge: federation: added remote sub ZID to existing pub route {ros2_name}"
                    );
                    return Ok(());
                }
                limits::check_topic_cap(self.total_routes())?;
                let bridge = ZDdsPubBridge::new(
                    &self.node,
                    &ros2_name,
                    &type_name,
                    type_hash,
                    &self.participant,
                    qos,
                    true,
                    self.cache_multiplier,
                )
                .await?;
                tracing::info!(
                    "ZDdsBridge: federation pub route {ros2_name} (remote sub from {remote_zid})"
                );
                self.pub_routes
                    .insert(ros2_name, RouteEntry::from_remote(bridge, remote_zid));
            }
            // Remote Service server → create local ZDdsClientBridge (DDS clients → remote server)
            EndpointKind::Service => {
                if let Some(entry) = self.cli_routes.get_mut(&ros2_name) {
                    entry.remote_zids.insert(remote_zid);
                    return Ok(());
                }
                limits::check_topic_cap(self.total_routes())?;
                let timeout = if ros2_name.contains("/_action/get_result") {
                    self.action_get_result_timeout
                } else {
                    self.service_timeout
                };
                let bridge = ZDdsClientBridge::new(
                    &self.node,
                    &ros2_name,
                    &type_name,
                    type_hash,
                    &self.participant,
                    qos,
                    timeout,
                )
                .await?;
                tracing::info!(
                    "ZDdsBridge: federation cli route {ros2_name} (remote srv from {remote_zid})"
                );
                self.cli_routes
                    .insert(ros2_name, RouteEntry::from_remote(bridge, remote_zid));
            }
            // Remote Service client → create local ZDdsServiceBridge (DDS servers ← remote client)
            EndpointKind::Client => {
                if let Some(entry) = self.srv_routes.get_mut(&ros2_name) {
                    entry.remote_zids.insert(remote_zid);
                    return Ok(());
                }
                limits::check_topic_cap(self.total_routes())?;
                let bridge = ZDdsServiceBridge::new(
                    &self.node,
                    &ros2_name,
                    &type_name,
                    &self.participant,
                    qos,
                )
                .await?;
                tracing::info!(
                    "ZDdsBridge: federation srv route {ros2_name} (remote cli from {remote_zid})"
                );
                self.srv_routes
                    .insert(ros2_name, RouteEntry::from_remote(bridge, remote_zid));
            }
        }
        Ok(())
    }

    async fn on_federation_retire(&mut self, ep: EndpointEntity, remote_zid: &str) -> Result<()> {
        let ros2_name = &ep.topic;
        match ep.kind {
            EndpointKind::Publisher => {
                if let Some(entry) = self.sub_routes.get_mut(ros2_name) {
                    entry.remote_zids.remove(remote_zid);
                    if entry.is_unused() {
                        self.sub_routes.remove(ros2_name);
                        tracing::debug!(
                            "ZDdsBridge: federation sub route {ros2_name} removed (remote pub {remote_zid} retired)"
                        );
                    }
                }
            }
            EndpointKind::Subscription => {
                if let Some(entry) = self.pub_routes.get_mut(ros2_name) {
                    entry.remote_zids.remove(remote_zid);
                    if entry.is_unused() {
                        self.pub_routes.remove(ros2_name);
                        tracing::debug!(
                            "ZDdsBridge: federation pub route {ros2_name} removed (remote sub {remote_zid} retired)"
                        );
                    }
                }
            }
            EndpointKind::Service => {
                if let Some(entry) = self.cli_routes.get_mut(ros2_name) {
                    entry.remote_zids.remove(remote_zid);
                    if entry.is_unused() {
                        self.cli_routes.remove(ros2_name);
                    }
                }
            }
            EndpointKind::Client => {
                if let Some(entry) = self.srv_routes.get_mut(ros2_name) {
                    entry.remote_zids.remove(remote_zid);
                    if entry.is_unused() {
                        self.srv_routes.remove(ros2_name);
                    }
                }
            }
        }
        Ok(())
    }

    // ─── local-GID removal helpers ────────────────────────────────────────────

    fn remove_local_pub_gid(&mut self, name: &str, gid: Gid) {
        if let Some(entry) = self.pub_routes.get_mut(name) {
            entry.local_gids.remove(&gid);
            if entry.is_unused() {
                self.pub_routes.remove(name);
                self.unregister_route_gids(name);
            }
        }
    }

    fn remove_local_sub_gid(&mut self, name: &str, gid: Gid) {
        if let Some(entry) = self.sub_routes.get_mut(name) {
            entry.local_gids.remove(&gid);
            if entry.is_unused() {
                self.sub_routes.remove(name);
                self.unregister_route_gids(name);
            }
        }
    }

    fn remove_local_cli_gid(&mut self, name: &str, gid: Gid) {
        if let Some(entry) = self.cli_routes.get_mut(name) {
            entry.local_gids.remove(&gid);
            if entry.is_unused() {
                self.cli_routes.remove(name);
                self.unregister_route_gids(name);
            }
        }
    }

    fn remove_local_srv_gid(&mut self, name: &str, gid: Gid) {
        if let Some(entry) = self.srv_routes.get_mut(name) {
            entry.local_gids.remove(&gid);
            if entry.is_unused() {
                self.srv_routes.remove(name);
                self.unregister_route_gids(name);
            }
        }
    }

    // ─── ros_discovery_info helpers ───────────────────────────────────────────

    fn register_route_gids(&mut self, name: &str, gids: RouteGids) {
        if let Some(rd) = &self.ros_discovery {
            for g in &gids.readers {
                rd.add_reader(*g);
            }
            for g in &gids.writers {
                rd.add_writer(*g);
            }
        }
        self.route_gids.insert(name.to_string(), gids);
    }

    fn unregister_route_gids(&mut self, name: &str) {
        if let Some(gids) = self.route_gids.remove(name)
            && let Some(rd) = &self.ros_discovery
        {
            for g in gids.readers {
                rd.remove_reader(g);
            }
            for g in gids.writers {
                rd.remove_writer(g);
            }
        }
    }

    // ─── local DDS discovery ──────────────────────────────────────────────────

    async fn on_publication(&mut self, ep: DiscoveredEndpoint) -> Result<()> {
        if !self.filter.allows(&ep.topic_name) {
            return Ok(());
        }

        if is_pubsub_topic(&ep.topic_name) {
            let ros2_name = match dds_topic_to_ros2_name(&ep.topic_name) {
                Some(n) => n,
                None => return Ok(()),
            };
            let ros2_type = dds_type_to_ros2_type(&ep.type_name);
            let ud = ep.qos.user_data.as_deref().unwrap_or(&[]);
            let type_info = type_info_from_user_data(&ros2_type, ud);
            let type_hash = type_info.as_ref().map(|ti| ti.hash.clone());
            let qos = BridgeQos {
                reliability: ep.qos.reliability.clone(),
                durability: ep.qos.durability.clone(),
                history: ep.qos.history.clone(),
                durability_service: ep.qos.durability_service.clone(),
                ..Default::default()
            };

            if let Some(entry) = self.pub_routes.get_mut(&ros2_name) {
                // Route already exists (possibly from federation) — register local GID.
                entry.local_gids.insert(ep.key);
                self.gid_to_name.insert(ep.key, ros2_name);
                return Ok(());
            }

            limits::check_topic_cap(self.total_routes())?;
            let bridge = ZDdsPubBridge::new(
                &self.node,
                &ros2_name,
                &ros2_type,
                type_hash,
                &self.participant,
                qos,
                ep.keyless,
                self.cache_multiplier,
            )
            .await?;
            let gids = RouteGids {
                readers: bridge.reader_guid().into_iter().collect(),
                writers: vec![],
            };
            let gid = ep.key;
            self.pub_routes
                .insert(ros2_name.clone(), RouteEntry::from_local(bridge, gid));
            self.register_route_gids(&ros2_name, gids);
            self.gid_to_name.insert(gid, ros2_name);
        } else if is_request_topic(&ep.topic_name) {
            // DDS service client: request writer discovered → create ZDdsClientBridge
            let ros2_name = match dds_topic_to_ros2_name(&ep.topic_name) {
                Some(n) => n,
                None => return Ok(()),
            };
            let ros2_type = if is_action_get_result_topic(&ep.topic_name) {
                dds_type_to_ros2_action_type(&ep.type_name)
            } else {
                dds_type_to_ros2_service_type(&ep.type_name)
            };

            if let Some(entry) = self.cli_routes.get_mut(&ros2_name) {
                entry.local_gids.insert(ep.key);
                self.gid_to_name.insert(ep.key, ros2_name);
                return Ok(());
            }

            let timeout = if is_action_get_result_topic(&ep.topic_name) {
                self.action_get_result_timeout
            } else {
                self.service_timeout
            };
            let ud = ep.qos.user_data.as_deref().unwrap_or(&[]);
            let type_info = type_info_from_user_data(&ros2_type, ud);
            let type_hash = type_info.as_ref().map(|ti| ti.hash.clone());
            limits::check_topic_cap(self.total_routes())?;
            let bridge = ZDdsClientBridge::new(
                &self.node,
                &ros2_name,
                &ros2_type,
                type_hash,
                &self.participant,
                BridgeQos::default(),
                timeout,
            )
            .await?;
            let gids = RouteGids {
                readers: bridge.reader_guid().into_iter().collect(),
                writers: bridge.writer_guid().into_iter().collect(),
            };
            self.cli_routes
                .insert(ros2_name.clone(), RouteEntry::from_local(bridge, ep.key));
            self.register_route_gids(&ros2_name, gids);
            self.gid_to_name.insert(ep.key, ros2_name);
        }
        Ok(())
    }

    async fn on_subscription(&mut self, ep: DiscoveredEndpoint) -> Result<()> {
        if !self.filter.allows(&ep.topic_name) {
            return Ok(());
        }

        if is_pubsub_topic(&ep.topic_name) {
            let ros2_name = match dds_topic_to_ros2_name(&ep.topic_name) {
                Some(n) => n,
                None => return Ok(()),
            };
            let ros2_type = dds_type_to_ros2_type(&ep.type_name);
            let ud = ep.qos.user_data.as_deref().unwrap_or(&[]);
            let type_info = type_info_from_user_data(&ros2_type, ud);
            let type_hash = type_info.as_ref().map(|ti| ti.hash.clone());
            let qos = BridgeQos {
                reliability: ep.qos.reliability.clone(),
                durability: ep.qos.durability.clone(),
                history: ep.qos.history.clone(),
                durability_service: ep.qos.durability_service.clone(),
                ..Default::default()
            };

            if let Some(entry) = self.sub_routes.get_mut(&ros2_name) {
                // Route already exists (possibly from federation) — register local GID.
                entry.local_gids.insert(ep.key);
                self.gid_to_name.insert(ep.key, ros2_name);
                return Ok(());
            }

            limits::check_topic_cap(self.total_routes())?;
            let bridge = ZDdsSubBridge::new(
                &self.node,
                &ros2_name,
                &ros2_type,
                type_hash,
                &self.participant,
                qos,
                ep.keyless,
            )
            .await?;
            let gids = RouteGids {
                readers: vec![],
                writers: bridge.writer_guid().into_iter().collect(),
            };
            let gid = ep.key;
            self.sub_routes
                .insert(ros2_name.clone(), RouteEntry::from_local(bridge, gid));
            self.register_route_gids(&ros2_name, gids);
            self.gid_to_name.insert(gid, ros2_name);
        } else if is_request_topic(&ep.topic_name) {
            // DDS service server: request reader discovered → create ZDdsServiceBridge
            let ros2_name = match dds_topic_to_ros2_name(&ep.topic_name) {
                Some(n) => n,
                None => return Ok(()),
            };
            let ros2_type = if is_action_get_result_topic(&ep.topic_name) {
                dds_type_to_ros2_action_type(&ep.type_name)
            } else {
                dds_type_to_ros2_service_type(&ep.type_name)
            };

            if let Some(entry) = self.srv_routes.get_mut(&ros2_name) {
                entry.local_gids.insert(ep.key);
                self.gid_to_name.insert(ep.key, ros2_name);
                return Ok(());
            }

            limits::check_topic_cap(self.total_routes())?;
            let bridge = ZDdsServiceBridge::new(
                &self.node,
                &ros2_name,
                &ros2_type,
                &self.participant,
                BridgeQos::default(),
            )
            .await?;
            let gids = RouteGids {
                readers: bridge.reader_guid().into_iter().collect(),
                writers: bridge.writer_guid().into_iter().collect(),
            };
            self.srv_routes
                .insert(ros2_name.clone(), RouteEntry::from_local(bridge, ep.key));
            self.register_route_gids(&ros2_name, gids);
            self.gid_to_name.insert(ep.key, ros2_name);
        }
        Ok(())
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Returns the Zenoh liveliness subscription pattern for federation.
///
/// Both RmwZenoh and Ros2Dds formats use `@ros2_lv` as the admin space prefix.
const fn liveliness_subscription_pattern() -> &'static str {
    "@ros2_lv/**"
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::Filter;

    #[test]
    fn test_filter_no_rules_allows_all() {
        let f = Filter::new(None, None).unwrap();
        assert!(f.allows("rt/chatter"));
        assert!(f.allows("rq/add_two_intsRequest"));
    }

    #[test]
    fn test_filter_allow_only_matching() {
        let f = Filter::new(Some("rt/chatter"), None).unwrap();
        assert!(f.allows("rt/chatter"));
        assert!(!f.allows("rt/other"));
    }

    #[test]
    fn test_filter_deny_blocks() {
        let f = Filter::new(None, Some("rt/private.*")).unwrap();
        assert!(!f.allows("rt/private_topic"));
        assert!(f.allows("rt/chatter"));
    }

    #[test]
    fn test_filter_deny_overrides_allow() {
        let f = Filter::new(Some(".*"), Some("rt/secret")).unwrap();
        assert!(!f.allows("rt/secret"));
        assert!(f.allows("rt/chatter"));
    }

    #[test]
    fn test_filter_invalid_regex() {
        assert!(Filter::new(Some("[invalid"), None).is_err());
        assert!(Filter::new(None, Some("[invalid")).is_err());
    }

    #[test]
    fn test_dedup_by_name_same_route_skipped() {
        use std::collections::HashMap;
        let mut routes: HashMap<String, &str> = HashMap::new();
        let name = "/add_two_ints".to_string();

        if !routes.contains_key(&name) {
            routes.insert(name.clone(), "route_a");
        }
        if !routes.contains_key(&name) {
            routes.insert(name.clone(), "route_b");
        }

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[&name], "route_a");
    }

    #[test]
    fn test_dedup_different_names_both_inserted() {
        use std::collections::HashMap;
        let mut routes: HashMap<String, &str> = HashMap::new();

        for name in ["/service_a", "/service_b"] {
            if !routes.contains_key(name) {
                routes.insert(name.to_string(), "route");
            }
        }
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn test_gid_reverse_map_removal() {
        use super::super::gid::Gid;
        use std::collections::HashMap;

        let gid = Gid::from([1u8; 16]);
        let name = "/chatter".to_string();

        let mut routes: HashMap<String, &str> = HashMap::new();
        let mut gid_to_name: HashMap<Gid, String> = HashMap::new();

        routes.insert(name.clone(), "route");
        gid_to_name.insert(gid, name.clone());

        if let Some(removed_name) = gid_to_name.remove(&gid) {
            routes.remove(&removed_name);
        }

        assert!(routes.is_empty());
        assert!(gid_to_name.is_empty());
    }

    #[test]
    fn test_route_entry_lifecycle() {
        use super::super::gid::Gid;

        struct FakeRoute;
        let gid = Gid::from([1u8; 16]);
        let mut entry = super::RouteEntry::from_local(FakeRoute, gid);

        assert!(!entry.is_unused());
        entry.local_gids.remove(&gid);
        assert!(entry.is_unused());
    }

    #[test]
    fn test_route_entry_remote_keeps_alive() {
        use super::super::gid::Gid;

        struct FakeRoute;
        let gid = Gid::from([1u8; 16]);
        let mut entry = super::RouteEntry::from_local(FakeRoute, gid);
        entry.remote_zids.insert("remote-bridge-zid".to_string());

        entry.local_gids.remove(&gid);
        // Remote ZID still present — not unused.
        assert!(!entry.is_unused());

        entry.remote_zids.remove("remote-bridge-zid");
        assert!(entry.is_unused());
    }

    #[test]
    fn test_liveliness_subscription_pattern() {
        assert_eq!(super::liveliness_subscription_pattern(), "@ros2_lv/**");
    }
}
