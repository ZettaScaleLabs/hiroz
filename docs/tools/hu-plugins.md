# hu Plugin Authoring Guide

`hu` supports third-party WASM plugins that run sandboxed inside the TUI's Plugins panel (panel 5). Plugins are compiled to WebAssembly and loaded at startup from `HU_PLUGIN_PATH` or `~/.local/share/hu/plugins/`.

## Quick start

A plugin is a Rust `cdylib` crate that implements the `hu-plugin` WIT world.

### 1. Create the crate

```sh
cargo new --lib my-hu-plugin
cd my-hu-plugin
```

Set the crate type in `Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.32"
```

### 2. Copy the WIT schema

Copy `hu-plugin.wit` from `crates/hiroz-union/wit/` into your crate's `wit/` directory. This file defines the stable ABI — do not modify it.

### 3. Implement the world

```rust
wit_bindgen::generate!({
    world: "hu-plugin",
    path: "wit/hu-plugin.wit",
});

use exports::hu::plugin::guest::{Guest, PluginEvent, PluginManifest};
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
        }
    }

    fn on_event(event: PluginEvent) {
        match event {
            PluginEvent::Tick => {
                render::println("hello from WASM!");
            }
            _ => {}
        }
    }
}

export!(MyPlugin);
```

### 4. Build

Install `cargo-component`, then build for the WASI Preview 2 target:

```sh
cargo install cargo-component
cargo component build --target wasm32-wasip2 --release
```

### 5. Install

```sh
mkdir -p ~/.local/share/hu/plugins
cp target/wasm32-wasip2/release/my_hu_plugin.wasm \
   ~/.local/share/hu/plugins/my-plugin.wasm
```

Start `hu` and press `5` to open the Plugins panel.

## WIT interfaces

### `graph` — ROS graph queries

| Function | Returns |
|---|---|
| `list-topics()` | `list<topic-info>` — name, type-name, publisher/subscriber counts |
| `list-nodes()` | `list<node-info>` — namespace and name |
| `list-services()` | `list<service-info>` — name, type-name, server count |

### `ros` — subscriptions and measurement

| Function | Description |
|---|---|
| `subscribe(topic)` | Returns a `subscription` resource; call `try-recv()` for the next JSON message |
| `measure-hz(topic, window-ms)` | Estimate publish rate (Hz) over the given window |
| `measure-bw(topic, window-ms)` | Estimate bandwidth (KB/s) over the given window |
| `connect-service(name, type)` | Returns a `service-client` resource; call `call(request-json, timeout-ms)` |

Messages are delivered as JSON strings. CDR decoding is handled by the host; plugins never see raw bytes.

### `render` — output

| Function | Description |
|---|---|
| `println(text)` | Append a line to the plugin's output buffer (shown in the right pane) |
| `set-title(title)` | Update the panel title |
| `emit-json(key, value)` | Shorthand for `println({"key":value})` |

The output buffer is a ring-buffer of 1000 lines. Old lines are discarded automatically.

## Events

```wit
variant plugin-event {
    key-action(string),    // user pressed a key bound in the manifest
    topic-selected(string), // user pressed Enter on a topic in another panel
    tick,                  // fired every tick-ms milliseconds
}
```

Set `tick-ms` in your manifest to control how often `Tick` fires. Use `0` to disable ticks entirely.

## Plugin discovery

`hu` searches for `.wasm` files in:

1. Directories listed in `HU_PLUGIN_PATH` (colon-separated).
2. `~/.local/share/hu/plugins/`.

All valid WASM components whose `manifest()` export succeeds are loaded. Failures are logged as warnings and skipped.

## Reference implementation

See `examples/wasm-plugins/hu-meter/` for a complete example that renders a hz/bw table from `graph::list-topics` on each Tick event.
