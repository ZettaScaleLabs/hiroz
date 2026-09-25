use std::sync::Arc;

use hiroz::{Builder, context::ZContextBuilder};

#[test]
fn context_and_node_expose_the_same_session() {
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"peer\"").unwrap();
    config.insert_json5("listen/endpoints", "[]").unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let context = ZContextBuilder::default()
        .with_zenoh_config(config)
        .build()
        .unwrap();
    let node = context
        .create_node("shared_session")
        .without_parameters()
        .build()
        .unwrap();

    assert!(Arc::ptr_eq(context.session(), node.session()));
}
