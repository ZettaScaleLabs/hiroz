# hu Plugin Authoring Guide

`hu` supports third-party WASM plugins that run sandboxed as CLI commands (`hu <plugin-name> <args>`), as TUI panes (panel 5), or as web handlers (`hu --web`). Plugins are compiled to WebAssembly and loaded at startup from `$HU_PLUGIN_PATH` or `~/.local/share/hu/plugins/`.

## Worlds

There are three WIT worlds — pick the one that matches your plugin's role:

| World | Use case | Event type |
|---|---|---|
| `hu-cli-plugin` | One-shot or streaming terminal tool (hz, bw, echo, bridge) | `cli-event` — startup, tick, interrupt |
| `hu-tui-plugin` | Tick-driven TUI pane with keybindings and topic navigation | `tui-event` — startup, tick, interrupt, key-action, topic-selected |
| `hu-web-plugin` | HTTP request/response handler for `hu --web` | no events — stateless `handle(req) → response` |

The `hu-plugin` world still exists as a compatibility alias for `hu-tui-plugin`. New plugins should use one of the three typed worlds above.

## Quick start (CLI plugin)

A plugin is a Rust `cdylib` crate that implements a WIT world using `wit_bindgen`.

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
wit-bindgen = "0.46"
```

### 2. Copy the WIT schema

Copy `hu-plugin.wit` from `crates/hiroz-union/wit/v0.4/` into your crate's `wit/` directory. The file declares package `hu:plugin@0.4.0` — do not modify it.

### 3. Implement the world

```rust
wit_bindgen::generate!({
    world: "hu-cli-plugin",
    path: "wit/hu-plugin.wit",
});

use hu::plugin::types::{EventKind, Permission};
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
```

`CliEvent` has exactly three arms: `Startup`, `Tick`, and `Interrupt`. There are no dead arms for TUI-only events — the type system enforces the CLI/TUI boundary at compile time.

### 4. Build

Build for the WASI Preview 2 target:

```sh
cargo build --target wasm32-wasip2 --release
```

No `cargo-component` or other tooling required — plain `cargo build` with the `wasm32-wasip2` target is sufficient.

### 5. Install

Name the file `<subcommand>.wasm` — `hu` strips any `hu-` prefix when discovering plugins, so `hu-meter.wasm` registers as `meter` and is invoked by `hu meter <args>`.

```sh
mkdir -p ~/.local/share/hu/plugins
cp target/wasm32-wasip2/release/my_hu_plugin.wasm \
   ~/.local/share/hu/plugins/my-plugin.wasm
```

Start `hu` and press `5` to open the Plugins panel (TUI plugins), or run `hu my-plugin <args>` from the terminal (CLI plugins).

## WIT world boundary

```mermaid
flowchart LR
    subgraph Host["Host (hu binary)"]
        G["graph\nlist-topics / list-nodes / list-services"]
        R["ros\nsubscribe / measure-hz / measure-hz-typed\nconnect-service / encode-yaml-to-cdr"]
        RT["raw-transport\nZenoh sessions declared in manifest"]
        S["session\nnamed Zenoh sessions"]
        Ren["render\nprintln / set-title / emit-json / exit"]
    end
    subgraph Plugin["Plugin (.wasm)"]
        MF["manifest() → PluginManifest"]
        OE["on-event(CliEvent | TuiEvent)\nor handle(HttpRequest) → HttpResponse"]
    end
    Host -->|imports provided to plugin| Plugin
    Plugin -->|exports consumed by host| Host
```

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
| `measure-hz(topic, window-ms)` | Estimate publish rate (Hz) as a scalar `f64` |
| `measure-hz-typed(topic, window-ms)` | Returns `hz-measurement { topic, rate-hz, sample-count }` |
| `measure-bw(topic, window-ms)` | Estimate bandwidth (KB/s) as a scalar `f64` |
| `measure-bw-typed(topic, window-ms)` | Returns `bw-measurement { topic, rate-kbps, sample-count }` |
| `connect-service(name, type)` | Returns a `service-client` resource; call `call(request-json, timeout-ms)` |
| `encode-yaml-to-cdr(yaml, type-name)` | Encode a YAML string to CDR bytes for the given ROS type |

Prefer the `*-typed` variants for new plugins — they carry topic name and sample count alongside the measurement and avoid a JSON round-trip.

Messages delivered by `subscribe` are JSON strings. CDR decoding is handled by the host; plugins never see raw bytes unless they use `raw-transport` directly.

### `render` — output

| Function | Description |
|---|---|
| `println(text)` | Append a line to the plugin's output buffer |
| `set-title(title)` | Update the panel title (TUI mode) |
| `emit-json(key, value)` | Shorthand for `println({"key":value})` |
| `exit(code)` | Signal the host to flush output and exit with the given code (CLI mode only) |

The output buffer is a ring-buffer of 1000 lines. Old lines are discarded automatically.

### `session` — raw Zenoh sessions

For bridge plugins and other low-level use: declare sessions in your manifest and retrieve them via `session::get-session(name)`. The returned `session-handle` exposes raw subscribe, publish, liveliness, queryable, and querier primitives over Zenoh key expressions.

## Events

### CLI events (`hu-cli-plugin`)

```wit
variant cli-event {
    startup(list<string>),   // fired once on load with CLI args after the plugin name
    tick,                    // fired every tick-ms milliseconds
    interrupt,               // user pressed Ctrl-C
}
```

### TUI events (`hu-tui-plugin`)

```wit
variant tui-event {
    startup(list<string>),
    key-action(string),      // user pressed a key declared in the manifest
    topic-selected(string),  // user pressed Enter on a topic in another panel
    tick,
    interrupt,
}
```

## Event lifecycle

```mermaid
sequenceDiagram
    participant Host as hu host
    participant Plugin as plugin.wasm

    Host->>Plugin: manifest()
    Plugin-->>Host: PluginManifest (name, sessions, tick_ms, …)
    Host->>Host: open Zenoh sessions declared in manifest
    Host->>Plugin: on-event(Startup([args…]))
    Note over Plugin: parse subcommand / init state

    loop every tick_ms ms
        Host->>Plugin: on-event(Tick)
        Plugin-->>Host: render::println(…)
    end

    Host->>Plugin: on-event(Interrupt)
    Plugin->>Host: render::exit(130)
    Host->>Host: flush output, exit
```

`Startup` is always the first event. In CLI mode parse your subcommand and arguments there. Call `render::exit(code)` when done; the host flushes output and exits. Set `tick_ms` to `0` to disable ticks (useful for one-shot commands that finish in `Startup`).

## Plugin discovery

```mermaid
flowchart TD
    A["$HU_PLUGIN_PATH dirs\n(colon-separated)"] --> S
    B["~/.local/share/hu/plugins/"] --> S
    S["scan for *.wasm files"] --> C["call manifest() on each candidate"]
    C --> D{manifest() succeeded?}
    D -->|yes| E["strip hu- prefix from filename\nregister as subcommand"]
    D -->|no| F["log warning, skip\n(visible in hu plugin list)"]
```

`hu` searches both locations in order. Files named `hu-<name>.wasm` register as `<name>`. Run `hu plugin list` to see which plugins loaded successfully.

## Environment

Plugins can read environment variables from the `hu` process (`HU_ROUTER`, `HU_DOMAIN`, and any others set in the shell). Filesystem and network access are not available. Only install plugins you trust.

## Reference implementations

See `crates/hiroz-union/plugins/hu-meter/` and `crates/hiroz-union/plugins/hu-monitor/` for complete examples that implement full CLI subcommand dispatch via the `Startup` event and periodic output via `Tick`.
