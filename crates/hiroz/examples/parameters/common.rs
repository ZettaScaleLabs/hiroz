use hiroz::{
    Builder, Result,
    context::{ZContext, ZContextBuilder},
};

pub fn init() {
    zenoh::init_log_from_env_or("error");
}

/// Build a context connected to a Zenoh router.
///
/// Pass an explicit endpoint (from `--endpoint`) or fall back to `tcp/127.0.0.1:7447`.
pub fn create_context(endpoint: Option<String>) -> Result<ZContext> {
    let ep = endpoint.unwrap_or_else(|| "tcp/127.0.0.1:7447".to_string());
    ZContextBuilder::default()
        .with_connect_endpoints([ep])
        .build()
}
