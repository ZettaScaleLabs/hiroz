//! Frontends: the native runtime surfaces that drive the [`CoreEngine`] and
//! present the platform to a human or a machine.
//!
//! A frontend owns its own I/O (the terminal, an HTTP server, stdout) and the
//! `CoreEngine` (Zenoh sessions, graph, event bus), and it hosts the plugins
//! that target its WIT world. This is the *native host side* of an interaction
//! surface; a WASM plugin is the *sandboxed guest side* of the same surface:
//!
//! | Frontend (native) | Guest contract (WIT world) |
//! |-------------------|----------------------------|
//! | [`Cli`]           | `hu-cli-plugin`            |
//! | [`Tui`]           | `hu-tui-plugin`           |
//! | [`Web`]           | `hu-web-plugin`           |
//! | [`Stream`]        | *(none — raw core events)* |
//!
//! Frontends are a small, closed, trusted set — they need full host access
//! (terminal, runtime, sessions) that the plugin sandbox withholds — so they
//! dispatch statically through this trait rather than through the WASM host.
//! See `docs/tools/why-hu.md` for the platform model.

use std::sync::Arc;

use crate::core::engine::CoreEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A native runtime surface over the [`CoreEngine`].
pub trait Frontend {
    /// Whether this frontend needs a live router before it can do anything
    /// useful. Interactive frontends (the TUI) return `false` so they can start
    /// and show a disconnected banner; one-shot / streaming frontends return
    /// `true` so the launcher fails fast with an actionable hint.
    fn needs_router(&self) -> bool {
        true
    }

    /// Run the frontend to completion, returning the process exit code.
    async fn run(self, core: Arc<CoreEngine>) -> Result<u8, BoxError>;
}

/// Gate on a router if the frontend needs one, then run it.
pub async fn launch<F: Frontend>(
    frontend: F,
    core: Arc<CoreEngine>,
    router_addr: &str,
) -> Result<u8, BoxError> {
    if frontend.needs_router() {
        crate::require_router(&core, router_addr).await?;
    }
    frontend.run(core).await
}

/// Interactive terminal UI. Hosts `hu-tui-plugin` panes; starts even with no
/// router and shows a disconnected banner.
pub struct Tui;

impl Frontend for Tui {
    fn needs_router(&self) -> bool {
        false
    }

    async fn run(self, core: Arc<CoreEngine>) -> Result<u8, BoxError> {
        crate::run_tui_mode(core).await?;
        Ok(0)
    }
}

/// HTTP server. Hosts `hu-web-plugin` route handlers.
pub struct Web {
    pub port: u16,
}

impl Frontend for Web {
    async fn run(self, core: Arc<CoreEngine>) -> Result<u8, BoxError> {
        crate::modes::web::run_web_mode(core, self.port).await?;
        Ok(0)
    }
}

/// Headless newline-delimited JSON event stream. No plugins — a direct
/// presentation of the core's event bus.
pub struct Stream {
    pub json: bool,
    pub echo: Vec<String>,
}

impl Frontend for Stream {
    async fn run(self, core: Arc<CoreEngine>) -> Result<u8, BoxError> {
        crate::modes::headless::run_headless_mode(&core, self.json, self.echo).await?;
        Ok(0)
    }
}

/// One-shot graph snapshot written to a file.
pub struct Export {
    pub path: String,
}

impl Frontend for Export {
    async fn run(self, core: Arc<CoreEngine>) -> Result<u8, BoxError> {
        crate::export::export_and_exit(&core, &self.path).await?;
        Ok(0)
    }
}

/// CLI frontend: run a single `hu-cli-plugin` and exit with its code.
#[cfg(feature = "wasm-plugins")]
pub struct Cli {
    pub plugin: String,
    pub args: Vec<String>,
}

#[cfg(feature = "wasm-plugins")]
impl Frontend for Cli {
    async fn run(self, core: Arc<CoreEngine>) -> Result<u8, BoxError> {
        let code = crate::modes::cli::run_cli_plugin(core, &self.plugin, self.args).await?;
        Ok(code as u8)
    }
}
