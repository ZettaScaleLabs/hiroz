//! render::Host implementation — println, set_title, emit_json, exit.

use super::super::state::PluginState;
use super::hu;

/// Maximum number of lines retained in a plugin's output buffer. Matches the
/// ring-buffer capacity documented in `docs/tools/hu-plugins.md`; older lines
/// are discarded so a long-running tick-driven plugin can't grow it unbounded.
const MAX_OUTPUT_LINES: usize = 1000;

impl hu::plugin::render::Host for PluginState {
    fn println(&mut self, text: String) {
        let mut lines = self.output_lines.lock();
        lines.push(text);
        let overflow = lines.len().saturating_sub(MAX_OUTPUT_LINES);
        if overflow > 0 {
            lines.drain(0..overflow);
        }
    }

    fn eprintln(&mut self, text: String) {
        eprintln!("{text}");
    }

    fn set_title(&mut self, title: String) {
        *self.title.lock() = title;
    }

    fn emit_json(&mut self, key: String, value: String) {
        let parsed_value: serde_json::Value =
            serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value));
        let obj = serde_json::json!({ key: parsed_value });
        self.println(obj.to_string());
    }

    fn exit(&mut self, code: u32) {
        self.exit_code = Some(code);
    }
}
