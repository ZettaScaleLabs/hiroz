//! WIT bindings and host implementation submodules.

pub mod graph;
pub mod render;
pub mod ros;
pub mod transport;

use wasmtime::component::bindgen;

// Legacy / TUI world — used for backward compat and for instantiating hu-plugin
// (v0.3) components. All Host trait impls on PluginState live here.
bindgen!({
    world: "hu-plugin",
    path: "wit/v0.4/hu-plugin.wit",
});

/// CLI-world bindings.  `with` directives reuse the interface types and Host
/// trait impls generated above so PluginState doesn't need extra impls.
pub mod cli_bindgen {
    use wasmtime::component::bindgen;
    bindgen!({
        world: "hu-cli-plugin",
        path: "wit/v0.4/hu-plugin.wit",
        with: {
            "hu:plugin/types":         super::hu::plugin::types,
            "hu:plugin/graph":         super::hu::plugin::graph,
            "hu:plugin/ros":           super::hu::plugin::ros,
            "hu:plugin/render":        super::hu::plugin::render,
            "hu:plugin/raw-transport": super::hu::plugin::raw_transport,
            "hu:plugin/session":       super::hu::plugin::session,
        }
    });
}

/// TUI-world bindings (new hu-tui-plugin world — same interfaces as legacy
/// hu-plugin but exports on-event(tui-event) instead of on-event(plugin-event)).
pub mod tui_bindgen {
    use wasmtime::component::bindgen;
    bindgen!({
        world: "hu-tui-plugin",
        path: "wit/v0.4/hu-plugin.wit",
        with: {
            "hu:plugin/types":         super::hu::plugin::types,
            "hu:plugin/graph":         super::hu::plugin::graph,
            "hu:plugin/ros":           super::hu::plugin::ros,
            "hu:plugin/render":        super::hu::plugin::render,
            "hu:plugin/raw-transport": super::hu::plugin::raw_transport,
            "hu:plugin/session":       super::hu::plugin::session,
        }
    });
}

/// Web-world bindings.  Only imports graph and ros (no TUI render interface).
pub mod web_bindgen {
    use wasmtime::component::bindgen;
    bindgen!({
        world: "hu-web-plugin",
        path: "wit/v0.4/hu-plugin.wit",
        with: {
            "hu:plugin/types": super::hu::plugin::types,
            "hu:plugin/graph": super::hu::plugin::graph,
            "hu:plugin/ros":   super::hu::plugin::ros,
        }
    });
}
