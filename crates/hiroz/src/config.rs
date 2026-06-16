//! ROS 2 Zenoh configuration builders
//!
//! Generates rmw_zenoh_cpp compatible configs programmatically.
//! All configurations are stored as compile-time constants using LazyLock
//! for zero-cost abstraction after first access.
//!
//! # Architecture
//! - Common overrides: 10 settings shared between router and session
//! - Router-specific: 5 settings unique to router mode
//! - Session-specific: 6 settings unique to peer mode
//!
//! # Example
//! ```no_run
//! # use hiroz::config::{RouterConfigBuilder, router_config, session_config};
//! # #[tokio::main]
//! # async fn main() -> zenoh::Result<()> {
//! // Create router config
//! let router_cfg = router_config()?;
//! let router = zenoh::open(router_cfg).await?;
//!
//! // Create session config
//! let session_cfg = session_config()?;
//! let session = zenoh::open(session_cfg).await?;
//!
//! // Customize router port
//! let custom_router = RouterConfigBuilder::new()
//!     .with_listen_port(7448)
//!     .build_config()?;
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

/// A single configuration override with key, value, and documentation
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConfigOverride {
    /// Configuration key path (e.g., "transport/link/tx/lease")
    pub key: &'static str,
    /// JSON value to set
    pub value: Value,
    /// Human-readable explanation of why this override exists
    pub reason: &'static str,
}

/// ROS 2 distro-specific default configurations
///
/// Different ROS 2 distributions have different default behaviors and feature support.
/// This struct captures those differences to ensure compatibility with rmw_zenoh_cpp.
#[derive(Debug, Clone)]
pub struct DistroDefaults {
    /// Whether this distro supports type hashing
    pub supports_type_hash: bool,
}

impl DistroDefaults {
    /// ROS 2 Humble (LTS) defaults
    ///
    /// - Humble uses rmw_zenoh v0.1.8
    /// - Type hash uses placeholder "TypeHashNotSupported"
    pub const fn humble() -> Self {
        Self {
            supports_type_hash: false,
        }
    }

    /// ROS 2 Kilted defaults
    ///
    /// - Kilted uses rmw_zenoh v0.6.x
    /// - Real type hashes (RIHS01 format)
    pub const fn kilted() -> Self {
        Self {
            supports_type_hash: true,
        }
    }

    /// ROS 2 Lyrical defaults
    ///
    /// - Lyrical uses rmw_zenoh v0.7.x
    /// - Real type hashes (RIHS01 format)
    pub const fn lyrical() -> Self {
        Self {
            supports_type_hash: true,
        }
    }

    /// ROS 2 Rolling defaults
    ///
    /// - Rolling uses rmw_zenoh v0.2.x
    /// - Real type hashes (RIHS01 format)
    pub const fn rolling() -> Self {
        Self {
            supports_type_hash: true,
        }
    }

    /// ROS 2 Jazzy defaults
    ///
    /// - Jazzy uses rmw_zenoh v0.2.9
    /// - Real type hashes (RIHS01 format)
    pub const fn jazzy() -> Self {
        Self {
            supports_type_hash: true,
        }
    }

    /// Get the default for the currently compiled distro based on feature flags
    /// When multiple distro features are enabled (e.g., with --all-features),
    /// priority order is: humble > kilted > lyrical > rolling > jazzy (default)
    pub const fn current() -> Self {
        // Priority 1: Humble
        if cfg!(feature = "humble") {
            return Self::humble();
        }

        // Priority 2: Kilted
        if cfg!(feature = "kilted") {
            return Self::kilted();
        }

        // Priority 3: Lyrical
        if cfg!(feature = "lyrical") {
            return Self::lyrical();
        }

        // Priority 4: Rolling
        if cfg!(feature = "rolling") {
            return Self::rolling();
        }

        // Priority 5 (default): Jazzy
        Self::jazzy()
    }
}

/// Common overrides shared between router and session configs (10 settings)
fn common_overrides() -> &'static [ConfigOverride] {
    static COMMON: LazyLock<Vec<ConfigOverride>> = LazyLock::new(|| {
        vec![
            ConfigOverride {
                key: "scouting/multicast/enabled",
                value: serde_json::json!(false),
                reason: "Disable multicast discovery - use TCP gossip instead",
            },
            ConfigOverride {
                key: "scouting/gossip/target",
                value: serde_json::json!({"router": ["router", "peer"], "peer": ["router"]}),
                reason: "Peers send gossip only to router (not to other peers) to minimize traffic at launch",
            },
            ConfigOverride {
                key: "timestamping/enabled",
                value: serde_json::json!({"router": true, "peer": true, "client": true}),
                reason: "Enable timestamping for peers and clients (required for transient_local durability)",
            },
            ConfigOverride {
                key: "transport/unicast/open_timeout",
                value: serde_json::json!(60000),
                reason: "Increased from 10s to 60s to avoid timeout when opening links with many nodes",
            },
            ConfigOverride {
                key: "transport/unicast/accept_timeout",
                value: serde_json::json!(60000),
                reason: "Increased from 10s to 60s to avoid timeout when accepting links with many nodes",
            },
            ConfigOverride {
                key: "transport/unicast/accept_pending",
                value: serde_json::json!(10000),
                reason: "Increased from 100 to 10000 to handle many simultaneous connection handshakes",
            },
            ConfigOverride {
                key: "transport/unicast/max_sessions",
                value: serde_json::json!(10000),
                reason: "Increased from 1000 to 10000 to support large number of concurrent sessions",
            },
            ConfigOverride {
                key: "transport/link/tx/lease",
                value: serde_json::json!(60000),
                reason: "Increased from 10s to 60s to avoid lease expiration at launch with many nodes",
            },
            ConfigOverride {
                key: "transport/link/tx/keep_alive",
                value: serde_json::json!(2),
                reason: "Decreased from 4 to 2 for loopback where packet loss is minimal",
            },
            ConfigOverride {
                key: "transport/shared_memory/enabled",
                value: serde_json::json!(false),
                reason: "Disabled by default until fully tested in production ROS environments",
            },
        ]
    });

    &COMMON
}

/// Router-specific overrides (5 settings)
fn router_specific_overrides() -> &'static [ConfigOverride] {
    static ROUTER_SPECIFIC: LazyLock<Vec<ConfigOverride>> = LazyLock::new(|| {
        vec![
            ConfigOverride {
                key: "mode",
                value: serde_json::json!("router"),
                reason: "Router mode required for ROS 2 discovery/routing",
            },
            ConfigOverride {
                key: "listen/endpoints",
                value: serde_json::json!(["tcp/[::]:7447"]),
                reason: "Standard ROS 2 port 7447, IPv6 wildcard for all interfaces",
            },
            ConfigOverride {
                key: "connect/endpoints",
                value: serde_json::json!([]),
                reason: "Router does not connect to other endpoints (empty list)",
            },
            ConfigOverride {
                key: "routing/router/peers_failover_brokering",
                value: serde_json::json!(false),
                reason: "Changed from true to false - unnecessary when peers connect directly, reduces overhead",
            },
            ConfigOverride {
                key: "transport/link/tx/queue/congestion_control/block/wait_before_close",
                value: serde_json::json!(5000000),
                reason: "Keep at 5s (vs 60s for session) - router routes to WiFi, lower value prevents long blocks",
            },
        ]
    });

    &ROUTER_SPECIFIC
}

/// Session-specific overrides (6 settings)
fn session_specific_overrides() -> &'static [ConfigOverride] {
    static SESSION_SPECIFIC: LazyLock<Vec<ConfigOverride>> = LazyLock::new(|| {
        vec![
            ConfigOverride {
                key: "mode",
                value: serde_json::json!("peer"),
                reason: "Peer mode for ROS nodes - connects to router for discovery and routing",
            },
            ConfigOverride {
                key: "connect/endpoints",
                value: serde_json::json!(["tcp/localhost:7447"]),
                reason: "Connect to Zenoh router on localhost at standard ROS 2 port 7447",
            },
            ConfigOverride {
                key: "listen/endpoints",
                value: serde_json::json!(["tcp/localhost:0"]),
                reason: "Accept connections only from localhost - external traffic routed via router",
            },
            ConfigOverride {
                key: "scouting/gossip/autoconnect_strategy",
                value: serde_json::json!({"peer": {"to_router": "always", "to_peer": "greater-zid"}}),
                reason: "Changed peer-to-peer from 'always' to 'greater-zid' to avoid redundant connections on loopback",
            },
            ConfigOverride {
                key: "queries_default_timeout",
                value: serde_json::json!(60000),
                reason: "Increased from 10s to 60s to handle slow service servers at launch",
            },
            ConfigOverride {
                key: "transport/link/tx/queue/congestion_control/block/wait_before_close",
                value: serde_json::json!(60000000),
                reason: "Increased from 5s to 60s to avoid premature link closure during launch congestion on loopback",
            },
        ]
    });

    &SESSION_SPECIFIC
}

/// Complete router configuration (cached statically)
///
/// Returns a Vec containing all router-specific and common overrides.
/// The Vec is cloned from static storage for modification if needed.
pub fn router_overrides() -> Vec<ConfigOverride> {
    let mut overrides =
        Vec::with_capacity(router_specific_overrides().len() + common_overrides().len());
    overrides.extend_from_slice(router_specific_overrides());
    overrides.extend_from_slice(common_overrides());
    overrides
}

/// Complete session configuration (cached statically)
///
/// Returns a Vec containing all session-specific and common overrides.
/// The Vec is cloned from static storage for modification if needed.
pub fn session_overrides() -> Vec<ConfigOverride> {
    let mut overrides =
        Vec::with_capacity(session_specific_overrides().len() + common_overrides().len());
    overrides.extend_from_slice(session_specific_overrides());
    overrides.extend_from_slice(common_overrides());
    overrides
}
/// Config key controlling transport-level shared memory.
pub(crate) const TRANSPORT_SHM_ENABLED_KEY: &str = "transport/shared_memory/enabled";

/// Enable transport-level shared memory on an existing config.
///
/// The common session/router overrides force `transport/shared_memory/enabled`
/// to `false` by default. When an SHM provider is configured, the transport must
/// also be told to accept SHM buffers, otherwise zenoh silently copies the SHM
/// contents into the regular network message and the zero-copy path never
/// engages. This applies the override that re-enables it.
pub(crate) fn enable_transport_shm(config: &mut zenoh::Config) -> zenoh::Result<()> {
    config.insert_json5(TRANSPORT_SHM_ENABLED_KEY, "true")?;
    Ok(())
}

/// Build a Zenoh config from a set of overrides
fn build_config(overrides: &[ConfigOverride]) -> zenoh::Result<zenoh::Config> {
    let mut config = zenoh::Config::default();
    for override_ in overrides {
        let value_str = serde_json::to_string(&override_.value)?;
        config.insert_json5(override_.key, &value_str)?;
    }
    Ok(config)
}

/// Create a router configuration matching rmw_zenoh_cpp defaults
///
/// # Example
/// ```no_run
/// # use hiroz::config::router_config;
/// # #[tokio::main]
/// # async fn main() -> zenoh::Result<()> {
/// let config = router_config()?;
/// let router = zenoh::open(config).await?;
/// # Ok(())
/// # }
/// ```
pub fn router_config() -> zenoh::Result<zenoh::Config> {
    build_config(&router_overrides())
}

/// Create a session configuration matching rmw_zenoh_cpp defaults
///
/// # Example
/// ```no_run
/// # use hiroz::config::session_config;
/// # #[tokio::main]
/// # async fn main() -> zenoh::Result<()> {
/// let config = session_config()?;
/// let session = zenoh::open(config).await?;
/// # Ok(())
/// # }
/// ```
pub fn session_config() -> zenoh::Result<zenoh::Config> {
    build_config(&session_overrides())
}

/// Build-time JSON5 generator with comments
///
/// Generates a JSON5 file with inline comments explaining each override.
/// Converts path notation (e.g., "connect/endpoints") into nested JSON structure.
/// Useful for generating reference configuration files.
///
/// # Example
/// ```rust
/// # use hiroz::config::{generate_json5, router_overrides};
/// let json5 = generate_json5(&router_overrides(), "Router Config");
/// std::fs::write("router_config.json5", json5)?;
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn generate_json5(overrides: &[ConfigOverride], name: &str) -> String {
    use serde_json::Value as JsonValue;
    use std::collections::BTreeMap;

    let mut output = format!("// GENERATED: {} - DO NOT EDIT\n", name);
    output.push_str("// This file is auto-generated from hiroz/src/config.rs\n");
    output.push_str("// Edit the source file and rebuild to make changes\n");

    // Build nested structure from path notation
    let mut root = JsonValue::Object(serde_json::Map::new());
    let mut comments: BTreeMap<String, String> = BTreeMap::new();

    for override_ in overrides {
        let path_parts: Vec<&str> = override_.key.split('/').collect();

        // Store comment at the full path
        comments.insert(override_.key.to_string(), override_.reason.to_string());

        // Navigate/create nested structure
        let mut current = &mut root;
        for (i, part) in path_parts.iter().enumerate() {
            if i == path_parts.len() - 1 {
                // Last part - insert the value
                if let JsonValue::Object(map) = current {
                    map.insert(part.to_string(), override_.value.clone());
                }
            } else {
                // Intermediate part - ensure object exists
                if let JsonValue::Object(map) = current {
                    current = map
                        .entry(part.to_string())
                        .or_insert_with(|| JsonValue::Object(serde_json::Map::new()));
                }
            }
        }
    }

    // Generate JSON5 with comments
    output.push_str(&generate_json5_with_comments(&root, &comments, "", 0));
    output
}

/// Helper function to recursively generate JSON5 with inline comments
fn generate_json5_with_comments(
    value: &serde_json::Value,
    comments: &std::collections::BTreeMap<String, String>,
    current_path: &str,
    indent_level: usize,
) -> String {
    use serde_json::Value as JsonValue;

    let indent = "  ".repeat(indent_level);
    let mut output = String::new();

    match value {
        JsonValue::Object(map) => {
            output.push_str("{\n");
            let entries: Vec<_> = map.iter().collect();
            for (i, (key, val)) in entries.iter().enumerate() {
                let new_path = if current_path.is_empty() {
                    key.to_string()
                } else {
                    format!("{}/{}", current_path, key)
                };

                // Add comment if exists for this path
                if let Some(comment) = comments.get(&new_path) {
                    output.push_str(&format!("{}  // {}\n", indent, comment));
                }

                output.push_str(&format!("{}  \"{}\": ", indent, key));

                let nested =
                    generate_json5_with_comments(val, comments, &new_path, indent_level + 1);
                // Trim the nested output if it's a simple value
                let nested_trimmed = if matches!(val, JsonValue::Object(_) | JsonValue::Array(_)) {
                    nested
                } else {
                    nested.trim().to_string()
                };
                output.push_str(&nested_trimmed);

                if i < entries.len() - 1 {
                    output.push_str(",\n");
                } else {
                    output.push('\n');
                }
            }
            output.push_str(&format!("{}}}", indent));
        }
        JsonValue::Array(arr) => {
            if arr.is_empty() {
                output.push_str("[]");
            } else if arr
                .iter()
                .all(|v| !matches!(v, JsonValue::Object(_) | JsonValue::Array(_)))
            {
                // Inline simple arrays
                output.push('[');
                for (i, item) in arr.iter().enumerate() {
                    output.push_str(&serde_json::to_string(item).unwrap());
                    if i < arr.len() - 1 {
                        output.push_str(", ");
                    }
                }
                output.push(']');
            } else {
                // Multi-line for complex arrays
                output.push_str("[\n");
                for (i, item) in arr.iter().enumerate() {
                    output.push_str(&format!("{}  ", indent));
                    output.push_str(
                        generate_json5_with_comments(
                            item,
                            comments,
                            current_path,
                            indent_level + 1,
                        )
                        .trim(),
                    );
                    if i < arr.len() - 1 {
                        output.push(',');
                    }
                    output.push('\n');
                }
                output.push_str(&format!("{}]", indent));
            }
        }
        other => {
            output.push_str(&serde_json::to_string_pretty(other).unwrap());
        }
    }

    output
}

/// Builder for router configuration with customization options
#[derive(Clone)]
pub struct RouterConfigBuilder {
    overrides: Vec<ConfigOverride>,
}

impl RouterConfigBuilder {
    /// Create a new router config builder with default ROS settings
    pub fn new() -> Self {
        Self {
            overrides: router_overrides(),
        }
    }

    /// Change the listen port (default: 7447)
    ///
    /// # Example
    /// ```rust
    /// # use hiroz::config::RouterConfigBuilder;
    /// let config = RouterConfigBuilder::new()
    ///     .with_listen_port(7448)
    ///     .build_config()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_listen_port(mut self, port: u16) -> Self {
        if let Some(listen) = self
            .overrides
            .iter_mut()
            .find(|o| o.key == "listen/endpoints")
        {
            listen.value = serde_json::json!([format!("tcp/[::]:{}", port)]);
        }
        self
    }

    /// Set a custom listen endpoint
    ///
    /// # Example
    /// ```rust
    /// # use hiroz::config::RouterConfigBuilder;
    /// let config = RouterConfigBuilder::new()
    ///     .with_listen_endpoint("tcp/0.0.0.0:7447")
    ///     .build_config()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_listen_endpoint(mut self, endpoint: &str) -> Self {
        if let Some(listen) = self
            .overrides
            .iter_mut()
            .find(|o| o.key == "listen/endpoints")
        {
            listen.value = serde_json::json!([endpoint]);
        }
        self
    }

    /// Enable transport-level shared memory on the router.
    ///
    /// The default router config disables `transport/shared_memory/enabled` (see
    /// [`common_overrides`]). A router (or `zenohd`) that relays SHM traffic
    /// between co-located peers must have transport SHM enabled, otherwise the
    /// zero-copy path is broken end-to-end even when both peers request SHM.
    ///
    /// # Example
    /// ```rust
    /// # use hiroz::config::RouterConfigBuilder;
    /// let config = RouterConfigBuilder::new()
    ///     .with_shm_enabled()
    ///     .build_config()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_shm_enabled(self) -> Self {
        self.with_override(
            TRANSPORT_SHM_ENABLED_KEY,
            serde_json::json!(true),
            "Enable transport SHM so the router can relay zero-copy buffers",
        )
    }

    /// Override a specific config key
    ///
    /// Replaces existing override if key exists, otherwise adds new one.
    ///
    /// # Example
    /// ```rust
    /// # use hiroz::config::RouterConfigBuilder;
    /// let config = RouterConfigBuilder::new()
    ///     .with_override(
    ///         "transport/unicast/max_sessions",
    ///         serde_json::json!(20000),
    ///         "Custom increased sessions"
    ///     )
    ///     .build_config()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_override(mut self, key: &'static str, value: Value, reason: &'static str) -> Self {
        if let Some(existing) = self.overrides.iter_mut().find(|o| o.key == key) {
            existing.value = value;
            existing.reason = reason;
        } else {
            self.overrides.push(ConfigOverride { key, value, reason });
        }
        self
    }

    /// Build the Zenoh config.
    ///
    /// Also available via the [`Builder`](crate::Builder) trait.
    pub fn build_config(self) -> zenoh::Result<zenoh::Config> {
        build_config(&self.overrides)
    }
}

impl Default for RouterConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for session configuration with customization options
#[derive(Clone)]
pub struct SessionConfigBuilder {
    overrides: Vec<ConfigOverride>,
}

impl SessionConfigBuilder {
    /// Create a new session config builder with default ROS settings
    pub fn new() -> Self {
        Self {
            overrides: session_overrides(),
        }
    }

    /// Change the router endpoint to connect to (default: tcp/localhost:7447)
    ///
    /// # Example
    /// ```rust
    /// # use hiroz::config::SessionConfigBuilder;
    /// let config = SessionConfigBuilder::new()
    ///     .with_router_endpoint("tcp/192.168.1.100:7447")
    ///     .build_config()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_router_endpoint(mut self, endpoint: &str) -> Self {
        if let Some(connect) = self
            .overrides
            .iter_mut()
            .find(|o| o.key == "connect/endpoints")
        {
            connect.value = serde_json::json!([endpoint]);
        }
        self
    }

    /// Override a specific config key
    ///
    /// Replaces existing override if key exists, otherwise adds new one.
    ///
    /// # Example
    /// ```rust
    /// # use hiroz::config::SessionConfigBuilder;
    /// let config = SessionConfigBuilder::new()
    ///     .with_override(
    ///         "queries_default_timeout",
    ///         serde_json::json!(120000),
    ///         "Increased timeout for slow network"
    ///     )
    ///     .build_config()?;
    /// # Ok::<(), zenoh::Error>(())
    /// ```
    pub fn with_override(mut self, key: &'static str, value: Value, reason: &'static str) -> Self {
        if let Some(existing) = self.overrides.iter_mut().find(|o| o.key == key) {
            existing.value = value;
            existing.reason = reason;
        } else {
            self.overrides.push(ConfigOverride { key, value, reason });
        }
        self
    }

    /// Build the Zenoh config.
    ///
    /// Also available via the [`Builder`](crate::Builder) trait.
    pub fn build_config(self) -> zenoh::Result<zenoh::Config> {
        build_config(&self.overrides)
    }
}

impl Default for SessionConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Builder;

    #[test]
    fn test_common_overrides_shared() {
        let router = router_overrides();
        let session = session_overrides();
        let common = common_overrides();

        // Verify common overrides are present in both
        for common_override in common {
            assert!(
                router.iter().any(|o| o.key == common_override.key),
                "Router missing common override: {}",
                common_override.key
            );
            assert!(
                session.iter().any(|o| o.key == common_override.key),
                "Session missing common override: {}",
                common_override.key
            );
        }
    }

    #[test]
    fn test_router_config_creates_valid_session() {
        // Use port 0 to let OS assign an available port
        let config = RouterConfigBuilder::new()
            .with_listen_endpoint("tcp/[::]:0")
            .build()
            .expect("Failed to build router config");

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let session = zenoh::open(config).await;
            assert!(
                session.is_ok(),
                "Failed to create Zenoh session with router config: {:?}",
                session.err()
            );
        });
    }

    #[test]
    fn test_session_config_creates_valid_session() {
        let config = session_config().expect("Failed to build session config");

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let session = zenoh::open(config).await;
            assert!(
                session.is_ok(),
                "Failed to create Zenoh session with peer config: {:?}",
                session.err()
            );
        });
    }

    #[test]
    fn test_all_overrides_produce_valid_config() {
        // Test router overrides
        for override_ in &router_overrides() {
            let mut config = zenoh::Config::default();
            let value_str = serde_json::to_string(&override_.value).unwrap();
            let result = config.insert_json5(override_.key, &value_str);
            assert!(
                result.is_ok(),
                "Router override '{}' is invalid: {:?}",
                override_.key,
                result.err()
            );
        }

        // Test session overrides
        for override_ in &session_overrides() {
            let mut config = zenoh::Config::default();
            let value_str = serde_json::to_string(&override_.value).unwrap();
            let result = config.insert_json5(override_.key, &value_str);
            assert!(
                result.is_ok(),
                "Session override '{}' is invalid: {:?}",
                override_.key,
                result.err()
            );
        }
    }

    #[test]
    fn test_router_builder_custom_port() {
        let config = RouterConfigBuilder::new()
            .with_listen_port(7448)
            .build()
            .expect("Failed to build router config");

        assert_eq!(config.mode().unwrap().to_string(), "router");
    }

    #[test]
    fn test_session_builder_custom_endpoint() {
        let config = SessionConfigBuilder::new()
            .with_router_endpoint("tcp/192.168.1.1:7447")
            .build()
            .expect("Failed to build session config");

        assert_eq!(config.mode().unwrap().to_string(), "peer");
    }

    #[test]
    fn test_builder_with_custom_override() {
        let config = RouterConfigBuilder::new()
            .with_override(
                "transport/unicast/max_sessions",
                serde_json::json!(20000),
                "Custom increased sessions",
            )
            .build()
            .expect("Failed to build");

        assert_eq!(config.mode().unwrap().to_string(), "router");
    }

    #[test]
    fn test_generate_json5_router() {
        let json5 = generate_json5(&router_overrides(), "Test Router Config");
        assert!(json5.contains("GENERATED"));
        assert!(json5.contains("mode"));
        assert!(json5.contains("router"));
    }

    #[test]
    fn test_common_default_disables_transport_shm() {
        let common = common_overrides();
        let shm = common
            .iter()
            .find(|o| o.key == TRANSPORT_SHM_ENABLED_KEY)
            .expect("common overrides should set transport shared_memory");
        assert_eq!(shm.value, serde_json::json!(false));
    }

    #[test]
    fn test_enable_transport_shm_overrides_default() {
        // Start from the session config, which disables transport SHM by default.
        let mut config = session_config().expect("Failed to build session config");
        assert_eq!(
            config.get_json(TRANSPORT_SHM_ENABLED_KEY).unwrap(),
            "false",
            "session config should disable transport SHM by default"
        );

        enable_transport_shm(&mut config).expect("Failed to enable transport SHM");
        assert_eq!(
            config.get_json(TRANSPORT_SHM_ENABLED_KEY).unwrap(),
            "true",
            "enable_transport_shm should flip the value to true"
        );
    }

    #[test]
    fn test_router_builder_with_shm_enabled() {
        let config = RouterConfigBuilder::new()
            .with_shm_enabled()
            .build_config()
            .expect("Failed to build router config with SHM");
        assert_eq!(config.get_json(TRANSPORT_SHM_ENABLED_KEY).unwrap(), "true");
    }

    #[test]
    fn test_generate_json5_session() {
        let json5 = generate_json5(&session_overrides(), "Test Session Config");
        assert!(json5.contains("GENERATED"));
        assert!(json5.contains("mode"));
        assert!(json5.contains("peer"));
    }
}
