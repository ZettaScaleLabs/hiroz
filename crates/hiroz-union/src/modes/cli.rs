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

    // The graph's liveliness subscriber (declared during CoreEngine::new)
    // replays existing tokens asynchronously via zenoh's own history query --
    // that reply hasn't necessarily landed yet the instant this function
    // resumes. One-shot commands (list/info) read the graph exactly once, at
    // Startup, with no tick loop to catch up on a later update; give the
    // replay a window to land first so they don't systematically see an
    // empty/incomplete graph. 300ms wasn't enough under CI load (still
    // observed "node not found" for entities published well before hu even
    // started); widened to 1s.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let exit_code = plugin
        .dispatch_cli_event(CliEvent::Startup(args))
        .exit_code();
    flush_output(plugin);
    if let Some(code) = exit_code {
        return Ok(code);
    }

    let raw_tick_ms = plugin.manifest().tick_ms;

    let (sigint_tx, mut sigint_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = sigint_tx.send(());
        }
    });

    // `tick_ms == 0` means "no periodic ticks" (one-shot / Startup+Interrupt-only
    // plugins). In that case wait solely on the sigint signal and never dispatch
    // a Tick.
    if raw_tick_ms == 0 {
        let _ = sigint_rx.await;
        let interrupt_code = plugin.dispatch_cli_event(CliEvent::Interrupt).exit_code();
        flush_output(plugin);
        return Ok(interrupt_code.unwrap_or(130));
    }

    let tick_ms = raw_tick_ms.max(10) as u64;
    let tick_interval = Duration::from_millis(tick_ms);

    loop {
        if sigint_rx.try_recv().is_ok() {
            let interrupt_code = plugin.dispatch_cli_event(CliEvent::Interrupt).exit_code();
            flush_output(plugin);
            if let Some(code) = interrupt_code {
                return Ok(code);
            }
            let code = plugin.dispatch_cli_event(CliEvent::Tick).exit_code();
            flush_output(plugin);
            return Ok(code.unwrap_or(130));
        }

        tokio::time::sleep(tick_interval).await;

        let code = plugin.dispatch_cli_event(CliEvent::Tick).exit_code();
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
