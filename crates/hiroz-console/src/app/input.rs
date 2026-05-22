//! Input handling and filter functionality

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
