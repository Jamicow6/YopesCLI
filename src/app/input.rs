//! The composer's text buffer: insertion, cursor motion, selection, and word edits,
//! all UTF-8-safe.

#[derive(Default)]
pub struct Input {
    pub(crate) value: String,
    pub(crate) cursor: usize,
    pub(crate) selection_anchor: Option<usize>,
}

impl Input {
    pub fn before_cursor(&self) -> &str {
        &self.value[..self.cursor]
    }

    pub fn with_cursor(&self) -> String {
        let mut value = self.value.clone();
        value.insert(self.cursor, '▌');
        value
    }

    pub(crate) fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            None
        } else if anchor < self.cursor {
            Some(anchor..self.cursor)
        } else {
            Some(self.cursor..anchor)
        }
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.value[range])
    }

    pub(crate) fn select_all(&mut self) {
        if !self.value.is_empty() {
            self.selection_anchor = Some(0);
            self.cursor = self.value.len();
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub(crate) fn delete_selection(&mut self) -> bool {
        if let Some(range) = self.selection() {
            self.value.replace_range(range.clone(), "");
            self.cursor = range.start;
            self.selection_anchor = None;
            true
        } else {
            self.selection_anchor = None;
            false
        }
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.selection_anchor = None;
    }

    pub(crate) fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
        self.selection_anchor = None;
    }

    pub(crate) fn take(&mut self) -> String {
        self.cursor = 0;
        self.selection_anchor = None;
        std::mem::take(&mut self.value)
    }

    pub(crate) fn insert(&mut self, character: char) {
        self.delete_selection();
        self.value.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub(crate) fn insert_str(&mut self, value: &str) {
        self.delete_selection();
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let clean = value
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect::<String>();
        self.value.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    pub(crate) fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some((index, _)) = self.value[..self.cursor].char_indices().next_back() {
            self.value.remove(index);
            self.cursor = index;
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    pub(crate) fn move_cursor(&mut self, target: usize, select: bool) {
        let target = target.min(self.value.len());
        if select {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else {
            self.selection_anchor = None;
        }
        self.cursor = target;
    }

    pub(crate) fn left(&mut self, select: bool) {
        let target = self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.move_cursor(target, select);
    }

    pub(crate) fn right(&mut self, select: bool) {
        let target = self.value[self.cursor..]
            .chars()
            .next()
            .map_or(self.value.len(), |character| {
                self.cursor + character.len_utf8()
            });
        self.move_cursor(target, select);
    }

    pub(crate) fn home(&mut self, select: bool) {
        self.move_cursor(0, select);
    }

    pub(crate) fn end(&mut self, select: bool) {
        self.move_cursor(self.value.len(), select);
    }

    /// Move vertically while preserving the cursor's character column.
    pub(crate) fn move_vertical(&mut self, down: bool, select: bool) {
        let before = &self.value[..self.cursor];
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let column = before[line_start..].chars().count();
        let line_end = self.value[line_start..]
            .find('\n')
            .map_or(self.value.len(), |n| line_start + n);
        let target_start = if down {
            if line_end == self.value.len() {
                return;
            }
            line_end + 1
        } else {
            if line_start == 0 {
                return;
            }
            self.value[..line_start - 1]
                .rfind('\n')
                .map_or(0, |n| n + 1)
        };
        let target_end = self.value[target_start..]
            .find('\n')
            .map_or(self.value.len(), |n| target_start + n);
        let target = self.value[target_start..target_end]
            .char_indices()
            .nth(column)
            .map_or(target_end, |(offset, _)| target_start + offset);
        self.move_cursor(target, select);
    }

    pub(crate) fn delete_word(&mut self) {
        if self.delete_selection() {
            return;
        }
        let before = &self.value[..self.cursor];
        let without_space = before.trim_end();
        let start = without_space
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, character)| index + character.len_utf8())
            .unwrap_or(0);
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(crate) fn pos_at_coords(&self, rel_x: usize, rel_y: usize, width: usize) -> usize {
        let width = width.max(1);
        let mut current_row = 0;
        let mut byte_offset = 0;

        for line in self.value.split('\n') {
            let mut col = 0;
            let mut line_bytes = 0;
            for ch in line.chars() {
                if current_row == rel_y && col >= rel_x {
                    return byte_offset + line_bytes;
                }
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                if col + w > width {
                    current_row += 1;
                    col = 0;
                    if current_row == rel_y && col >= rel_x {
                        return byte_offset + line_bytes;
                    }
                }
                col += w;
                line_bytes += ch.len_utf8();
            }
            if current_row == rel_y {
                return byte_offset + line_bytes;
            }
            current_row += 1;
            byte_offset += line_bytes + 1;
        }
        self.value.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Input;

    #[test]
    fn editing_respects_utf8_boundaries() {
        let mut input = Input::default();
        input.insert('é');
        input.insert('🦀');
        input.left(false);
        input.backspace();
        assert_eq!(input.value, "🦀");
        input.right(false);
        input.insert_str("\r\nhello");
        assert_eq!(input.value, "🦀\nhello");
    }

    #[test]
    fn deletes_the_previous_word_and_space() {
        let mut input = Input::default();
        input.insert_str("hello world  ");
        input.delete_word();
        assert_eq!(input.value, "hello ");
        input.delete_word();
        assert_eq!(input.value, "");
    }

    #[test]
    fn selection_and_replacement() {
        let mut input = Input::default();
        input.insert_str("abcdef");
        input.select_all();
        assert_eq!(input.selected_text(), Some("abcdef"));
        input.insert('x');
        assert_eq!(input.value, "x");
        assert_eq!(input.cursor, 1);
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn shift_navigation_selects_range() {
        let mut input = Input::default();
        input.insert_str("hello");
        input.left(true);
        input.left(true);
        assert_eq!(input.selected_text(), Some("lo"));
        input.backspace();
        assert_eq!(input.value, "hel");
        assert_eq!(input.cursor, 3);
    }
}
