use std::{sync::Arc, time::Duration};

use crate::{
    core::engine::CoreEngine,
    plugin::wasm::{self, CliEvent, load_plugin_named},
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
    // Capture the graph and this process's own session zids before `core` is
    // moved into the loader, so the wait below settles only on a *genuinely
    // external* participant. hu opens two sessions (graph + ROS node, the
    // CoreEngine KNOWN GAP), so both zids are excluded or hu's own echoed
    // liveliness tokens would satisfy the wait.
    //
    // NOTE: plugin-opened raw sessions (`open_declared_sessions`) are not listed
    // here — safe only because they declare no liveliness token and never appear
    // as graph entities. A plugin declaring a token would need its zid added.
    let graph = core.graph.clone();
    let own_zids = [core.session.zid(), core.node.session().zid()];
    let (mut plugins, _) = load_plugin_named(core, plugin_name)?;
    // Only CLI plugins are valid here.
    let plugin = plugins
        .iter_mut()
        .find(|p| p.is_cli() && p.manifest().name == plugin_name)
        .ok_or_else(|| format!("CLI WASM plugin '{plugin_name}' not found"))?;

    // The graph's liveliness subscriber (from CoreEngine::new) replays existing
    // tokens asynchronously, and that reply may not have landed when we resume.
    // One-shot commands (tick_ms == 0) read the graph exactly once at Startup
    // with no tick loop to catch up, so they need the replay first or they see
    // an empty graph.
    //
    // Rather than a fixed sleep, wait on a real condition: an external
    // participant appearing, then the graph going quiet. It does not treat an
    // empty-but-quiet graph as settled — on a CPU-starved runner the external
    // token can arrive after any fixed quiet window. The timeout cap is only hit
    // when there is genuinely nothing external (e.g. `service call` to a missing
    // service), so it is kept modest.
    //
    // Tick plugins (tick_ms > 0) re-read on every Tick, so an early first read
    // self-heals and the settle wait is pure dead time — and on a constrained
    // runner it can consume the whole test window before the first tick. Gate it
    // on one-shot plugins only.
    if plugin.manifest().tick_ms == 0 {
        graph
            .wait_for_external_settled(
                &own_zids,
                Duration::from_millis(300),
                Duration::from_secs(6),
            )
            .await;
    }

    let exit_code = plugin.dispatch_cli_event(CliEvent::Startup(args));
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
        let interrupt_code = plugin.dispatch_cli_event(CliEvent::Interrupt);
        flush_output(plugin);
        return Ok(interrupt_code.unwrap_or(130));
    }

    let tick_ms = raw_tick_ms.max(10) as u64;
    let tick_interval = Duration::from_millis(tick_ms);

    loop {
        if sigint_rx.try_recv().is_ok() {
            let interrupt_code = plugin.dispatch_cli_event(CliEvent::Interrupt);
            flush_output(plugin);
            if let Some(code) = interrupt_code {
                return Ok(code);
            }
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
