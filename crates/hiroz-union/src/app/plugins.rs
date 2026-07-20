//! WASM plugin manager — loads, selects, and dispatches to hu-* plugins.

#[cfg(feature = "wasm-plugins")]
use crate::plugin::wasm::{TuiEvent, WasmPlugin};

pub struct PluginManager {
    #[cfg(feature = "wasm-plugins")]
    pub plugins: Vec<WasmPlugin>,
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

    pub fn count(&self) -> usize {
        #[cfg(feature = "wasm-plugins")]
        return self.plugins.len();
        #[cfg(not(feature = "wasm-plugins"))]
        return 0;
    }

    pub fn select_next(&mut self) {
        let max = self.count();
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
