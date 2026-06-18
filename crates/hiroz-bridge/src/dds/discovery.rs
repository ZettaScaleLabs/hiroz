use super::{gid::Gid, participant::BridgeQos};

/// Metadata about a discovered DDS endpoint (publication or subscription).
#[derive(Debug, Clone)]
pub struct DiscoveredEndpoint {
    pub key: Gid,
    pub participant_key: Gid,
    pub topic_name: String,
    pub type_name: String,
    pub keyless: bool,
    pub qos: BridgeQos,
}

/// Events emitted by the DDS discovery loop.
#[derive(Debug)]
pub enum DiscoveryEvent {
    DiscoveredPublication(DiscoveredEndpoint),
    UndiscoveredPublication(Gid),
    DiscoveredSubscription(DiscoveredEndpoint),
    UndiscoveredSubscription(Gid),
}
