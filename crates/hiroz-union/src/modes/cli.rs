use std::{sync::Arc, time::Duration};

use crate::{
    core::engine::CoreEngine,
    plugin::wasm::{self, CliEvent, load_plugins},
};

/// Run a loaded WASM plugin in CLI mode.
///
/// Only dispatches `CliEvent` — the type system prevents sending `key-action`
/// or `topic-selected` (TUI-only events) down this path.
pub async fn run_cli_plugin(
    core: Arc<CoreEngine>,
    plugin_name: &str,
    args: Vec<String>,
) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
    let (mut plugins, _) = load_plugins(core)?;
    // Only CLI plugins are valid here.
    let plugin = plugins
        .iter_mut()
        .find(|p| p.is_cli() && p.manifest().name == plugin_name)
        .ok_or_else(|| format!("CLI WASM plugin '{plugin_name}' not found"))?;

    let exit_code = plugin.dispatch_cli_event(CliEvent::Startup(args));
    flush_output(plugin);
    if let Some(code) = exit_code {
        return Ok(code);
    }

    let tick_ms = plugin.manifest().tick_ms.max(10) as u64;
    let tick_interval = Duration::from_millis(tick_ms);

    let (sigint_tx, mut sigint_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = sigint_tx.send(());
        }
    });

    loop {
        if sigint_rx.try_recv().is_ok() {
            plugin.dispatch_cli_event(CliEvent::Interrupt);
            flush_output(plugin);
            let code = plugin.dispatch_cli_event(CliEvent::Tick);
            flush_output(plugin);
            return Ok(code.unwrap_or(130));
        }

        tokio::time::sleep(tick_interval).await;

        let code = plugin.dispatch_cli_event(CliEvent::Tick);
        flush_output(plugin);
        if let Some(c) = code {
            return Ok(c);
        }
    }
}

fn flush_output(plugin: &mut wasm::WasmPlugin) {
    let mut lines = plugin.output_lines().lock();
    for line in lines.drain(..) {
        println!("{line}");
    }
}
