use clap::Parser;
use hiroz_protocol::KeyExprFormat;

fn default_domain_id() -> u32 {
    std::env::var("ROS_DOMAIN_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Zenoh key expression wire format.
#[derive(Debug, Clone, PartialEq, clap::ValueEnum)]
pub enum WireFormat {
    /// rmw_zenoh_cpp compatible format. Interops with ros-z and rmw_zenoh_cpp.
    #[value(name = "rmw-zenoh")]
    RmwZenoh,
}

impl From<WireFormat> for KeyExprFormat {
    fn from(_: WireFormat) -> Self {
        KeyExprFormat::RmwZenoh
    }
}

/// Bridge between DDS-based ROS 2 nodes and a Zenoh/ros-z network.
#[derive(Parser, Debug, Clone)]
#[command(name = "ros-z-bridge-dds")]
pub struct Config {
    /// Zenoh endpoint to connect to.
    #[arg(short, long, default_value = "tcp/127.0.0.1:7447")]
    pub zenoh_endpoint: String,

    /// ROS 2 domain ID. Defaults to ROS_DOMAIN_ID env var, or 0.
    #[arg(short, long, default_value_t = default_domain_id())]
    pub domain_id: u32,

    /// Namespace prefix applied to all bridged topics/services on the Zenoh side.
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Node name used for liveliness tokens.
    #[arg(long, default_value = "zenoh_bridge_dds")]
    pub node_name: String,

    /// Topic allow-list regex. Only DDS topic names matching this pattern are bridged.
    #[arg(long)]
    pub allow: Option<String>,

    /// Topic deny-list regex. DDS topic names matching this pattern are not bridged.
    #[arg(long)]
    pub deny: Option<String>,

    /// Timeout in seconds for Zenoh get() calls on service routes.
    #[arg(long, default_value_t = 10)]
    pub service_timeout_secs: u64,

    /// Timeout in seconds for action get_result Zenoh get() calls.
    #[arg(long, default_value_t = 300)]
    pub action_get_result_timeout_secs: u64,

    /// TRANSIENT_LOCAL AdvancedPublisher cache depth multiplier.
    #[arg(long, default_value_t = 10)]
    pub transient_local_cache_multiplier: usize,

    /// Zenoh key expression wire format (rmw-zenoh only; reserved for future formats).
    #[arg(long, value_enum, default_value = "rmw-zenoh")]
    pub wire_format: WireFormat,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_domain_id_without_env() {
        unsafe { std::env::remove_var("ROS_DOMAIN_ID") };
        assert_eq!(default_domain_id(), 0);
    }

    #[test]
    fn test_default_domain_id_with_env() {
        unsafe { std::env::set_var("ROS_DOMAIN_ID", "42") };
        assert_eq!(default_domain_id(), 42);
        unsafe { std::env::remove_var("ROS_DOMAIN_ID") };
    }

    #[test]
    fn test_default_domain_id_invalid_env() {
        unsafe { std::env::set_var("ROS_DOMAIN_ID", "not_a_number") };
        assert_eq!(default_domain_id(), 0);
        unsafe { std::env::remove_var("ROS_DOMAIN_ID") };
    }

    #[test]
    fn test_config_defaults() {
        let cfg = Config::parse_from(["ros-z-bridge-dds"]);
        assert_eq!(cfg.zenoh_endpoint, "tcp/127.0.0.1:7447");
        assert_eq!(cfg.node_name, "zenoh_bridge_dds");
        assert!(cfg.namespace.is_none());
        assert!(cfg.allow.is_none());
        assert!(cfg.deny.is_none());
    }
}
