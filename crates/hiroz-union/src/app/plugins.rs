//! WASM plugin manager — loads, selects, and dispatches to hu-* plugins.

#[cfg(feature = "wasm-plugins")]
use crate::plugin::wasm::{TuiEvent, WasmPlugin};

pub struct PluginManager {
    #[cfg(feature = "wasm-plugins")]
    plugins: Vec<WasmPlugin>,
    pub failed: Vec<(String, String)>,
    pub selected_index: usize,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "wasm-plugins")]
            plugins: Vec::new(),
            failed: Vec::new(),
            selected_index: 0,
        }
    }

    #[cfg(feature = "wasm-plugins")]
    pub fn plugins(&self) -> &[WasmPlugin] {
        &self.plugins
    }

    #[cfg(feature = "wasm-plugins")]
    pub fn set_plugins(&mut self, plugins: Vec<WasmPlugin>) {
        self.plugins = plugins;
    }

    pub fn count(&self) -> usize {
        #[cfg(feature = "wasm-plugins")]
        return self.plugins.len();
        #[cfg(not(feature = "wasm-plugins"))]
        return 0;
    }

    pub fn select_next(&mut self) {
        let max = self.count() + self.failed.len();
        if max > 0 && self.selected_index < max - 1 {
            self.selected_index += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    #[cfg(feature = "wasm-plugins")]
    pub fn dispatch_tick(&mut self, plugin_idx: usize) {
        if let Some(plugin) = self.plugins.get_mut(plugin_idx) {
            plugin.dispatch_tui_event(TuiEvent::Tick);
        }
    }

    #[cfg(not(feature = "wasm-plugins"))]
    pub fn dispatch_tick(&mut self, _plugin_idx: usize) {}

    /// Whether the plugin at `selected_index` accepts TUI events (so key
    /// presses should be forwarded to it rather than driving the TUI).
    pub fn selected_is_tui(&self) -> bool {
        #[cfg(feature = "wasm-plugins")]
        return self
            .plugins
            .get(self.selected_index)
            .is_some_and(|p| p.is_tui());
        #[cfg(not(feature = "wasm-plugins"))]
        return false;
    }

    /// Forward a key press to a focused TUI plugin as a `key-action` event.
    #[cfg(feature = "wasm-plugins")]
    pub fn dispatch_key_action(&mut self, plugin_idx: usize, key: String) {
        if let Some(plugin) = self.plugins.get_mut(plugin_idx)
            && plugin.is_tui()
        {
            plugin.dispatch_tui_event(TuiEvent::KeyAction(key));
        }
    }

    #[cfg(not(feature = "wasm-plugins"))]
    pub fn dispatch_key_action(&mut self, _plugin_idx: usize, _key: String) {}

    /// Broadcast the currently-selected topic to every loaded TUI plugin as a
    /// `topic-selected` event, so panes can follow the user's topic navigation.
    #[cfg(feature = "wasm-plugins")]
    pub fn dispatch_topic_selected(&mut self, topic: String) {
        for plugin in self.plugins.iter_mut().filter(|p| p.is_tui()) {
            plugin.dispatch_tui_event(TuiEvent::TopicSelected(topic.clone()));
        }
    }

    #[cfg(not(feature = "wasm-plugins"))]
    pub fn dispatch_topic_selected(&mut self, _topic: String) {}

    /// Number of loaded TUI-world plugins (cheap gate before broadcasting).
    pub fn tui_count(&self) -> usize {
        #[cfg(feature = "wasm-plugins")]
        return self.plugins.iter().filter(|p| p.is_tui()).count();
        #[cfg(not(feature = "wasm-plugins"))]
        return 0;
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_manager_empty_without_wasm() {
        let mgr = PluginManager::new();
        assert_eq!(mgr.count(), 0);
    }
}
