use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicUsize},
};

use tracing::{debug, warn};
use zenoh::{Result, Session, Wait};

pub use hiroz_protocol::KeyExprFormat;

use crate::{
    Builder,
    entity::normalize_node_namespace,
    graph::Graph,
    node::ZNodeBuilder,
    time::{ClockKind, ZClock},
};

#[derive(Debug, Default)]
pub struct GlobalCounter(AtomicUsize);

impl GlobalCounter {
    pub fn increment(&self) -> usize {
        self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel)
    }
}

use std::path::PathBuf;

use serde_json::json;

/// Remapping rules for ROS names
#[derive(Debug, Clone, Default)]
pub struct RemapRules {
    rules: HashMap<String, String>,
}

impl RemapRules {
    /// Create a new empty remap rules set
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a remapping rule
    /// Format: "from:=to"
    pub fn add_rule(&mut self, rule: &str) -> Result<()> {
        if let Some((from, to)) = rule.split_once(":=") {
            if from.is_empty() || to.is_empty() {
                return Err("Invalid remap rule: both sides must be non-empty".into());
            }
            self.rules.insert(from.to_string(), to.to_string());
            Ok(())
        } else {
            Err("Invalid remap rule format: expected 'from:=to'".into())
        }
    }

    /// Apply remapping to a name
    pub fn apply(&self, name: &str) -> String {
        if let Some(remapped) = self.rules.get(name) {
            debug!("[CTX] Remapped '{}' -> '{}'", name, remapped);
            remapped.clone()
        } else {
            name.to_string()
        }
    }

    /// Check if any rules are defined
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

pub struct ZContextBuilder {
    domain_id: DomainId,
    namespace: String,
    enclave: String,
    zenoh_config: Option<zenoh::Config>,
    config_file: Option<PathBuf>,
    config_overrides: Vec<(String, serde_json::Value)>,
    remap_rules: RemapRules,
    enable_logging: bool,
    shm_config: Option<Arc<crate::shm::ShmConfig>>,
    keyexpr_format: hiroz_protocol::KeyExprFormat,
    clock: Option<ZClock>,
}

/// The builder's resolved (or pending) ROS domain id.
///
/// `Invalid` only exists between `ZContextBuilder::default()` and
/// `.build()`/`.with_domain_id()` -- a live `ZContext` always carries a
/// concrete `usize`. Modeling "haven't resolved an invalid ROS_DOMAIN_ID
/// yet" as a variant, rather than a `usize` plus a side-channel error
/// field, makes it a state the type carries instead of an invariant call
/// sites have to remember to check (and that `with_domain_id()` has to
/// remember to clear).
#[derive(Debug, Clone, PartialEq, Eq)]
enum DomainId {
    /// A concrete domain, either explicit or read from `ROS_DOMAIN_ID`.
    Value(usize),
    /// `ROS_DOMAIN_ID` was set but is not a valid non-negative integer.
    /// `build()` rejects this unless `.with_domain_id()` overrides it
    /// first. Falling back to domain 0 silently would put a node on the
    /// wrong ROS graph after an operator typo; `rcl_get_default_domain_id`
    /// treats this the same way, returning an error that aborts
    /// `rcl_init` rather than defaulting.
    Invalid(String),
}

impl DomainId {
    /// Matches `rclcpp`/`rclpy`: read `ROS_DOMAIN_ID` from the environment,
    /// so the normal ROS 2 deployment story (set the env var, don't touch
    /// source) works here too. `.with_domain_id()` called after
    /// `default()` still overrides this, same precedence as every other
    /// ROS 2 client library.
    fn from_env() -> Self {
        Self::parse(std::env::var("ROS_DOMAIN_ID").ok())
    }

    /// Pure parsing, taking the env var's value directly rather than
    /// reading it -- so this is unit-testable without mutating (and
    /// racing on) real process-global state.
    fn parse(value: Option<String>) -> Self {
        match value {
            Some(val) => match val.parse::<usize>() {
                Ok(id) => Self::Value(id),
                Err(_) => Self::Invalid(val),
            },
            None => Self::Value(0),
        }
    }
}

impl std::fmt::Display for DomainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(id) => write!(f, "{id}"),
            Self::Invalid(raw) => write!(f, "<invalid ROS_DOMAIN_ID {raw:?}>"),
        }
    }
}

impl Default for ZContextBuilder {
    fn default() -> Self {
        Self {
            domain_id: DomainId::from_env(),
            namespace: String::default(),
            enclave: String::default(),
            zenoh_config: None,
            config_file: None,
            config_overrides: Vec::default(),
            remap_rules: RemapRules::default(),
            enable_logging: bool::default(),
            shm_config: None,
            keyexpr_format: hiroz_protocol::KeyExprFormat::default(),
            clock: None,
        }
    }
}

impl ZContextBuilder {
    /// Set the ROS domain ID, overriding `ROS_DOMAIN_ID` (and any error
    /// parsing it) with an explicit value.
    pub fn with_domain_id(mut self, domain_id: usize) -> Self {
        self.domain_id = DomainId::Value(domain_id);
        self
    }

    /// Set the default namespace inherited by nodes created from this context.
    pub fn with_namespace(mut self, namespace: impl AsRef<str>) -> Self {
        self.namespace = normalize_node_namespace(namespace.as_ref());
        self
    }

    /// Set the enclave name
    pub fn with_enclave<S: Into<String>>(mut self, enclave: S) -> Self {
        self.enclave = enclave.into();
        self
    }

    /// Set the key expression format for ROS 2 entity mapping and graph discovery.
    ///
    /// # Example
    /// ```ignore
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    /// use hiroz_protocol::KeyExprFormat;
    ///
    /// // Default (RmwZenoh)
    /// let ctx = ZContextBuilder::default().build()?;
    ///
    /// // Explicit format selection
    /// let ctx = ZContextBuilder::default()
    ///     .keyexpr_format(KeyExprFormat::RmwZenoh)
    ///     .build()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn keyexpr_format(mut self, format: hiroz_protocol::KeyExprFormat) -> Self {
        self.keyexpr_format = format;
        self
    }

    /// Load configuration from a JSON file
    pub fn with_config_file<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.config_file = Some(path.into());
        self
    }

    /// Add a JSON configuration override
    ///
    /// # Example
    /// ```
    /// use serde_json::json;
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// let ctx = ZContextBuilder::default()
    ///     .with_json("scouting/multicast/enabled", json!(false))
    ///     .with_json("connect/endpoints", json!(["tcp/127.0.0.1:7447"]))
    ///     .build()
    ///     .expect("Failed to build context");
    /// ```
    pub fn with_json<K: Into<String>, V: serde::Serialize>(mut self, key: K, value: V) -> Self {
        let key = key.into();
        let value_json = serde_json::to_value(&value)
            .unwrap_or_else(|_| panic!("Failed to serialize value for key: {}", key));
        self.config_overrides.push((key, value_json));
        self
    }

    /// Convenience method: disable multicast scouting
    pub fn disable_multicast_scouting(self) -> Self {
        self.with_json("scouting/multicast/enabled", json!(false))
    }

    /// Convenience method: connect to specific endpoints
    ///
    /// # Example
    /// ```
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// let ctx = ZContextBuilder::default()
    ///     .with_connect_endpoints(["tcp/127.0.0.1:7447"])
    ///     .build()
    ///     .expect("Failed to build context");
    /// ```
    pub fn with_connect_endpoints<I, S>(self, endpoints: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let endpoints: Vec<String> = endpoints.into_iter().map(|s| s.into()).collect();
        self.with_json("connect/endpoints", json!(endpoints))
    }

    /// Convenience method: listen on specific endpoints
    ///
    /// By default, `ZContextBuilder` will build a context that only listens for
    /// connections from localhost. To change this so that it, for example, listens
    /// on all interfaces, use this method as in the example below.
    ///
    /// # Example
    /// ```
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// let ctx = ZContextBuilder::default()
    ///     .with_listen_endpoints(["tcp/[::]:0"])
    ///     .build()
    ///     .expect("Failed to build context");
    /// ```
    pub fn with_listen_endpoints<I, S>(self, endpoints: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let endpoints: Vec<String> = endpoints.into_iter().map(|s| s.into()).collect();
        self.with_json("listen/endpoints", json!(endpoints))
    }

    /// Convenience method: connect to localhost zenohd
    pub fn connect_to_local_zenohd(self) -> Self {
        self.with_connect_endpoints(["tcp/127.0.0.1:7447"])
    }

    /// Convenience method: set mode (peer, client, router)
    pub fn with_mode<S: Into<String>>(self, mode: S) -> Self {
        self.with_json("mode", json!(mode.into()))
    }

    /// Override the default ROS session config with a custom Zenoh configuration
    ///
    /// # Example
    /// ```
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// let custom_config = zenoh::Config::default();
    /// let ctx = ZContextBuilder::default()
    ///     .with_zenoh_config(custom_config)
    ///     .build()
    ///     .expect("Failed to build context");
    /// ```
    pub fn with_zenoh_config(mut self, config: zenoh::Config) -> Self {
        self.zenoh_config = Some(config);
        self
    }

    /// Customize the default ROS session config to connect to a specific router endpoint
    ///
    /// # Example
    /// ```
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// let ctx = ZContextBuilder::default()
    ///     .with_router_endpoint("tcp/192.168.1.1:7447")?
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_router_endpoint<S: Into<String>>(mut self, endpoint: S) -> Result<Self> {
        let session_config = crate::config::SessionConfigBuilder::new()
            .with_router_endpoint(&endpoint.into())
            .build_config()?;
        self.zenoh_config = Some(session_config);
        Ok(self)
    }

    /// Add a name remapping rule
    ///
    /// # Arguments
    /// * `rule` - Remapping rule in format "from:=to"
    ///
    /// # Example
    /// ```
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// let ctx = ZContextBuilder::default()
    ///     .with_remap_rule("/foo:=/bar")?
    ///     .with_remap_rule("__node:=my_node")?
    ///     .build()
    ///     .expect("Failed to build context");
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_remap_rule<S: Into<String>>(mut self, rule: S) -> Result<Self> {
        self.remap_rules.add_rule(&rule.into())?;
        Ok(self)
    }

    /// Add multiple remapping rules
    ///
    /// # Arguments
    /// * `rules` - Iterator of remapping rules in format "from:=to"
    pub fn with_remap_rules<I, S>(mut self, rules: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for rule in rules {
            self.remap_rules.add_rule(&rule.into())?;
        }
        Ok(self)
    }

    /// Enable Zenoh logging initialization with default level "error"
    pub fn with_logging_enabled(mut self) -> Self {
        self.enable_logging = true;
        self
    }

    /// Select the clock kind used by this context and all nodes created from it.
    pub fn with_clock_kind(mut self, kind: ClockKind) -> Self {
        self.clock = Some(ZClock::from_kind(kind));
        self
    }

    /// Inject a pre-configured clock.
    pub fn with_clock(mut self, clock: ZClock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Enable SHM with default pool size (10MB) and threshold (512 bytes). Also enables transport SHM on the session.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// let ctx = ZContextBuilder::default()
    ///     .with_shm_enabled()?
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_enabled(self) -> Result<Self> {
        let provider = Arc::new(
            crate::shm::ShmProviderBuilder::new(crate::shm::DEFAULT_SHM_POOL_SIZE).build()?,
        );
        Ok(self.with_shm_config(crate::shm::ShmConfig::new(provider)))
    }

    /// Enable SHM with custom pool size.
    ///
    /// # Arguments
    /// * `size_bytes` - Pool size in bytes
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// let ctx = ZContextBuilder::default()
    ///     .with_shm_pool_size(100 * 1024 * 1024)?  // 100MB
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_pool_size(self, size_bytes: usize) -> Result<Self> {
        let provider = Arc::new(crate::shm::ShmProviderBuilder::new(size_bytes).build()?);
        Ok(self.with_shm_config(crate::shm::ShmConfig::new(provider)))
    }

    /// Set custom SHM configuration.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::shm::{ShmConfig, ShmProviderBuilder};
    /// use hiroz::Builder;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// let provider = Arc::new(ShmProviderBuilder::new(50 * 1024 * 1024).build()?);
    /// let config = ShmConfig::new(provider).with_threshold(10_000);
    ///
    /// let ctx = ZContextBuilder::default()
    ///     .with_shm_config(config)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_config(mut self, config: crate::shm::ShmConfig) -> Self {
        self.shm_config = Some(Arc::new(config));
        self
    }

    /// Set SHM threshold (minimum message size for SHM).
    ///
    /// Only effective if SHM has been enabled via `with_shm_enabled()` or similar.
    ///
    /// # Example
    /// ```no_run
    /// use hiroz::context::ZContextBuilder;
    /// use hiroz::Builder;
    ///
    /// # fn main() -> zenoh::Result<()> {
    /// let ctx = ZContextBuilder::default()
    ///     .with_shm_enabled()?
    ///     .with_shm_threshold(50_000)  // 50KB threshold
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shm_threshold(mut self, threshold: usize) -> Self {
        if let Some(ref mut config) = self.shm_config {
            // Need to modify Arc content - make it unique or clone
            let mut new_config = (**config).clone();
            new_config = new_config.with_threshold(threshold);
            self.shm_config = Some(Arc::new(new_config));
        }
        self
    }

    /// Parse and apply overrides from environment variable
    ///
    /// Expected format: `key1=value1;key2=value2`
    /// Values should be valid JSON5
    ///
    /// # Example
    /// ```
    /// // In shell:
    /// // export ZENOH_CONFIG_OVERRIDE='mode="client";connect/endpoints=["tcp/192.168.1.1:7447"]'
    /// ```
    fn apply_env_overrides(mut self) -> Result<Self> {
        if let Ok(overrides_str) = std::env::var("ZENOH_CONFIG_OVERRIDE") {
            tracing::debug!(
                "Applying config overrides from ZENOH_CONFIG_OVERRIDE: {}",
                overrides_str
            );

            // Parse semicolon-separated key=value pairs
            for pair in overrides_str.split(';') {
                let pair = pair.trim();
                if pair.is_empty() {
                    continue;
                }

                // Split on first '=' only
                if let Some((key, value)) = pair.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();

                    // Parse JSON5 value
                    match json5::from_str::<serde_json::Value>(value) {
                        Ok(json_value) => {
                            tracing::debug!("Override: {} = {}", key, json_value);
                            self.config_overrides.push((key.to_string(), json_value));
                        }
                        Err(e) => {
                            return Err(format!(
                                "Failed to parse ZENOH_CONFIG_OVERRIDE value for key '{}': {} (value: {})",
                                key, e, value
                            ).into());
                        }
                    }
                } else {
                    return Err(format!(
                        "Invalid ZENOH_CONFIG_OVERRIDE format: '{}'. Expected 'key=value'",
                        pair
                    )
                    .into());
                }
            }
        }

        Ok(self)
    }
}

impl Builder for ZContextBuilder {
    type Output = ZContext;

    #[tracing::instrument(name = "ctx_build", skip(self), fields(
        domain_id = %self.domain_id,
        config_file = ?self.config_file
    ))]
    fn build(self) -> Result<ZContext> {
        // Priority order:
        // 1. Custom Zenoh config passed via with_zenoh_config()
        // 2. Config file passed via with_config_file()
        // 3. ZENOH_SESSION_CONFIG_URI environment variable (same as rmw_zenoh_cpp)
        // 4. **NEW DEFAULT**: ROS session config (connects to router at tcp/localhost:7447)
        //    This matches rmw_zenoh_cpp behavior

        let DomainId::Value(_) = &self.domain_id else {
            return Err(format!(
                "{}: not a valid non-negative integer; set ROS_DOMAIN_ID to a \
                 valid domain or call .with_domain_id() explicitly",
                self.domain_id
            )
            .into());
        };

        debug!(
            "[CTX] Building context: domain_id={}, has_config={}",
            self.domain_id,
            self.config_file.is_some()
        );

        // Capture enclave before moving self
        let enclave = self.enclave.clone();

        // Apply environment variable overrides first
        let builder = self.apply_env_overrides()?;
        debug!(
            "[CTX] Applied {} env overrides",
            builder.config_overrides.len()
        );

        // Initialize logging if enabled
        if builder.enable_logging {
            zenoh::init_log_from_env_or("error");
        }

        let has_custom_config = builder.zenoh_config.is_some();
        let has_config_file = builder.config_file.is_some();
        let has_env_config = std::env::var("ZENOH_SESSION_CONFIG_URI").is_ok();

        let mut config = if let Some(config) = builder.zenoh_config {
            config
        } else if let Some(ref config_file) = builder.config_file {
            // Use explicit config file
            zenoh::Config::from_file(config_file)?
        } else if let Ok(uri) = std::env::var("ZENOH_SESSION_CONFIG_URI") {
            // Use environment variable config URI (same as rmw_zenoh_cpp)
            zenoh::Config::from_file(uri)?
        } else {
            // DEFAULT: Use ROS session config (requires router at localhost:7447)
            // This is the key change - matching rmw_zenoh_cpp behavior
            crate::config::session_config()?
        };

        // common_overrides disables transport SHM; re-enable it when an SHM provider is set.
        if builder.shm_config.is_some() {
            crate::config::enable_transport_shm(&mut config).map_err(|e| {
                format!("Failed to enable transport shared memory for SHM config: {e}")
            })?;
            debug!("[CTX] SHM provider configured: enabled transport/shared_memory");
        }

        // Apply all JSON overrides
        for (key, value) in builder.config_overrides {
            let value_str = serde_json::to_string(&value)
                .map_err(|e| format!("Failed to serialize value for key '{}': {}", key, e))?;

            config.insert_json5(&key, &value_str).map_err(|e| {
                format!(
                    "Failed to apply config override '{}' = '{}': {}",
                    key, value_str, e
                )
            })?;
        }

        // Open Zenoh session
        let session = zenoh::open(config).wait()?;
        debug!("[CTX] Zenoh session opened: zid={}", session.zid());

        // Check if router is running when using default peer mode
        if !has_custom_config && !has_config_file && !has_env_config {
            let mut routers_zid = session.info().routers_zid().wait();
            if routers_zid.next().is_none() {
                warn!("[CTX] No routers connected");
            } else {
                debug!("[CTX] Connected to routers");
            }
        }

        let DomainId::Value(domain_id) = builder.domain_id else {
            unreachable!("build() already rejected a non-Value domain_id above")
        };
        let graph = Arc::new(Graph::new(
            &session,
            domain_id,
            builder.keyexpr_format.clone(),
        )?);

        Ok(ZContext {
            session: Arc::new(session),
            counter: Arc::new(GlobalCounter::default()),
            domain_id,
            namespace: builder.namespace,
            enclave,
            graph,
            remap_rules: builder.remap_rules,
            shm_config: builder.shm_config,
            keyexpr_format: builder.keyexpr_format,
            clock: builder.clock.unwrap_or_default(),
        })
    }
}

/// A live hiroz context backed by an open Zenoh session.
///
/// `ZContext` is the root object for all hiroz communication. Create one with
/// [`ZContextBuilder`] and use it to create [`ZNode`](crate::node::ZNode)s.
///
/// # Example
///
/// ```rust,ignore
/// use hiroz::prelude::*;
///
/// let ctx = ZContextBuilder::default().build()?;
/// let node = ctx.create_node("my_node").build()?;
/// ```
#[derive(Clone)]
pub struct ZContext {
    pub(crate) session: Arc<Session>,
    // Global counter for the participants
    counter: Arc<GlobalCounter>,
    domain_id: usize,
    namespace: String,
    enclave: String,
    graph: Arc<Graph>,
    remap_rules: RemapRules,
    pub(crate) shm_config: Option<Arc<crate::shm::ShmConfig>>,
    pub(crate) keyexpr_format: hiroz_protocol::KeyExprFormat,
    pub(crate) clock: ZClock,
}

impl std::fmt::Debug for ZContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZContext")
            .field("domain_id", &self.domain_id)
            .field("namespace", &self.namespace)
            .field("enclave", &self.enclave)
            .finish_non_exhaustive()
    }
}

impl ZContext {
    /// Create a builder for a new ROS 2 node within this context.
    ///
    /// Create a lifecycle node builder.
    ///
    /// Call `.build()` on the returned builder (requires `use hiroz::Builder;`).
    pub fn create_lifecycle_node<S: AsRef<str>>(
        &self,
        name: S,
    ) -> crate::lifecycle::node::ZLifecycleNodeBuilder {
        crate::lifecycle::node::ZLifecycleNodeBuilder {
            ctx: self.clone(),
            name: name.as_ref().to_owned(),
            namespace: if self.namespace.is_empty() {
                None
            } else {
                Some(self.namespace.clone())
            },
            enable_communication_interface: true,
            type_description_service: false,
        }
    }

    /// Call `.build()` on the returned [`ZNodeBuilder`](crate::node::ZNodeBuilder) to
    /// produce the node. Requires `use hiroz::Builder;` in scope.
    pub fn create_node<S: AsRef<str>>(&self, name: S) -> ZNodeBuilder {
        ZNodeBuilder {
            domain_id: self.domain_id,
            name: name.as_ref().to_owned(),
            namespace: self.namespace.clone(),
            enclave: self.enclave.clone(),
            session: self.session.clone(),
            counter: self.counter.clone(),
            graph: self.graph.clone(),
            remap_rules: self.remap_rules.clone(),
            shm_config: self.shm_config.clone(),
            keyexpr_format: self.keyexpr_format.clone(),
            clock: self.clock.clone(),
            enable_type_desc_service: false,
            enable_parameters: true,
            parameter_overrides: std::collections::HashMap::new(),
        }
    }

    /// Close the underlying Zenoh session, releasing all network resources.
    ///
    /// After calling `shutdown`, all nodes, publishers, subscribers, and
    /// service clients/servers created from this context become invalid.
    pub fn shutdown(&self) -> Result<()> {
        self.session.close().wait()
    }

    /// Get a reference to the graph for setting up event callbacks
    pub fn graph(&self) -> &Arc<crate::graph::Graph> {
        &self.graph
    }

    /// Access the context clock used by nodes and runtime helpers.
    pub fn clock(&self) -> &ZClock {
        &self.clock
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `DomainId::parse` takes the env var's value as a plain argument
    // rather than reading `ROS_DOMAIN_ID` itself, so these are pure unit
    // tests: no process-global mutation, no #[serial], no race with the
    // dozens of other tests elsewhere in this crate that build a
    // `ZContextBuilder::default()` on their own thread.

    #[test]
    fn parse_falls_back_to_zero_when_unset() {
        assert_eq!(DomainId::parse(None), DomainId::Value(0));
    }

    #[test]
    fn parse_reads_a_valid_value() {
        assert_eq!(DomainId::parse(Some("42".to_string())), DomainId::Value(42));
    }

    #[test]
    fn parse_rejects_an_invalid_value() {
        assert_eq!(
            DomainId::parse(Some("not-a-number".to_string())),
            DomainId::Invalid("not-a-number".to_string())
        );
    }

    #[test]
    fn with_domain_id_overrides_an_invalid_parse() {
        let builder = ZContextBuilder {
            domain_id: DomainId::Invalid("garbage".to_string()),
            ..Default::default()
        }
        .with_domain_id(7);
        assert_eq!(builder.domain_id, DomainId::Value(7));
    }
}
