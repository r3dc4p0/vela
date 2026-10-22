//! Word wrap: how one logical line becomes several visual rows.
//!
//! Wrapping is a pure function of `(text, width, tab_width)` with no state and
//! no knowledge of the terminal, so the renderer, the cursor motions and the
//! mouse mapping cannot disagree about where a row ends: they all ask
//! [`LineLayout`]. A second implementation that "just" recomputed the break
//! points would drift, and the symptom — a cursor that jumps a character when
//! it crosses a row boundary — is miserable to debug.
//!
//! The rules, in order of precedence:
//!
//! 1. A row holds as many characters as fit in `width` display columns.
//! 2. When the next character does not fit, the row ends after the last run of
//!    whitespace in it, so a continuation row starts with a word rather than
//!    with the space that separated it from the previous row.
//! 3. A word longer than a row is broken at the column limit.
//! 4. Whitespace that does not fit hangs past the edge instead of opening the
//!    next row with a blank. Hanging whitespace is clipped when painted, which
//!    is invisible, and keeps every row starting on a character worth seeing.
//!
//! Segments partition the line: every character belongs to exactly one row, so
//! any cursor offset maps to a row, and back again.

use std::collections::HashMap;
use std::rc::Rc;

use super::motion::char_width;
use super::storage::TextStorage;
use super::WHOLE_LINE_LIMIT;

/// One visual row: a slice of a logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Character offset of the first character, from the start of the line.
    pub char_off: usize,
    /// Number of characters in the row.
    pub chars: usize,
    /// Display column at which the row starts, from the start of the line.
    ///
    /// Kept so tab stops stay aligned to the line rather than to the row: a tab
    /// is painted by its width at the *line* column, so a row that starts at
    /// column 37 must know that.
    pub start_col: usize,
    /// Display width of the row, in cells. Can exceed the wrap width by a
    /// hanging whitespace run (rule 4); the renderer clips it.
    pub width: usize,
}

impl Segment {
    /// Character offset one past the last character of the row.
    pub fn end_char(&self) -> usize {
        self.char_off + self.chars
    }
}

/// Break `chars` into rows of at most `width` display columns.
fn wrap_chars(chars: &[char], width: usize, tab_width: usize) -> Vec<Segment> {
    let width = width.max(1);
    let n = chars.len();
    let mut out = Vec::new();
    if n == 0 {
        out.push(Segment {
            char_off: 0,
            chars: 0,
            start_col: 0,
            width: 0,
        });
        return out;
    }

    let mut start = 0usize;
    let mut start_col = 0usize;
    let mut col = 0usize;
    let mut idx = 0usize;
    // Where the last whitespace run inside the current row ended: the place a
    // break should go, if the row has to end.
    let mut brk: Option<(usize, usize)> = None;

    while idx < n {
        let c = chars[idx];
        let w = char_width(c, col, tab_width);
        // `col - start_col` is the row's width so far; measuring from the row
        // start is what makes a hanging row behave: after one, the next row
        // begins with a full budget again.
        if col - start_col + w > width && idx > start {
            if c.is_whitespace() {
                // Hang the run and break after it, so the next row starts on a
                // visible character. This is the case that would otherwise
                // produce rows consisting of a single space.
                let mut end = idx;
                let mut end_col = col;
                while end < n && chars[end].is_whitespace() {
                    end_col += char_width(chars[end], end_col, tab_width);
                    end += 1;
                }
                out.push(Segment {
                    char_off: start,
                    chars: end - start,
                    start_col,
                    width: end_col - start_col,
                });
                start = end;
                start_col = end_col;
                col = end_col;
                idx = end;
                brk = None;
            } else if let Some((b, bc)) = brk.filter(|(b, _)| *b > start && *b <= idx) {
                out.push(Segment {
                    char_off: start,
                    chars: b - start,
                    start_col,
                    width: bc - start_col,
                });
                start = b;
                start_col = bc;
                col = bc;
                brk = None;
                // Replay what was measured past the break, so `col` and `brk`
                // describe the new row rather than the old one.
                for k in b..idx {
                    col += char_width(chars[k], col, tab_width);
                    if chars[k].is_whitespace() && (k + 1 == n || !chars[k + 1].is_whitespace()) {
                        brk = Some((k + 1, col));
                    }
                }
            } else {
                out.push(Segment {
                    char_off: start,
                    chars: idx - start,
                    start_col,
                    width: col - start_col,
                });
                start = idx;
                start_col = col;
                brk = None;
            }
            continue;
        }
        col += w;
        idx += 1;
        if chars[idx - 1].is_whitespace() && (idx == n || !chars[idx].is_whitespace()) {
            brk = Some((idx, col));
        }
    }

    // A hanging whitespace run can consume the rest of the line; do not follow
    // it with an empty row.
    if start < n || out.is_empty() {
        out.push(Segment {
            char_off: start,
            chars: n - start,
            start_col,
            width: col - start_col,
        });
    }
    out
}

/// Character offset in `chars` closest to display column `target`, where
/// `chars[0]` sits at column `start_col`.
fn offset_at(chars: &[char], start_col: usize, target: usize, tab_width: usize) -> usize {
    let mut col = start_col;
    for (i, &c) in chars.iter().enumerate() {
        let w = char_width(c, col, tab_width);
        if col + w > target {
            return i;
        }
        col += w;
    }
    chars.len()
}

/// A line's characters and the rows they were wrapped into.
#[derive(Debug, Clone)]
pub struct LineLayout {
    text: String,
    chars: Vec<char>,
    segments: Vec<Segment>,
}

impl LineLayout {
    /// Wrap `text` into rows of at most `width` display columns.
    pub fn new(text: &str, width: usize, tab_width: usize) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let segments = wrap_chars(&chars, width, tab_width);
        Self {
            text: text.to_string(),
            chars,
            segments,
        }
    }

    /// The line as it was wrapped, line break excluded.
    ///
    /// Kept so the syntax highlighter can lex a whole line once and hand each
    /// row its slice: lexing a row in isolation would start every continuation
    /// row in the default state, mid-string and mid-comment.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The line, as characters.
    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    /// The rows, top to bottom. Never empty.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Number of visual rows the line occupies.
    pub fn height(&self) -> usize {
        self.segments.len()
    }

    /// Row containing character offset `char_off`.
    ///
    /// A position exactly on a boundary belongs to the later row: that is where
    /// the caret is drawn, since the character it precedes is the first of that
    /// row.
    pub fn row_of(&self, char_off: usize) -> usize {
        let mut lo = 0usize;
        let mut hi = self.segments.len();
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.segments[mid].char_off <= char_off {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Character offset in row `row` closest to display column `col`, where
    /// `col` is counted from the row's own left edge.
    pub fn pos_at(&self, row: usize, col: usize, tab_width: usize) -> usize {
        let Some(seg) = self.segments.get(row) else {
            return self.chars.len();
        };
        let text = &self.chars[seg.char_off..seg.end_char()];
        seg.char_off + offset_at(text, seg.start_col, seg.start_col + col, tab_width)
    }
}

/// Per-frame cache of line layouts, and the viewport arithmetic built on them.
///
/// The viewport is stored as `(line, row)`: the first visible line and the
/// first visible row *within* it. Keeping it in those terms — rather than as a
/// single global visual-row index — is what keeps every operation here
/// proportional to the number of rows on screen instead of the number of lines
/// above them.
#[derive(Debug)]
pub struct LayoutCache {
    /// Wrap width, or `None` when wrapping is off.
    width: Option<usize>,
    tab_width: usize,
    lines: HashMap<usize, Rc<LineLayout>>,
}

impl LayoutCache {
    pub fn new(width: Option<usize>, tab_width: usize) -> Self {
        Self {
            width,
            tab_width,
            lines: HashMap::new(),
        }
    }

    /// True when wrapping is in effect.
    pub fn wraps(&self) -> bool {
        self.width.is_some()
    }

    /// The layout of `line`, or `None` when the line is not wrapped — either
    /// because wrapping is off, or because the line is too long to expand
    /// cheaply.
    pub fn layout(&mut self, storage: &dyn TextStorage, line: usize) -> Option<Rc<LineLayout>> {
        let width = self.width?;
        if let Some(cached) = self.lines.get(&line) {
            return Some(cached.clone());
        }
        if storage.line_len(line) > WHOLE_LINE_LIMIT {
            return None;
        }
        let layout = Rc::new(LineLayout::new(
            &storage.line_slice(line),
            width,
            self.tab_width,
        ));
        self.lines.insert(line, layout.clone());
        Some(layout)
    }

    /// Visual rows occupied by `line`; 1 when it is not wrapped.
    pub fn height(&mut self, storage: &dyn TextStorage, line: usize) -> usize {
        match self.layout(storage, line) {
            Some(layout) => layout.height(),
            None => 1,
        }
    }

    /// The row `offset` rows below the viewport top `(top, sub)`.
    ///
    /// `None` once the walk runs past the end of the buffer.
    pub fn row_at(
        &mut self,
        storage: &dyn TextStorage,
        top: usize,
        sub: usize,
        offset: usize,
    ) -> Option<(usize, usize)> {
        let last = storage.len_lines().saturating_sub(1);
        let mut line = top.min(last);
        let mut seg = sub;
        let mut remaining = offset;
        loop {
            let h = self.height(storage, line);
            if seg >= h {
                // A stale sub-row (after an edit or a resize) lands at the
                // start of the next line rather than looping forever.
                line += 1;
                seg = 0;
                if line > last {
                    return None;
                }
                continue;
            }
            let available = h - seg;
            if remaining < available {
                return Some((line, seg + remaining));
            }
            remaining -= available;
            line += 1;
            seg = 0;
            if line > last {
                return None;
            }
        }
    }

    /// Move the viewport top `delta` rows (negative moves up), stopping at the
    /// ends of the buffer.
    pub fn shift(
        &mut self,
        storage: &dyn TextStorage,
        top: usize,
        sub: usize,
        delta: isize,
    ) -> (usize, usize) {
        let last = storage.len_lines().saturating_sub(1);
        let mut top = top.min(last);
        let mut sub = sub;
        let mut remaining = delta;
        while remaining > 0 {
            let h = self.height(storage, top);
            if sub + 1 < h {
                sub += 1;
            } else if top < last {
                top += 1;
                sub = 0;
            } else {
                break;
            }
            remaining -= 1;
        }
        while remaining < 0 {
            if sub > 0 {
                sub -= 1;
            } else if top > 0 {
                top -= 1;
                sub = self.height(storage, top).saturating_sub(1);
            } else {
                break;
            }
            remaining += 1;
        }
        (top, sub)
    }

    /// Rows between the viewport top `(top, sub)` and the cursor, or `None`
    /// when the cursor is further than `cap` rows below it.
    ///
    /// The answer is negative when the cursor is on the top line but above the
    /// first visible row of it, which is how "scrolled down inside one very
    /// tall line" is expressed.
    pub fn cursor_row(
        &mut self,
        storage: &dyn TextStorage,
        top: usize,
        sub: usize,
        cur_line: usize,
        cur_off: usize,
        cap: usize,
    ) -> Option<isize> {
        if cur_line < top {
            return None;
        }
        let cur_seg = match self.layout(storage, cur_line) {
            Some(layout) => layout.row_of(cur_off),
            None => 0,
        };
        let mut line = top;
        let mut seg = sub;
        let mut abs = 0usize;
        while line < cur_line {
            let h = self.height(storage, line);
            abs += h.saturating_sub(seg);
            if abs > cap {
                return None;
            }
            line += 1;
            seg = 0;
        }
        if cur_seg < seg {
            return Some(-((seg - cur_seg) as isize));
        }
        abs += cur_seg - seg;
        if abs > cap {
            return None;
        }
        Some(abs as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::storage::RopeStorage;

    fn wrap(text: &str, width: usize) -> Vec<Segment> {
        LineLayout::new(text, width, 4).segments().to_vec()
    }

    /// The text of each row, for readable assertions.
    fn rows(text: &str, width: usize) -> Vec<String> {
        let layout = LineLayout::new(text, width, 4);
        layout
            .segments()
            .iter()
            .map(|s| layout.chars()[s.char_off..s.end_char()].iter().collect())
            .collect()
    }

    #[test]
    fn a_short_line_is_one_row() {
        let segs = wrap("hello", 40);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].char_off, 0);
        assert_eq!(segs[0].chars, 5);
        assert_eq!(segs[0].width, 5);
    }

    #[test]
    fn an_empty_line_is_one_empty_row() {
        let segs = wrap("", 40);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].width, 0);
        assert_eq!(segs[0].chars, 0);
    }

    #[test]
    fn breaks_after_the_last_space_in_the_row() {
        // "hello " is exactly six columns, so the break lands after the space
        // and the continuation row starts with the word.
        assert_eq!(rows("hello world", 8), vec!["hello ", "world"]);
    }

    #[test]
    fn a_word_wider_than_the_row_is_broken() {
        assert_eq!(rows("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_space_that_does_not_fit_hangs_instead_of_starting_a_row() {
        // The space cannot fit in four columns, but opening the next row with
        // it would produce a row of one blank; hanging it is invisible.
        let segs = wrap("aaaa bbbb", 4);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].chars, 5, "the space belongs to the first row");
        assert_eq!(segs[1].char_off, 5);
        assert_eq!(segs[1].width, 4);
    }

    #[test]
    fn a_whitespace_only_line_does_not_grow_an_empty_row() {
        let segs = wrap("     ", 2);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].chars, 5);
        assert!(segs.iter().all(|s| s.chars > 0), "{segs:?}");
    }

    #[test]
    fn wide_characters_are_measured_in_cells() {
        // 日本語 is six cells: two per character.
        assert_eq!(rows("日本語", 4), vec!["日本", "語"]);
        let segs = wrap("日本語", 4);
        assert_eq!(segs[0].width, 4);
        assert_eq!(segs[1].width, 2);
    }

    #[test]
    fn tabs_are_measured_from_the_line_column() {
        // A tab at column 0 fills four cells, so "ab" moves to the next row.
        assert_eq!(rows("\tab", 4), vec!["\t", "ab"]);
        let segs = wrap("\tab", 4);
        assert_eq!(segs[0].width, 4);
        assert_eq!(segs[1].start_col, 4);
    }

    #[test]
    fn zero_width_still_terminates() {
        for text in ["abc", "a b c", "", "\t\t"] {
            let segs = LineLayout::new(text, 0, 4).segments().to_vec();
            assert!(!segs.is_empty(), "{text:?}");
            assert!(
                segs.iter().all(|s| s.chars > 0) || text.is_empty(),
                "{segs:?}"
            );
        }
    }

    #[test]
    fn segments_partition_the_line() {
        // The invariant every caller relies on: the rows cover the line exactly
        // once, in order, so a character offset has one home.
        let samples = [
            "hello world",
            "aaaa bbbb",
            "  leading and trailing   ",
            "oneverylongwordwithoutspaces",
            "日本語 テスト です",
            "\t\tindented\n",
            "a  b   c    d     e",
        ];
        for text in samples {
            for width in 1..12usize {
                let layout = LineLayout::new(text, width, 4);
                let segs = layout.segments();
                let mut expected = 0usize;
                for (i, s) in segs.iter().enumerate() {
                    assert_eq!(s.char_off, expected, "{text:?} width {width} seg {i}");
                    assert!(s.chars > 0, "empty row in {text:?} width {width}");
                    expected = s.end_char();
                }
                assert_eq!(
                    expected,
                    layout.chars().len(),
                    "{text:?} width {width} does not cover the line"
                );
            }
        }
    }

    #[test]
    fn rows_fit_the_width_except_for_hanging_whitespace() {
        for text in [
            "hello world again",
            "aaaa bbbb",
            "a b c d e f",
            "word   word",
        ] {
            for width in 4..14usize {
                let layout = LineLayout::new(text, width, 4);
                for seg in layout.segments() {
                    let content = &layout.chars()[seg.char_off..seg.end_char()];
                    let trailing_ws = content
                        .iter()
                        .rev()
                        .take_while(|c| c.is_whitespace())
                        .count();
                    let body = &content[..content.len() - trailing_ws];
                    let mut col = seg.start_col;
                    for &c in body {
                        col += char_width(c, col, 4);
                    }
                    assert!(
                        col - seg.start_col <= width,
                        "{text:?} width {width}: row wider than the viewport"
                    );
                }
            }
        }
    }

    #[test]
    fn row_of_finds_the_row_that_holds_a_position() {
        let layout = LineLayout::new("hello world", 8, 4);
        assert_eq!(layout.row_of(0), 0);
        assert_eq!(layout.row_of(5), 0, "the trailing space is on row 0");
        assert_eq!(layout.row_of(6), 1, "a boundary belongs to the later row");
        assert_eq!(
            layout.row_of(11),
            1,
            "one past the end stays on the last row"
        );
    }

    #[test]
    fn pos_at_is_the_inverse_of_row_of() {
        for text in ["hello world", "abcdefghij", "日本語 テスト"] {
            for width in 3..10usize {
                let layout = LineLayout::new(text, width, 4);
                for (i, _) in layout.chars().iter().enumerate() {
                    let row = layout.row_of(i);
                    let seg = layout.segments()[row];
                    let col = layout.chars()[seg.char_off..i]
                        .iter()
                        .fold(seg.start_col, |c, &ch| c + char_width(ch, c, 4))
                        - seg.start_col;
                    assert_eq!(
                        layout.pos_at(row, col, 4),
                        i,
                        "{text:?} width {width} offset {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn pos_at_clamps_past_the_end_of_a_row() {
        let layout = LineLayout::new("hello world", 8, 4);
        assert_eq!(layout.pos_at(0, 99, 4), 6, "end of row 0");
        assert_eq!(layout.pos_at(9, 0, 4), 11, "past the last row");
    }

    // ------------------------------------------------------------ viewport

    fn storage(text: &str) -> RopeStorage {
        RopeStorage::from_text(text)
    }

    #[test]
    fn heights_come_from_the_wrap() {
        let s = storage("hello world\nshort\n");
        let mut cache = LayoutCache::new(Some(8), 4);
        assert_eq!(cache.height(&s, 0), 2);
        assert_eq!(cache.height(&s, 1), 1);
        let mut plain = LayoutCache::new(None, 4);
        assert_eq!(plain.height(&s, 0), 1, "no wrap, one row per line");
    }

    #[test]
    fn very_long_lines_are_not_wrapped() {
        let text = "x".repeat(WHOLE_LINE_LIMIT + 1);
        let s = storage(&text);
        let mut cache = LayoutCache::new(Some(8), 4);
        assert_eq!(cache.height(&s, 0), 1);
        assert!(cache.layout(&s, 0).is_none());
    }

    #[test]
    fn row_at_walks_down_from_the_viewport_top() {
        let s = storage("hello world\nshort\naaa bbb ccc\n");
        let mut cache = LayoutCache::new(Some(8), 4);
        // Rows 0..1 are line 0, row 2 is line 1, rows 3..4 are line 2.
        assert_eq!(cache.row_at(&s, 0, 0, 0), Some((0, 0)));
        assert_eq!(cache.row_at(&s, 0, 0, 1), Some((0, 1)));
        assert_eq!(cache.row_at(&s, 0, 0, 2), Some((1, 0)));
        assert_eq!(cache.row_at(&s, 0, 0, 3), Some((2, 0)));
        assert_eq!(cache.row_at(&s, 0, 0, 4), Some((2, 1)));
        assert_eq!(
            cache.row_at(&s, 0, 0, 5),
            Some((3, 0)),
            "the empty line after the trailing newline is a row of its own"
        );
        assert_eq!(cache.row_at(&s, 0, 0, 6), None, "past the end");
    }

    #[test]
    fn row_at_starts_from_a_sub_row() {
        let s = storage("hello world\nshort\n");
        let mut cache = LayoutCache::new(Some(8), 4);
        assert_eq!(cache.row_at(&s, 0, 1, 0), Some((0, 1)));
        assert_eq!(cache.row_at(&s, 0, 1, 1), Some((1, 0)));
    }

    #[test]
    fn shift_moves_by_rows_and_stops_at_the_ends() {
        let s = storage("hello world\nshort\n");
        let mut cache = LayoutCache::new(Some(8), 4);
        assert_eq!(cache.shift(&s, 0, 0, 1), (0, 1));
        assert_eq!(cache.shift(&s, 0, 0, 2), (1, 0));
        assert_eq!(cache.shift(&s, 0, 1, -1), (0, 0));
        assert_eq!(cache.shift(&s, 0, 0, -5), (0, 0), "clamped at the top");
        assert_eq!(cache.shift(&s, 2, 0, 9), (2, 0), "clamped at the bottom");
    }

    #[test]
    fn shift_without_wrapping_moves_by_lines() {
        let s = storage("a\nb\nc\n");
        let mut cache = LayoutCache::new(None, 4);
        assert_eq!(cache.shift(&s, 0, 0, 2), (2, 0));
        assert_eq!(cache.shift(&s, 2, 0, -1), (1, 0));
    }

    #[test]
    fn cursor_row_counts_rows_from_the_top() {
        let s = storage("hello world\nshort\n");
        let mut cache = LayoutCache::new(Some(8), 4);
        // Cursor at offset 6 is on line 0, row 1.
        assert_eq!(cache.cursor_row(&s, 0, 0, 0, 6, 100), Some(1));
        // Cursor at line 1 is two rows down.
        assert_eq!(cache.cursor_row(&s, 0, 0, 1, 0, 100), Some(2));
        // Scrolled into the middle of a tall line, the cursor can be above.
        assert_eq!(cache.cursor_row(&s, 0, 1, 0, 0, 100), Some(-1));
    }

    #[test]
    fn cursor_row_gives_up_when_the_cursor_is_far_below() {
        let s = storage("a\nb\nc\nd\n");
        let mut cache = LayoutCache::new(None, 4);
        assert_eq!(cache.cursor_row(&s, 0, 0, 3, 0, 100), Some(3));
        assert_eq!(cache.cursor_row(&s, 0, 0, 3, 0, 2), None, "beyond the cap");
    }
}
