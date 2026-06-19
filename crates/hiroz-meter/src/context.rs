use anyhow::Result;
use hiroz::{Builder, context::ZContextBuilder, graph::Graph, node::ZNode};
use std::sync::Arc;

pub struct Ctx {
    pub session: Arc<zenoh::Session>,
    pub node: Arc<ZNode>,
    pub graph: Arc<Graph>,
    pub domain: usize,
    #[allow(dead_code)]
    pub router: String,
}

macro_rules! ze {
    ($e:expr) => {
        $e.map_err(|e| anyhow::anyhow!("{e}"))
    };
}

pub async fn connect(router: &str, domain: usize) -> Result<Ctx> {
    let mut config = zenoh::Config::default();
    ze!(config.insert_json5("mode", "\"client\""))?;
    ze!(config.insert_json5("connect/endpoints", &format!("[\"{}\"]", router)))?;
    ze!(config.insert_json5("scouting/multicast/enabled", "false"))?;

    let session = ze!(zenoh::open(config.clone()).await)?;
    let session = Arc::new(session);

    let pattern = format!("@ros2_lv/{domain}/**");
    let format = hiroz_protocol::KeyExprFormat::RmwZenoh;
    let graph = ze!(Graph::new_with_pattern(
        &session,
        domain,
        pattern,
        move |ke| { format.parse_liveliness(ke) }
    ))?;
    let graph = Arc::new(graph);

    let zctx = ze!(ZContextBuilder::default()
        .with_domain_id(domain)
        .with_zenoh_config(config)
        .build())?;
    let node = ze!(zctx.create_node("hu_meter").build())?;
    let node = Arc::new(node);

    Ok(Ctx {
        session,
        node,
        graph,
        domain,
        router: router.to_string(),
    })
}
