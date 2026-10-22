//! The editing surface: gutter, text, cursor, selection and highlights.
//!
//! Rendering is done cell by cell into the frame buffer rather than through a
//! `Widget`, because the editor needs exact control over tabs, wide characters,
//! control characters, selection edges and horizontal scrolling.
//!
//! Two properties keep this fast on large files:
//!
//! * only the visible rows are materialised, and only the visible *window* of
//!   each row, so a 10 MiB single-line file costs the same as a short one;
//! * syntax tokens come from a cached per-line lexer state, so scrolling does
//!   not re-lex the document.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use crate::buffer::motion::{char_width, visual_col_of};
use crate::buffer::Document;
use crate::config::Config;
use crate::search::SearchState;
use crate::syntax::{self, StateCache, Token};
use crate::theme::Theme;

/// Characters of a line we are willing to materialise in one go.
const WHOLE_LINE_LIMIT: usize = 8 * 1024;
/// Cap on the cursor-column scan for pathological single-line files.
const CURSOR_SCAN_LIMIT: usize = 200_000;

/// A slice of a line, positioned both in characters and in display columns.
struct Window {
    text: String,
    start_char: usize,
    start_col: usize,
}

/// Extract the part of `line` starting at display column `left`, wide enough to
/// fill `cells` columns.
fn line_window(doc: &Document, line: usize, left: usize, cells: usize, tab_width: usize) -> Window {
    let storage = doc.storage();
    let line_start = storage.line_start(line);
    let line_len = storage.line_slice(line).chars().count();

    if line_len <= WHOLE_LINE_LIMIT {
        let text = storage.line_slice(line);
        // The whole line is cheap to materialise, but it still has to start at
        // the scroll offset: painting it from column 0 would ignore
        // `scroll_left` entirely, which silently hid the cursor whenever a line
        // was wider than the window.
        if left == 0 {
            return Window {
                text,
                start_char: 0,
                start_col: 0,
            };
        }
        let mut ci = 0usize;
        let mut col = 0usize;
        for c in text.chars() {
            if col >= left {
                break;
            }
            col += char_width(c, col, tab_width);
            ci += 1;
        }
        return Window {
            text: text.chars().skip(ci).collect(),
            start_char: ci,
            start_col: col,
        };
    }

    // Long line: walk forward in chunks until we reach the scroll offset.
    let mut ci = 0usize;
    let mut col = 0usize;
    'outer: while ci < line_len {
        let chunk_end = (ci + 1024).min(line_len);
        let chunk = storage.slice_chars(line_start + ci, line_start + chunk_end);
        for c in chunk.chars() {
            if col >= left {
                break 'outer;
            }
            col += char_width(c, col, tab_width);
            ci += 1;
        }
    }
    let end = (ci + cells + 8).min(line_len);
    Window {
        text: storage.slice_chars(line_start + ci, line_start + end),
        start_char: ci,
        start_col: col,
    }
}

/// Visual column of the cursor, bounded so a huge line cannot stall a frame.
fn cursor_visual_col(doc: &Document, line: usize, tab_width: usize) -> usize {
    let storage = doc.storage();
    let line_start = storage.line_start(line);
    let cursor = doc.cursor();
    if cursor <= line_start {
        return 0;
    }
    let chars = cursor - line_start;
    if chars <= CURSOR_SCAN_LIMIT {
        visual_col_of(storage, line_start, cursor, tab_width)
    } else {
        // Approximate on pathological single-line documents; the cursor stays
        // visible, which is what matters.
        visual_col_of(
            storage,
            line_start,
            line_start + CURSOR_SCAN_LIMIT,
            tab_width,
        )
    }
}

/// Style inputs shared by every painted row.
struct PaintCtx<'a> {
    theme: &'a Theme,
    base: Style,
    /// Every caret's selection, ascending.
    selections: &'a [Range<usize>],
    matches: &'a [Range<usize>],
    current_match: Option<usize>,
    brackets: Option<(usize, usize)>,
    tab_width: usize,
    show_ws: bool,
    show_guides: bool,
    cursor: usize,
    /// Positions of the secondary carets, ascending. The primary caret is drawn
    /// by the terminal itself.
    extra_cursors: &'a [usize],
    cur_line: usize,
    text_x: u16,
    text_w: usize,
    left: usize,
}

/// Paint one line into `row`. Returns the screen position of the cursor when it
/// is on this line.
#[allow(clippy::too_many_arguments)]
fn paint_line(
    buf: &mut Buffer,
    ctx: &PaintCtx<'_>,
    row: u16,
    line: usize,
    line_start: usize,
    line_len: usize,
    window: &Window,
    chars: &[char],
    tokens: &[Token],
) -> Option<Position> {
    let theme = ctx.theme;
    let right = ctx.text_x + ctx.text_w as u16;

    let line_end = line_start + line_len;
    let mut token_idx = 0usize;
    let mut match_idx = ctx.matches.partition_point(|r| r.end <= line_start);
    let mut sel_idx = ctx.selections.partition_point(|r| r.end <= line_start);

    let mut col = window.start_col;
    let mut ci = 0usize;
    let mut x = ctx.text_x;
    let mut cursor_pos = None;

    while x < right && ci < chars.len() {
        let c = chars[ci];
        let abs = line_start + window.start_char + ci;

        // --- style -----------------------------------------------------
        let mut style = ctx.base;
        while token_idx < tokens.len() && tokens[token_idx].end() <= ci {
            token_idx += 1;
        }
        if token_idx < tokens.len() && tokens[token_idx].start <= ci {
            style = style.fg(theme.syntax.get(tokens[token_idx].kind));
        }
        while match_idx < ctx.matches.len() && ctx.matches[match_idx].end <= abs {
            match_idx += 1;
        }
        if match_idx < ctx.matches.len() {
            let m = &ctx.matches[match_idx];
            if m.start <= abs && abs < m.end {
                let is_current = ctx.current_match == Some(match_idx);
                style = style
                    .bg(if is_current {
                        theme.current_match_bg
                    } else {
                        theme.match_bg
                    })
                    .fg(theme.match_fg);
            }
        }
        while sel_idx < ctx.selections.len() && ctx.selections[sel_idx].end <= abs {
            sel_idx += 1;
        }
        if sel_idx < ctx.selections.len() {
            let sel = &ctx.selections[sel_idx];
            if sel.start <= abs && abs < sel.end {
                style = style.bg(theme.selection_bg).fg(theme.selection_fg);
            }
        }
        if let Some((a, b)) = ctx.brackets {
            if abs == a || abs == b {
                style = style.fg(theme.bracket_match).add_modifier(Modifier::BOLD);
            }
        }
        // Secondary carets are painted as a block over their character, the
        // same way the terminal draws the primary one.
        if ctx.extra_cursors.binary_search(&abs).is_ok() {
            style = style.bg(theme.cursor_bg).fg(theme.cursor_fg);
        }
        if abs == ctx.cursor && line == ctx.cur_line {
            cursor_pos.get_or_insert(Position::new(x, row));
        }

        // --- glyph -----------------------------------------------------
        let w = char_width(c, col, ctx.tab_width);
        if c == '\t' {
            for k in 0..w {
                if x >= right {
                    break;
                }
                let sym = if ctx.show_ws && k + 1 == w {
                    "→"
                } else {
                    " "
                };
                buf[(x, row)].set_symbol(sym).set_style(style);
                x += 1;
            }
        } else if c.is_control() {
            if x + 2 <= right {
                buf[(x, row)].set_symbol("^").set_style(
                    Style::default()
                        .fg(theme.dim)
                        .bg(style.bg.unwrap_or(theme.bg)),
                );
                let letter = ((c as u8) & 0x1F).wrapping_add(b'@') as char;
                buf[(x + 1, row)]
                    .set_symbol(&letter.to_string())
                    .set_style(style);
            }
            x += 2;
        } else if w == 0 {
            // Zero-width characters attach to the previous cell.
            if x > ctx.text_x {
                let mut s = buf[(x - 1, row)].symbol().to_string();
                s.push(c);
                buf[(x - 1, row)].set_symbol(&s);
            }
        } else if c == ' ' && ctx.show_ws {
            // Mark spaces too, otherwise trailing whitespace stays invisible.
            buf[(x, row)].set_symbol("·").set_style(
                Style::default()
                    .fg(theme.dim)
                    .bg(style.bg.unwrap_or(theme.bg)),
            );
            x += 1;
        } else {
            if x + w as u16 > right {
                break;
            }
            let mut tmp = [0u8; 4];
            buf[(x, row)]
                .set_symbol(c.encode_utf8(&mut tmp))
                .set_style(style);
            for k in 1..w as u16 {
                buf[(x + k, row)].set_symbol("").set_style(style);
            }
            x += w as u16;
        }

        col += w;
        ci += 1;
    }

    // Cursor at end of line (or on an empty line).
    if line == ctx.cur_line && cursor_pos.is_none() && ctx.cursor >= line_end {
        cursor_pos = Some(Position::new(x.min(right.saturating_sub(1)), row));
    }

    // Selection covering the line break.
    let selection_covers_break = ctx
        .selections
        .iter()
        .any(|sel| sel.start <= line_end && sel.end > line_end);
    if selection_covers_break && x < right {
        buf[(x, row)]
            .set_symbol(" ")
            .set_style(Style::default().bg(theme.selection_bg));
    }

    // A secondary caret at the end of the line has no character to paint over,
    // so give it its own cell.
    if x < right && ctx.extra_cursors.binary_search(&line_end).is_ok() {
        buf[(x, row)]
            .set_symbol(" ")
            .set_style(Style::default().bg(theme.cursor_bg));
    }

    // Truncation marker when the line continues past the right edge.
    if ci < chars.len() && right > ctx.text_x {
        buf[(right - 1, row)]
            .set_symbol("›")
            .set_style(Style::default().fg(theme.dim).bg(theme.bg));
    }

    cursor_pos
}

/// Everything the editor needs to draw itself.
pub struct EditorRenderer<'a> {
    pub doc: &'a mut Document,
    pub theme: &'a Theme,
    pub cfg: &'a Config,
    pub search: &'a SearchState,
    pub brackets: Option<(usize, usize)>,
    pub cache: &'a mut StateCache,
    pub focused: bool,
}

impl EditorRenderer<'_> {
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rows = area.height as usize;
        self.doc.set_viewport_rows(rows);

        let tab_width = self.cfg.tab_width();
        let line_count = self.doc.line_count();
        let (cur_line, _) = self.doc.cursor_line_col();

        // --- vertical scroll -------------------------------------------------
        // The viewport is normally re-derived from the cursor every frame, so
        // that motions keep the caret on screen. A scroll gesture detaches it:
        // the user asked to look somewhere else, and snapping back on the next
        // draw would undo the gesture. `attach_view` is called by every cursor
        // movement, so the editor resumes following as soon as one happens.
        let scroll_off = self.cfg.editor.scroll_off.min(rows.saturating_sub(1) / 2);
        let mut top = self.doc.scroll_top().min(line_count.saturating_sub(1));
        let following = !self.doc.view_detached();
        if following {
            if cur_line < top + scroll_off {
                top = cur_line.saturating_sub(scroll_off);
            }
            if cur_line + scroll_off >= top + rows {
                top = (cur_line + scroll_off + 1).saturating_sub(rows);
            }
            top = top.min(line_count.saturating_sub(1));
        }
        self.doc.set_scroll_top(top);

        // --- layout ----------------------------------------------------------
        let digits = decimal_digits(line_count).max(3);
        let show_numbers = self.cfg.editor.line_numbers;
        let gutter_w: u16 = if show_numbers { (digits + 1) as u16 } else { 0 };
        let text_x = area.x + gutter_w;
        let text_w = area.width.saturating_sub(gutter_w) as usize;
        if text_w == 0 {
            return;
        }

        // --- horizontal scroll ----------------------------------------------
        // Same rule as above: a detached view keeps the column the wheel left
        // it on instead of being pulled back to the caret.
        let cur_col = cursor_visual_col(self.doc, cur_line, tab_width);
        let cur_line_len = self.doc.storage().line_slice(cur_line).chars().count();
        let mut left = self.doc.scroll_left();
        if following {
            if cur_line_len < WHOLE_LINE_LIMIT {
                let start = self.doc.storage().line_start(cur_line);
                let width =
                    visual_col_of(self.doc.storage(), start, start + cur_line_len, tab_width);
                if width <= text_w {
                    left = 0;
                }
            }
            if cur_col < left {
                left = cur_col;
            } else if cur_col >= left + text_w {
                left = cur_col + 1 - text_w;
            }
        } else {
            // A detached view keeps the column the wheel left it on, but never
            // past the end of the longest line: there is nothing to see there.
            let limit = self.doc.content_width(tab_width).saturating_sub(text_w);
            left = left.min(limit);
        }
        self.doc.set_scroll_left(left);

        let theme = self.theme;
        let base = Style::default().fg(theme.fg).bg(theme.bg);
        let buf = frame.buffer_mut();
        buf.set_style(area, base);
        if show_numbers {
            let gutter = Rect::new(area.x, area.y, gutter_w, area.height);
            buf.set_style(
                gutter,
                Style::default().fg(theme.gutter_fg).bg(theme.gutter_bg),
            );
        }

        let selections = self.doc.selections();
        let extra_positions: Vec<usize> = self.doc.extra_carets().iter().map(|c| c.pos).collect();
        let caret_lines: Vec<usize> = self
            .doc
            .carets()
            .iter()
            .map(|c| self.doc.storage().line_of_char(c.pos))
            .collect();

        let ctx = PaintCtx {
            theme,
            base,
            selections: &selections,
            matches: self.search.ranges(),
            current_match: self.search.matches.current,
            brackets: self.brackets,
            tab_width,
            show_ws: self.cfg.editor.show_whitespace,
            show_guides: self.cfg.editor.show_indent_guides,
            cursor: self.doc.cursor(),
            extra_cursors: &extra_positions,
            cur_line,
            text_x,
            text_w,
            left,
        };

        let lang = self.doc.language();
        let syntax_on = self.cfg.editor.syntax_highlighting && lang.has_highlighting();
        let mut cursor_pos: Option<Position> = None;
        let mut scratch_tokens: Vec<Token> = Vec::new();

        for row in 0..rows {
            let y = area.y + row as u16;
            let line = top + row;

            // ---------------- gutter ----------------
            if show_numbers {
                if line >= line_count {
                    buf.set_string(
                        area.x,
                        y,
                        " ".repeat(digits),
                        Style::default().fg(theme.dim),
                    );
                } else {
                    let (n, style) = if line == cur_line {
                        (
                            line + 1,
                            Style::default()
                                .fg(theme.gutter_active_fg)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if self.cfg.editor.relative_numbers {
                        (
                            line.abs_diff(cur_line),
                            Style::default().fg(theme.gutter_fg),
                        )
                    } else {
                        (line + 1, Style::default().fg(theme.gutter_fg))
                    };
                    let label = n.to_string();
                    let pad = digits.saturating_sub(label.chars().count());
                    let mut s = String::with_capacity(digits);
                    for _ in 0..pad {
                        s.push(' ');
                    }
                    s.push_str(&label);
                    buf.set_string(area.x, y, &s, style);
                }
            }

            if line >= line_count {
                buf.set_string(text_x, y, "~", Style::default().fg(theme.dim).bg(theme.bg));
                continue;
            }

            // ---------------- row background ----------------
            if self.cfg.editor.highlight_cursor_line && caret_lines.contains(&line) && self.focused
            {
                let band = Rect::new(text_x, y, text_w as u16, 1);
                buf.set_style(band, Style::default().bg(theme.cursor_line_bg));
            }

            // ---------------- content ----------------
            let storage = self.doc.storage();
            let line_start = storage.line_start(line);
            let line_len = storage.line_slice(line).chars().count();
            let window = line_window(self.doc, line, left, text_w, tab_width);
            let chars: Vec<char> = window.text.chars().collect();

            if ctx.show_guides {
                draw_indent_guides(buf, &ctx, y, &window, line_start, area);
            }

            scratch_tokens.clear();
            if syntax_on {
                let state = self
                    .cache
                    .state_for(line, &mut |l| storage.line_slice(l), lang);
                let (tokens, _) = syntax::lex_line(lang, &window.text, state);
                scratch_tokens = tokens;
            }

            // Paint the row; the renderer owns row placement.
            if let Some(pos) = paint_line(
                buf,
                &ctx,
                y,
                line,
                line_start,
                line_len,
                &window,
                &chars,
                &scratch_tokens,
            ) {
                if pos.x < area.x + area.width {
                    cursor_pos = Some(pos);
                }
            }
        }

        if let Some(pos) = cursor_pos {
            frame.set_cursor_position(pos);
        }
    }
}

/// Indent guides are drawn before text so glyphs win on occupied cells.
fn draw_indent_guides(
    buf: &mut Buffer,
    ctx: &PaintCtx<'_>,
    y: u16,
    window: &Window,
    _line_start: usize,
    area: Rect,
) {
    let leading: usize = window
        .text
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { ctx.tab_width } else { 1 })
        .sum();
    let mut guide = ctx.tab_width;
    while guide < leading {
        if guide >= ctx.left {
            let gx = ctx.text_x + (guide - ctx.left) as u16;
            if gx < area.x + area.width {
                let cell = &mut buf[(gx, y)];
                if cell.symbol() == " " {
                    cell.set_symbol("│")
                        .set_style(Style::default().fg(ctx.theme.indent_guide));
                }
            }
        }
        guide += ctx.tab_width;
    }
}

/// Number of decimal digits in `n` (at least 1).
pub fn decimal_digits(n: usize) -> usize {
    let mut d = 1;
    let mut v = n;
    while v >= 10 {
        v /= 10;
        d += 1;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_to_string(doc: &mut Document, cfg: &Config, width: u16, height: u16) -> String {
        let theme = Theme::dark();
        let search = SearchState::new();
        let mut cache = StateCache::new();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                let mut r = EditorRenderer {
                    doc,
                    theme: &theme,
                    cfg,
                    search: &search,
                    brackets: None,
                    cache: &mut cache,
                    focused: true,
                };
                r.render(frame, area);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn digits() {
        assert_eq!(decimal_digits(0), 1);
        assert_eq!(decimal_digits(9), 1);
        assert_eq!(decimal_digits(10), 2);
        assert_eq!(decimal_digits(1_234_567), 7);
    }

    #[test]
    fn renders_text_and_gutter() {
        let mut doc = Document::from_text("hello\nworld", None);
        let cfg = Config::default();
        let out = render_to_string(&mut doc, &cfg, 20, 4);
        assert!(out.contains("hello"), "{out}");
        assert!(out.contains("world"), "{out}");
        assert!(out.contains("  1"), "gutter: {out}");
    }

    #[test]
    fn renders_tabs_as_spaces() {
        let mut doc = Document::from_text("\tx", None);
        let cfg = Config::default();
        let out = render_to_string(&mut doc, &cfg, 20, 2);
        assert!(out.contains("    x"), "{out}");
    }

    #[test]
    fn shows_tilde_past_end_of_file() {
        let mut doc = Document::from_text("one", None);
        let cfg = Config::default();
        let out = render_to_string(&mut doc, &cfg, 10, 4);
        assert!(out.contains('~'), "{out}");
    }

    #[test]
    fn narrow_terminal_does_not_panic() {
        let mut doc = Document::from_text("a very long line indeed", None);
        let cfg = Config::default();
        for w in 1..6u16 {
            let _ = render_to_string(&mut doc, &cfg, w, 3);
        }
    }

    #[test]
    fn zero_size_area_is_safe() {
        let mut doc = Document::from_text("x", None);
        let cfg = Config::default();
        let _ = render_to_string(&mut doc, &cfg, 0, 0);
    }
}
