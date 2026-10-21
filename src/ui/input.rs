//! A single-line text field used by every prompt in the UI.
//!
//! Kept deliberately small and independent of the document model: prompts edit
//! a `String`, not a `Document`, so they cannot accidentally mutate a buffer.

use crate::buffer::motion;

/// Editable single-line input with a cursor measured in characters.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    value: String,
    /// Cursor position in characters, `0..=len`.
    cursor: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// Replace the contents and move the cursor to the end.
    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Take the value, leaving the field empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.value)
    }

    fn byte_of(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }

    pub fn insert_char(&mut self, c: char) {
        let at = self.byte_of(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        // A single-line field never accepts line breaks.
        let cleaned: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        let at = self.byte_of(self.cursor);
        self.value.insert_str(at, &cleaned);
        self.cursor += cleaned.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_of(self.cursor - 1);
        let end = self.byte_of(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        let start = self.byte_of(self.cursor);
        let end = self.byte_of(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.len();
    }

    pub fn word_left(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
    }

    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Delete the word before the cursor (Ctrl+W inside a prompt).
    pub fn delete_word_back(&mut self) {
        let before = self.cursor;
        self.word_left();
        let start = self.byte_of(self.cursor);
        let end = self.byte_of(before);
        self.value.replace_range(start..end, "");
    }

    /// Character index under a given visual column, for mouse clicks.
    pub fn char_at_visual_col(&self, target: usize, tab_width: usize) -> usize {
        let (offset, _) = motion::char_at_visual_col(&self.value, target, tab_width);
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_moving() {
        let mut i = TextInput::new();
        for c in "abc".chars() {
            i.insert_char(c);
        }
        assert_eq!(i.value(), "abc");
        i.left();
        i.insert_char('X');
        assert_eq!(i.value(), "abXc");
        assert_eq!(i.cursor(), 3);
    }

    #[test]
    fn backspace_and_delete() {
        let mut i = TextInput::with("abc");
        i.backspace();
        assert_eq!(i.value(), "ab");
        i.home();
        i.delete();
        assert_eq!(i.value(), "b");
        i.end();
        i.delete();
        assert_eq!(i.value(), "b", "delete at end is a no-op");
    }

    #[test]
    fn unicode_is_character_indexed() {
        let mut i = TextInput::with("héllo");
        i.home();
        i.right();
        i.insert_char('🦀');
        assert_eq!(i.value(), "h🦀éllo");
        i.backspace();
        assert_eq!(i.value(), "héllo");
    }

    #[test]
    fn pasted_newlines_are_stripped() {
        let mut i = TextInput::new();
        i.insert_str("a\nb\r\nc");
        assert_eq!(i.value(), "abc");
    }

    #[test]
    fn word_motion_and_deletion() {
        let mut i = TextInput::with("foo bar baz");
        i.word_left();
        assert_eq!(i.cursor(), 8);
        i.delete_word_back();
        assert_eq!(i.value(), "foo baz");
    }

    #[test]
    fn take_clears() {
        let mut i = TextInput::with("x");
        assert_eq!(i.take(), "x");
        assert!(i.is_empty());
        assert_eq!(i.cursor(), 0);
    }
}
