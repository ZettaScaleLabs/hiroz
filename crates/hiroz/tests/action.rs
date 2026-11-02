// Action-related integration tests
// This module organizes all action-related tests into a single integration test

mod action {
    // These tests deliberately reuse ROS names. Keep each fixture's session
    // private so parallel tests cannot discover each other's servers through
    // the default localhost router (disabling multicast alone is insufficient).
    fn context_builder() -> hiroz::context::ZContextBuilder {
        hiroz::context::ZContextBuilder::default()
            .with_mode("peer")
            .disable_multicast_scouting()
            .with_json("connect/endpoints", Vec::<String>::new())
            .with_json("listen/endpoints", Vec::<String>::new())
            .with_json("scouting/gossip/enabled", false)
            .with_json("scouting/delay", 0)
    }

    mod cancellation;
    mod client;
    mod communication;
    mod expiration;
    mod goal_handle;
    mod goal_state_machine;
    mod graph;
    mod interaction;
    mod names;
    mod remapping;
    mod server;
    mod wait;
}
