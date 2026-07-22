//! render::Host implementation — println, set_title, emit_json, exit.

use super::super::state::PluginState;
use super::hu;

impl hu::plugin::render::Host for PluginState {
    fn println(&mut self, text: String) {
        self.output_lines.lock().push(text);
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
