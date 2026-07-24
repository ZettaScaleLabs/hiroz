//! Minimal CLI plugin — mirrors the "Quick start" code in docs/tools/hu-plugins.md.
//!
//! This crate is built in CI by the WASM plugin job (`wasm32-wasip2` target)
//! to ensure the authoring guide stays valid as the WIT interface evolves.
//! It is not shipped as part of the hiroz release.

wit_bindgen::generate!({
    world: "hu-cli-plugin",
    path: "wit/hu-plugin.wit",
});

use hu::plugin::types::EventKind;
use hu::plugin::render;

struct MyPlugin;

impl Guest for MyPlugin {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "my-plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "My first hu plugin".to_string(),
            bindings: vec![],
            tick_ms: 1000,
            sessions: vec![],
            subscribed_events: vec![EventKind::Startup, EventKind::Tick],
            required_permissions: vec![],
        }
    }

    fn on_event(event: CliEvent) {
        match event {
            CliEvent::Startup(args) => {
                // args is the CLI argument list after the plugin name.
                // e.g. `hu my-plugin foo bar` → args = ["foo", "bar"]
                let _ = args;
            }
            CliEvent::Tick => {
                render::println("hello from WASM!");
            }
            CliEvent::Interrupt => {
                render::exit(130);
            }
        }
    }
}

export!(MyPlugin);
