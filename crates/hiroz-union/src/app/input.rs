//! Input handling and filter functionality

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, FocusPane, PAGE_SCROLL_AMOUNT, Panel, QUICK_MEASURE_DURATION_SECS};

/// Handle a single key event. Returns `true` if the application should quit.
pub async fn handle_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }

    if app.filter_mode {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.exit_filter_mode(),
            KeyCode::Backspace => app.delete_filter_char(),
            KeyCode::Left => app.move_filter_cursor_left(),
            KeyCode::Right => app.move_filter_cursor_right(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_filter()
            }
            KeyCode::Char(c) => app.enter_filter_char(c),
            _ => {}
        }
        return Ok(false);
    }

    if app.show_help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                app.show_help = false;
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('?') => app.show_help = true,

        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus_pane == FocusPane::List {
                app.select_previous();
            } else {
                app.detail_state.selected_section = match app.detail_state.selected_section {
                    super::DetailSection::Publishers => super::DetailSection::Clients,
                    super::DetailSection::Subscribers => super::DetailSection::Publishers,
                    super::DetailSection::Clients => super::DetailSection::Subscribers,
                };
                app.scroll_detail_up();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus_pane == FocusPane::List {
                app.select_next();
            } else {
                app.detail_state.selected_section = match app.detail_state.selected_section {
                    super::DetailSection::Publishers => super::DetailSection::Subscribers,
                    super::DetailSection::Subscribers => super::DetailSection::Clients,
                    super::DetailSection::Clients => super::DetailSection::Publishers,
                };
                app.scroll_detail_down();
            }
        }
        KeyCode::Left | KeyCode::Char('h') if app.focus_pane == FocusPane::Detail => {
            app.focus_pane = FocusPane::List;
        }
        KeyCode::Right | KeyCode::Char('l') if app.focus_pane == FocusPane::List => {
            app.focus_pane = FocusPane::Detail;
        }

        KeyCode::PageUp => {
            for _ in 0..PAGE_SCROLL_AMOUNT {
                if app.focus_pane == FocusPane::List {
                    app.select_previous();
                } else {
                    app.scroll_detail_up();
                }
            }
        }
        KeyCode::PageDown => {
            for _ in 0..PAGE_SCROLL_AMOUNT {
                if app.focus_pane == FocusPane::List {
                    app.select_next();
                } else {
                    app.scroll_detail_down();
                }
            }
        }
        KeyCode::Home => {
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::End => {
            let max = match app.current_panel {
                Panel::Topics => app.cached_topics.len().saturating_sub(1),
                Panel::Nodes => app.cached_nodes.len().saturating_sub(1),
                Panel::Services => app.cached_services.len().saturating_sub(1),
                Panel::Measure | Panel::Plugins => 0,
            };
            app.selected_index = max;
        }

        KeyCode::Tab => {
            app.current_panel = app.current_panel.next();
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::BackTab => {
            app.current_panel = app.current_panel.prev();
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::Char('1') => {
            app.current_panel = Panel::Topics;
            app.selected_index = 0;
        }
        KeyCode::Char('2') => {
            app.current_panel = Panel::Services;
            app.selected_index = 0;
        }
        KeyCode::Char('3') => {
            app.current_panel = Panel::Nodes;
            app.selected_index = 0;
        }
        KeyCode::Char('4') => {
            app.current_panel = Panel::Measure;
            app.selected_index = 0;
        }
        KeyCode::Char('5') => {
            app.current_panel = Panel::Plugins;
            app.plugin_mgr.selected_index = 0;
        }
        KeyCode::Char('t') if app.current_panel == Panel::Plugins => {
            let idx = app.plugin_mgr.selected_index;
            app.plugin_mgr.dispatch_tick(idx);
        }

        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.focus_pane == FocusPane::Detail {
                match app.current_panel {
                    Panel::Nodes => {
                        app.detail_state.publishers_expanded =
                            !app.detail_state.publishers_expanded;
                    }
                    _ => match app.detail_state.selected_section {
                        super::DetailSection::Publishers => {
                            app.detail_state.publishers_expanded =
                                !app.detail_state.publishers_expanded;
                        }
                        super::DetailSection::Subscribers => {
                            app.detail_state.subscribers_expanded =
                                !app.detail_state.subscribers_expanded;
                        }
                        super::DetailSection::Clients => {
                            app.detail_state.clients_expanded = !app.detail_state.clients_expanded;
                        }
                    },
                }
            } else {
                app.focus_pane = FocusPane::Detail;
            }
        }

        KeyCode::Esc if app.focus_pane == FocusPane::Detail => {
            app.focus_pane = FocusPane::List;
        }

        KeyCode::Char('/') => app.enter_filter_mode(),

        KeyCode::Char('r') => {
            if app.current_panel == Panel::Measure {
                app.clear_measuring_topics();
            } else if app.current_panel == Panel::Topics
                && !app.cached_topics.is_empty()
                && let Some((topic, _)) = app.cached_topics.get(app.selected_index)
            {
                let topic = topic.clone();
                app.status_message = format!("Measuring rate for {}...", topic);
                match app
                    .quick_measure_rate(&topic, QUICK_MEASURE_DURATION_SECS)
                    .await
                {
                    Ok(rate) => {
                        app.set_temp_status(format!("{}: {:.1} Hz", topic, rate));
                    }
                    Err(e) => {
                        app.set_temp_status(format!("Rate measurement failed: {}", e));
                    }
                }
            }
        }

        KeyCode::Char('m') => match app.current_panel {
            Panel::Topics => {
                if !app.cached_topics.is_empty()
                    && let Some((topic, _)) = app.cached_topics.get(app.selected_index)
                {
                    let topic = topic.clone();
                    app.toggle_measuring_topic(&topic).await;
                }
            }
            Panel::Services => {
                if !app.cached_services.is_empty()
                    && let Some((service, _)) = app.cached_services.get(app.selected_index)
                {
                    let service = service.clone();
                    app.toggle_measuring_topic(&service).await;
                }
            }
            Panel::Nodes => {
                app.set_temp_status("Use Topics tab to add topics to measurement".to_string());
            }
            Panel::Measure | Panel::Plugins => {}
        },

        KeyCode::Char('S') => {
            app.take_screenshot = true;
        }

        KeyCode::Char('e') => {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let filename = format!("hiroz-rates_{}.csv", timestamp);
            match app.export_metrics(&filename) {
                Ok(()) => {
                    app.set_temp_status(format!("Exported to {}", filename));
                }
                Err(e) => {
                    app.set_temp_status(format!("Export failed: {}", e));
                }
            }
        }

        KeyCode::Char('w') => match app.toggle_recording() {
            Ok(msg) => {
                app.set_temp_status(msg);
            }
            Err(e) => {
                app.set_temp_status(format!("{}", e));
            }
        },

        _ => {}
    }

    Ok(false)
}

use super::App;

impl App {
    pub fn matches_filter(&self, filter_text: &str, item: &str) -> bool {
        if filter_text.is_empty() {
            true
        } else {
            item.to_lowercase().contains(&filter_text.to_lowercase())
        }
    }

    pub fn filter_items<T, F>(&self, items: &[T], filter_text: &str, extract_fn: F) -> Vec<T>
    where
        T: Clone,
        F: Fn(&T) -> Vec<String>,
    {
        items
            .iter()
            .filter(|item| {
                let search_fields = extract_fn(item);
                search_fields
                    .iter()
                    .any(|field| self.matches_filter(filter_text, field))
            })
            .cloned()
            .collect()
    }

    pub fn enter_filter_char(&mut self, new_char: char) {
        let byte_index = self
            .filter_input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.filter_cursor)
            .unwrap_or(self.filter_input.len());
        self.filter_input.insert(byte_index, new_char);
        self.move_filter_cursor_right();
        self.selected_index = 0; // Reset selection when filter changes
    }

    pub fn delete_filter_char(&mut self) {
        if self.filter_cursor == 0 {
            return;
        }

        let current_index = self.filter_cursor;
        let from_left_to_current_index = current_index - 1;

        let before_char_to_delete = self.filter_input.chars().take(from_left_to_current_index);
        let after_char_to_delete = self.filter_input.chars().skip(current_index);

        self.filter_input = before_char_to_delete.chain(after_char_to_delete).collect();
        self.move_filter_cursor_left();
        self.selected_index = 0;
    }

    pub fn move_filter_cursor_left(&mut self) {
        let cursor_moved_left = self.filter_cursor.saturating_sub(1);
        self.filter_cursor = cursor_moved_left;
    }

    pub fn move_filter_cursor_right(&mut self) {
        let cursor_moved_right = self.filter_cursor.saturating_add(1);
        self.filter_cursor = self.clamp_filter_cursor(cursor_moved_right);
    }

    pub fn clamp_filter_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.min(self.filter_input.chars().count())
    }

    pub fn clear_filter(&mut self) {
        self.filter_input.clear();
        self.filter_cursor = 0;
        self.selected_index = 0;
    }

    pub fn exit_filter_mode(&mut self) {
        self.filter_mode = false;
        self.reset_status();
    }

    pub fn enter_filter_mode(&mut self) {
        self.filter_mode = true;
        self.status_message = "Type to filter | Esc:exit Ctrl+U:clear Enter:apply".to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::Panel;

    #[test]
    fn panel_tab_cycles() {
        let mut p = Panel::Topics;
        p = p.next();
        assert_eq!(p, Panel::Services);
        p = p.next();
        assert_eq!(p, Panel::Nodes);
        p = p.next();
        assert_eq!(p, Panel::Measure);
        p = p.next();
        assert_eq!(p, Panel::Plugins);
        p = p.next();
        assert_eq!(p, Panel::Topics);
    }

    #[test]
    fn panel_back_tab_cycles() {
        let mut p = Panel::Topics;
        p = p.prev();
        assert_eq!(p, Panel::Plugins);
        p = p.prev();
        assert_eq!(p, Panel::Measure);
    }
}
