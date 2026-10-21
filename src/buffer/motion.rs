//! Cursor motion primitives.
//!
//! All motions operate on character indices and are grapheme aware: moving left
//! over `é` (e + combining acute) or `👨‍👩‍👧` moves over the whole cluster, and
//! backspace deletes the whole cluster. Grapheme scans are bounded to a small
//! window so a keystroke never walks an entire multi-megabyte line.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use super::storage::TextStorage;

/// How far we look back/forward when resolving grapheme boundaries. Any cluster
/// longer than this is pathological; we degrade to a single character step
/// instead of scanning the whole line.
const GRAPHEME_WINDOW: usize = 64;

/// How far a word motion scans before giving up on finding a boundary.
const WORD_WINDOW: usize = 8192;

/// A cursor movement request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    /// First press goes to the first non-blank character, second to column 0.
    LineStartSmart,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// Parameters that affect motion results.
#[derive(Debug, Clone, Copy)]
pub struct MotionCtx {
    pub tab_width: usize,
    pub page: usize,
}

impl Default for MotionCtx {
    fn default() -> Self {
        Self {
            tab_width: 4,
            page: 20,
        }
    }
}

/// Character classes used for word motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Start of the grapheme cluster that ends at `idx`.
pub fn prev_grapheme(storage: &dyn TextStorage, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let start = idx.saturating_sub(GRAPHEME_WINDOW);
    let window = storage.slice_chars(start, idx);
    let mut char_pos = start;
    let mut last = start;
    for g in window.graphemes(true) {
        last = char_pos;
        char_pos += g.chars().count();
    }
    last
}

/// Start of the grapheme cluster that starts at `idx`.
pub fn next_grapheme(storage: &dyn TextStorage, idx: usize) -> usize {
    let len = storage.len_chars();
    if idx >= len {
        return len;
    }
    let end = (idx + GRAPHEME_WINDOW).min(len);
    let window = storage.slice_chars(idx, end);
    match window.graphemes(true).next() {
        Some(g) => idx + g.chars().count(),
        None => len,
    }
}

/// Display width of `text` starting at visual column `start_col`.
pub fn display_width(text: &str, start_col: usize, tab_width: usize) -> usize {
    let mut col = start_col;
    for c in text.chars() {
        col += char_width(c, col, tab_width);
    }
    col
}

/// Display width of a single character placed at visual column `col`.
pub fn char_width(c: char, col: usize, tab_width: usize) -> usize {
    match c {
        '\t' => {
            let tw = tab_width.max(1);
            tw - (col % tw)
        }
        '\n' | '\r' => 0,
        c if c.is_control() => 2, // rendered as a two-cell caret notation
        c => UnicodeWidthChar::width(c).unwrap_or(1),
    }
}

/// Visual column of `char_idx` inside the line starting at `line_start`.
pub fn visual_col_of(
    storage: &dyn TextStorage,
    line_start: usize,
    char_idx: usize,
    tab_width: usize,
) -> usize {
    if char_idx <= line_start {
        return 0;
    }
    let text = storage.slice_chars(line_start, char_idx);
    display_width(&text, 0, tab_width)
}

/// Character offset inside `line_text` closest to visual column `target`.
///
/// Returns `(char_offset, visual_col)`. Landing past the end of the line clamps
/// to the line length, which is the behaviour users expect when moving down
/// through lines of different lengths.
pub fn char_at_visual_col(line_text: &str, target: usize, tab_width: usize) -> (usize, usize) {
    let mut col = 0usize;
    let mut offset = 0usize;
    for c in line_text.chars() {
        let w = char_width(c, col, tab_width);
        if col + w > target {
            return (offset, col);
        }
        col += w;
        offset += 1;
    }
    (offset, col)
}

/// Move left by one grapheme.
pub fn left(storage: &dyn TextStorage, idx: usize) -> usize {
    prev_grapheme(storage, idx)
}

/// Move right by one grapheme.
pub fn right(storage: &dyn TextStorage, idx: usize) -> usize {
    next_grapheme(storage, idx)
}

/// Start of the previous word (or of the current word when inside one).
pub fn word_left(storage: &dyn TextStorage, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let start = idx.saturating_sub(WORD_WINDOW);
    let window: Vec<char> = storage.slice_chars(start, idx).chars().collect();
    let mut i = window.len();
    // Skip whitespace immediately before the cursor.
    while i > 0 && class_of(window[i - 1]) == Class::Space {
        i -= 1;
    }
    if i == 0 {
        return start;
    }
    let class = class_of(window[i - 1]);
    while i > 0 && class_of(window[i - 1]) == class {
        i -= 1;
    }
    start + i
}

/// Start of the next word.
pub fn word_right(storage: &dyn TextStorage, idx: usize) -> usize {
    let len = storage.len_chars();
    if idx >= len {
        return len;
    }
    let end = (idx + WORD_WINDOW).min(len);
    let window: Vec<char> = storage.slice_chars(idx, end).chars().collect();
    let mut i = 0usize;
    if i < window.len() {
        let class = class_of(window[i]);
        while i < window.len() && class_of(window[i]) == class {
            i += 1;
        }
    }
    while i < window.len() && class_of(window[i]) == Class::Space {
        i += 1;
    }
    idx + i
}

/// End of the current/next word (used by `Ctrl+Right` style motions on some
/// terminals); exposed for future key bindings.
pub fn word_end(storage: &dyn TextStorage, idx: usize) -> usize {
    let len = storage.len_chars();
    let mut i = idx;
    if i < len && class_of(storage.char_at(i).unwrap_or(' ')) == Class::Space {
        i = word_right(storage, i);
    } else {
        i = word_right(storage, i);
        // step back onto the last character of the word
        i = prev_grapheme(storage, i);
    }
    i.min(len)
}

/// Apply a motion, returning the new cursor position.
///
/// `preferred_col` is the visual column remembered across vertical motions; it
/// is returned so callers can keep it stable while moving through short lines.
pub fn apply(
    storage: &dyn TextStorage,
    cursor: usize,
    motion: Motion,
    ctx: MotionCtx,
    preferred_col: Option<usize>,
) -> (usize, Option<usize>) {
    let len = storage.len_chars();
    let cursor = cursor.min(len);

    match motion {
        Motion::Left => (left(storage, cursor), None),
        Motion::Right => (right(storage, cursor), None),
        Motion::WordLeft => (word_left(storage, cursor), None),
        Motion::WordRight => (word_right(storage, cursor), None),
        Motion::LineStart => {
            let line = storage.line_of_char(cursor);
            (storage.line_start(line), None)
        }
        Motion::LineStartSmart => {
            let line = storage.line_of_char(cursor);
            let start = storage.line_start(line);
            let text = storage.line_slice(line);
            let first_non_blank = text.chars().position(|c| !c.is_whitespace()).unwrap_or(0);
            if cursor == start + first_non_blank {
                (start, None)
            } else {
                (start + first_non_blank, None)
            }
        }
        Motion::LineEnd => {
            let line = storage.line_of_char(cursor);
            let start = storage.line_start(line);
            (start + storage.line_slice(line).chars().count(), None)
        }
        Motion::DocStart => (0, None),
        Motion::DocEnd => (len, None),
        Motion::Up | Motion::Down => {
            let line = storage.line_of_char(cursor);
            let line_start = storage.line_start(line);
            let col = preferred_col
                .unwrap_or_else(|| visual_col_of(storage, line_start, cursor, ctx.tab_width));
            let target_line = match motion {
                Motion::Up => {
                    if line == 0 {
                        return (cursor, Some(col));
                    }
                    line - 1
                }
                _ => {
                    if line + 1 >= storage.len_lines() {
                        return (cursor, Some(col));
                    }
                    line + 1
                }
            };
            let target_start = storage.line_start(target_line);
            let text = storage.line_slice(target_line);
            let (offset, _) = char_at_visual_col(&text, col, ctx.tab_width);
            (target_start + offset, Some(col))
        }
        Motion::PageUp | Motion::PageDown => {
            let step = ctx.page.max(1);
            let line = storage.line_of_char(cursor);
            let line_start = storage.line_start(line);
            let col = preferred_col
                .unwrap_or_else(|| visual_col_of(storage, line_start, cursor, ctx.tab_width));
            let last = storage.len_lines().saturating_sub(1);
            let target_line = if motion == Motion::PageUp {
                line.saturating_sub(step)
            } else {
                (line + step).min(last)
            };
            let target_start = storage.line_start(target_line);
            let text = storage.line_slice(target_line);
            let (offset, _) = char_at_visual_col(&text, col, ctx.tab_width);
            (target_start + offset, Some(col))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::storage::RopeStorage;

    fn s(text: &str) -> RopeStorage {
        RopeStorage::from_text(text)
    }

    #[test]
    fn grapheme_motion_respects_clusters() {
        let doc = s("aé👨‍👩‍👧b");
        // indices: a=0 é=1 family=2..7 b=7
        assert_eq!(right(&doc, 0), 1);
        assert_eq!(right(&doc, 1), 2);
        assert_eq!(right(&doc, 2), 7);
        assert_eq!(left(&doc, 7), 2);
        assert_eq!(left(&doc, 2), 1);
    }

    #[test]
    fn combining_marks_are_one_step() {
        let doc = s("e\u{0301}x");
        assert_eq!(right(&doc, 0), 2);
        assert_eq!(left(&doc, 2), 0);
    }

    #[test]
    fn word_motions() {
        let doc = s("hello brave  world.rs");
        assert_eq!(word_right(&doc, 0), 6);
        assert_eq!(word_right(&doc, 6), 13);
        assert_eq!(word_left(&doc, 17), 13);
        assert_eq!(word_left(&doc, 13), 6);
        assert_eq!(word_left(&doc, 0), 0);
    }

    #[test]
    fn vertical_motion_keeps_column() {
        let doc = s("abcdef\nxy\nabcdef");
        let ctx = MotionCtx::default();
        let (pos, col) = apply(&doc, 4, Motion::Down, ctx, None);
        assert_eq!(pos, 9, "clamped to end of short line");
        assert_eq!(col, Some(4));
        let (pos2, _) = apply(&doc, pos, Motion::Down, ctx, col);
        assert_eq!(pos2, 14, "column 4 restored on the long line");
    }

    #[test]
    fn vertical_motion_respects_tabs() {
        // "\tab" puts the cursor after the tab at visual column 4.
        let doc = s("\tab\n123456789");
        let ctx = MotionCtx {
            tab_width: 4,
            page: 10,
        };
        let (pos, _) = apply(&doc, 1, Motion::Down, ctx, None);
        assert_eq!(pos, 8, "tab counts as 4 columns, not 1 character");
    }

    #[test]
    fn line_bounds() {
        let doc = s("one\ntwo\n");
        let ctx = MotionCtx::default();
        let (p, _) = apply(&doc, 5, Motion::LineEnd, ctx, None);
        assert_eq!(p, 7);
        let (p, _) = apply(&doc, 5, Motion::LineStart, ctx, None);
        assert_eq!(p, 4);
        let (p, _) = apply(&doc, 4, Motion::DocEnd, ctx, None);
        assert_eq!(p, 8);
    }

    #[test]
    fn smart_home_toggles() {
        let doc = s("    indented");
        let ctx = MotionCtx::default();
        let (p, _) = apply(&doc, 13, Motion::LineStartSmart, ctx, None);
        assert_eq!(p, 4);
        let (p, _) = apply(&doc, p, Motion::LineStartSmart, ctx, None);
        assert_eq!(p, 0);
    }
}
