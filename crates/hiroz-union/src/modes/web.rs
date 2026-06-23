//! HTTP server mode — routes requests to `hu-web-plugin` WASM components.
//!
//! Each web plugin is mounted at `/plugins/<name>/` and receives the full
//! request path stripped of the prefix. The plugin's `handle()` export returns
//! an HTTP response that is forwarded to the caller.
//!
//! Enable with `--features web-plugins` and run with `hu --web [port]`.

#[cfg(feature = "web-plugins")]
pub use inner::run_web_mode;

#[cfg(not(feature = "web-plugins"))]
pub async fn run_web_mode(
    _core: std::sync::Arc<crate::core::engine::CoreEngine>,
    _port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("web plugin support not compiled in (rebuild with --features web-plugins)".into())
}

#[cfg(feature = "web-plugins")]
mod inner {
    use std::sync::Arc;

    use axum::{
        Router,
        body::Bytes,
        extract::{Path, Query, Request, State},
        http::{HeaderValue, StatusCode},
        response::{IntoResponse, Response},
        routing::any,
    };
    use parking_lot::Mutex;
    use tokio::net::TcpListener;

    use crate::{
        core::engine::CoreEngine,
        plugin::wasm::{HttpRequest, WasmPlugin, load_plugins},
    };

    struct WebState {
        plugins: Mutex<Vec<WasmPlugin>>,
    }

    pub async fn run_web_mode(
        core: Arc<CoreEngine>,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (plugins, failed) = load_plugins(core)?;
        if !failed.is_empty() {
            for (path, err) in &failed {
                tracing::warn!("Failed to load plugin {path}: {err}");
            }
        }

        let web_count = plugins.iter().filter(|p| p.is_web()).count();
        tracing::info!("Loaded {} web plugin(s)", web_count);

        let state = Arc::new(WebState {
            plugins: Mutex::new(plugins),
        });

        let app = Router::new()
            .route("/plugins/{name}/*path", any(handle_plugin_request))
            .route("/plugins/{name}", any(handle_plugin_request_root))
            .with_state(state);

        let listener = TcpListener::bind(("0.0.0.0", port)).await?;
        tracing::info!("hu web server listening on http://0.0.0.0:{port}");
        axum::serve(listener, app).await?;
        Ok(())
    }

    async fn handle_plugin_request(
        State(state): State<Arc<WebState>>,
        Path((name, path)): Path<(String, String)>,
        req: Request,
    ) -> Response {
        dispatch_to_plugin(state, &name, &format!("/{path}"), req).await
    }

    async fn handle_plugin_request_root(
        State(state): State<Arc<WebState>>,
        Path(name): Path<String>,
        req: Request,
    ) -> Response {
        dispatch_to_plugin(state, &name, "/", req).await
    }

    async fn dispatch_to_plugin(
        state: Arc<WebState>,
        plugin_name: &str,
        path: &str,
        req: Request,
    ) -> Response {
        let method = req.method().as_str().to_string();
        let query = req.uri().query().unwrap_or("").to_string();
        let body: Bytes = match axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024).await {
            Ok(b) => b,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}"))
                    .into_response();
            }
        };

        let wit_req = HttpRequest {
            method,
            path: path.to_string(),
            query,
            body: body.to_vec(),
        };

        let mut plugins = state.plugins.lock();
        let plugin = plugins
            .iter_mut()
            .find(|p| p.is_web() && p.manifest().name == plugin_name);

        match plugin {
            None => (
                StatusCode::NOT_FOUND,
                format!("web plugin '{plugin_name}' not found"),
            )
                .into_response(),
            Some(p) => match p.dispatch_web_request(wit_req) {
                None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                Some(resp) => {
                    let mut builder = axum::response::Response::builder().status(resp.status);
                    if let Ok(ct) = HeaderValue::from_str(&resp.content_type) {
                        builder = builder.header("content-type", ct);
                    }
                    builder
                        .body(axum::body::Body::from(resp.body))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
            },
        }
    }
}
