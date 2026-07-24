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
    // Capture the graph and this process's own session zid before `core` is
    // moved into the loader, so one-shot commands can wait below for a
    // *genuinely external* participant to appear. hu opens a single Zenoh
    // session shared by its graph and its ROS node, so excluding that one zid
    // is enough or hu's own echoed liveliness tokens would satisfy the wait.
    //
    // NOTE: WASM plugins can open their own raw `zenoh::Session`s
    // (`open_declared_sessions`), which are NOT in this list. That is only safe
    // because those sessions are raw pub/sub with no `ZNode`/liveliness-token
    // declaration, so they never appear as graph entities. If a plugin ever
    // declares a liveliness token on its own session, add its zid here or the
    // barrier could settle on hu's own plugin session.
    let graph = core.graph.clone();
    let own_zids = [core.session.zid()];
    let (mut plugins, _) = load_plugin_named(core, plugin_name)?;
    // Only CLI plugins are valid here.
    let plugin = plugins
        .iter_mut()
        .find(|p| p.is_cli() && p.manifest().name == plugin_name)
        .ok_or_else(|| format!("CLI WASM plugin '{plugin_name}' not found"))?;

    // The graph's liveliness subscriber (declared during CoreEngine::new)
    // replays existing tokens asynchronously via zenoh's own history query --
    // that reply hasn't necessarily landed yet the instant this function
    // resumes. One-shot commands (list/info, tick_ms == 0) read the graph
    // exactly once, at Startup, with no tick loop to catch up on a later
    // update, so they need the replay to land first or they systematically see
    // an empty/incomplete graph.
    //
    // Instead of a blind fixed sleep (which over-sleeps on a fast machine and
    // under-sleeps on a contended CI runner), wait on a real condition: an
    // external participant appearing in the graph, then the graph going quiet.
    // Crucially this does not treat an empty-but-quiet graph as "settled" — on a
    // CPU-starved runner the external liveliness token can arrive later than any
    // fixed quiet window, and returning early there is what made these commands
    // read an empty graph. The cap is only ever reached when there is genuinely
    // nothing external to discover (e.g. `service call` to a nonexistent
    // service), so it is kept modest to not stack onto such commands' own
    // timeouts.
    //
    // Tick plugins (tick_ms > 0), by contrast, re-read the graph on every Tick,
    // so an early first read self-heals — the settle wait is pure dead time for
    // them. Worse, on a constrained CI runner it stacks on the first-tick
    // interval and can consume the whole `hu my-plugin` test window before a
    // single tick fires. So gate it on one-shot plugins only.
    if plugin.manifest().tick_ms == 0 {
        graph
            .wait_for_external_settled(
                &own_zids,
                Duration::from_millis(300),
                Duration::from_secs(6),
            )
            .await;
    }

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
