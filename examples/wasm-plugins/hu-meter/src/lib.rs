// Reference WASM plugin: hz/bw meter panel.
//
// Build:
//   cargo component build --target wasm32-wasip2 --release
//   # output: target/wasm32-wasip2/release/hu_meter_wasm.wasm
//
// Install:
//   mkdir -p ~/.local/share/hu/plugins
//   cp target/wasm32-wasip2/release/hu_meter_wasm.wasm ~/.local/share/hu/plugins/hu-meter.wasm

wit_bindgen::generate!({
    world: "hu-plugin",
    path: "../wit/hu-plugin.wit",
});

use exports::hu::plugin::guest::{Guest, PluginEvent, PluginManifest};
use hu::plugin::{graph, render};

struct HuMeter {
    // Topic currently being tracked (empty = show list).
    tracked_topic: String,
}

static mut STATE: HuMeter = HuMeter {
    tracked_topic: String::new(),
};

impl Guest for HuMeter {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "hu-meter".to_string(),
            version: "0.1.0".to_string(),
            description: "Hz/bw measurement panel".to_string(),
            bindings: vec![],
            tick_ms: 1000,
        }
    }

    fn on_event(event: PluginEvent) {
        let state = unsafe { &mut STATE };
        match event {
            PluginEvent::Tick => state.render(),
            PluginEvent::TopicSelected(topic) => {
                state.tracked_topic = topic;
                render::set_title(&state.tracked_topic);
            }
            PluginEvent::KeyAction(cmd) => {
                if cmd == "clear" {
                    state.tracked_topic.clear();
                    render::set_title("hu-meter");
                }
            }
        }
    }
}

impl HuMeter {
    fn render(&self) {
        let topics = graph::list_topics();
        if topics.is_empty() {
            render::println("  (no topics)");
            return;
        }
        render::println(&format!("{:<40} {:>8} {:>10}", "TOPIC", "HZ", "BW(KB/s)"));
        render::println(&"-".repeat(62));
        for t in &topics {
            let hz_result = hu::plugin::ros::measure_hz(&t.name, 1000);
            let bw_result = hu::plugin::ros::measure_bw(&t.name, 1000);
            let hz = hz_result.unwrap_or(0.0);
            let bw = bw_result.unwrap_or(0.0);
            if hz > 0.0 || bw > 0.0 {
                render::println(&format!("{:<40} {:>8.1} {:>10.1}", t.name, hz, bw));
            }
        }
    }
}

export!(HuMeter);
