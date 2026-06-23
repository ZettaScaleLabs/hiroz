//! render::Host implementation — println, set_title, emit_json, exit.

use super::super::state::PluginState;
use super::hu;

impl hu::plugin::render::Host for PluginState {
    fn println(&mut self, text: String) {
        let mut lines = self.output_lines.lock();
        lines.push(text);
        if lines.len() > 1000 {
            lines.drain(0..500);
        }
    }

    fn set_title(&mut self, title: String) {
        *self.title.lock() = title;
    }

    fn emit_json(&mut self, key: String, value: String) {
        self.println(format!("{{\"{key}\":{value}}}"));
    }

    fn exit(&mut self, code: u32) {
        self.exit_code = Some(code);
    }
}
