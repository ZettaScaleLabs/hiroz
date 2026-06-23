use std::{sync::Arc, time::Duration};

use crate::{
    core::engine::CoreEngine,
    plugin::wasm::{self, hu::plugin::types::PluginEvent, load_plugins},
};

/// Run a loaded WASM plugin in CLI mode.
///
/// Fires `startup(args)` once, then ticks the plugin every `tick_ms` (minimum
/// 10 ms) until the plugin calls `render::exit(code)` or SIGINT arrives.
/// Flushes `output_lines` to stdout after each tick. Returns the exit code.
pub async fn run_cli_plugin(
    core: Arc<CoreEngine>,
    plugin_name: &str,
    args: Vec<String>,
) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
    let (mut plugins, _) = load_plugins(core)?;
    let plugin = plugins
        .iter_mut()
        .find(|p| p.manifest.name == plugin_name)
        .ok_or_else(|| format!("WASM plugin '{plugin_name}' not found"))?;

    // Fire startup event with CLI args.
    let exit_code = plugin.dispatch_event(PluginEvent::Startup(args));
    flush_output(plugin);
    if let Some(code) = exit_code {
        return Ok(code);
    }

    let tick_ms = plugin.manifest.tick_ms.max(10) as u64;
    let tick_interval = Duration::from_millis(tick_ms);

    // Tokio signal handler for SIGINT (Ctrl-C).
    let (sigint_tx, mut sigint_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = sigint_tx.send(());
        }
    });

    loop {
        // Check for SIGINT; fire interrupt action so the plugin can clean up.
        if sigint_rx.try_recv().is_ok() {
            plugin.dispatch_event(PluginEvent::Interrupt);
            flush_output(plugin);
            // Give the plugin one more tick to call exit().
            let code = plugin.dispatch_event(PluginEvent::Tick);
            flush_output(plugin);
            return Ok(code.unwrap_or(130));
        }

        tokio::time::sleep(tick_interval).await;

        let code = plugin.dispatch_event(PluginEvent::Tick);
        flush_output(plugin);
        if let Some(c) = code {
            return Ok(c);
        }
    }
}

fn flush_output(plugin: &mut wasm::WasmPlugin) {
    let mut lines = plugin.output_lines.lock();
    for line in lines.drain(..) {
        println!("{line}");
    }
}
