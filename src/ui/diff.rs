//! Drawing the diff view.
//!
//! Two columns, one header, one scroll offset. The rows come from
//! [`crate::diff`] already paired, so this file never has to think about which
//! lines correspond: it draws row *n* of the left column and row *n* of the
//! right one and the alignment takes care of itself.
//!
//! Nothing here is quadratic. Only the visible rows are touched, each row is
//! one `line_slice` per side, and the gutters are filled once per frame rather
//! than per row.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::app::App;
use crate::buffer::motion::char_width;
use crate::buffer::Document;
use crate::diff::Kind;
use crate::theme::Theme;

/// Which column is being drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    /// The mark in the sign column for this side.
    ///
    /// A deletion is `-` on the left and nothing on the right; an insertion the
    /// other way round; a changed row is `~` on both, because both moved.
    fn sign(self, kind: Kind) -> char {
        match (self, kind) {
            (_, Kind::Same) => ' ',
            (Side::Left, Kind::Removed) => '-',
            (Side::Right, Kind::Added) => '+',
            (_, Kind::Changed) => '~',
            // A changed row always has both sides; this is unreachable but the
            // match has to be total.
            (Side::Left, Kind::Added) => ' ',
            (Side::Right, Kind::Removed) => ' ',
        }
    }

    fn background(self, theme: &Theme, kind: Kind, present: bool) -> Color {
        if !present {
            return theme.diff_filler_bg;
        }
        match (self, kind) {
            (Side::Left, Kind::Removed | Kind::Changed) => theme.diff_del_bg,
            (Side::Right, Kind::Added | Kind::Changed) => theme.diff_add_bg,
            _ => theme.bg,
        }
    }

    fn sign_colour(self, theme: &Theme, kind: Kind) -> Color {
        match (self, kind) {
            (Side::Left, Kind::Removed) => theme.error,
            (Side::Right, Kind::Added) => theme.success,
            (_, Kind::Changed) => theme.warn,
            _ => theme.dim,
        }
    }
}

/// Draw the whole view into `area`, header included.
pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App) {
    if area.width < 12 || area.height < 2 {
        return;
    }
    let theme = app.theme.clone();
    let tab_width = app.config.tab_width();
    let Some(view) = app.diff.as_mut() else {
        return;
    };

    let header = Rect::new(area.x, area.y, area.width, 1);
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    // The scroll keys need to know what a page is, and this is the only place
    // that knows how much room there is.
    view.height = body.height as usize;

    draw_header(buf, header, &theme, view);

    // One divider column, the rest shared as evenly as the width allows.
    let left_w = area.width.saturating_sub(1) / 2;
    let right_w = area.width.saturating_sub(1) - left_w;
    let left_rect = Rect::new(body.x, body.y, left_w, body.height);
    let right_rect = Rect::new(body.x + left_w + 1, body.y, right_w, body.height);

    for y in body.y..body.y + body.height {
        let cell = &mut buf[(body.x + left_w, y)];
        cell.set_symbol("▏")
            .set_style(Style::default().fg(theme.line));
    }

    let left_doc = &app.buffers[view.left];
    let right_doc = &app.buffers[view.right];
    let left_num = digits(view.left_lines);
    let right_num = digits(view.right_lines);

    let scroll = view.scroll;
    for offset in 0..body.height as usize {
        let Some(row) = view.rows.get(scroll + offset) else {
            // Past the end of the comparison: the columns are painted with the
            // document's own background rather than left as garbage from the
            // previous frame.
            clear_row(buf, left_rect, body.y + offset as u16, &theme);
            clear_row(buf, right_rect, body.y + offset as u16, &theme);
            continue;
        };
        draw_side(
            buf,
            left_rect,
            body.y + offset as u16,
            &theme,
            tab_width,
            left_doc,
            row.left_line(),
            row.kind,
            Side::Left,
            left_num,
        );
        draw_side(
            buf,
            right_rect,
            body.y + offset as u16,
            &theme,
            tab_width,
            right_doc,
            row.right_line(),
            row.kind,
            Side::Right,
            right_num,
        );
    }
}

/// The bar above the columns: what is being compared, how much, and how far
/// through the changes the view is.
fn draw_header(buf: &mut Buffer, rect: Rect, theme: &Theme, view: &crate::app::DiffView) {
    let base = Style::default().bg(theme.hint_bg).fg(theme.topbar_fg);
    for x in rect.x..rect.x + rect.width {
        buf[(x, rect.y)].set_symbol(" ").set_style(base);
    }

    let mut x = rect.x + 1;
    let right_edge = rect.x + rect.width;
    let put = |buf: &mut Buffer, x: &mut u16, text: &str, style: Style| {
        for c in text.chars() {
            if *x >= right_edge {
                return;
            }
            let mut tmp = [0u8; 4];
            buf[(*x, rect.y)]
                .set_symbol(c.encode_utf8(&mut tmp))
                .set_style(style);
            *x += 1;
        }
    };

    // The two names, then the counts, then the position — in that order, so a
    // narrow window loses the position before it loses the names.
    put(
        buf,
        &mut x,
        &view.left_label,
        base.add_modifier(Modifier::BOLD),
    );
    put(
        buf,
        &mut x,
        "  →  ",
        Style::default().bg(theme.hint_bg).fg(theme.dim),
    );
    put(
        buf,
        &mut x,
        &view.right_label,
        base.add_modifier(Modifier::BOLD),
    );

    let summary = if view.stats.is_same() {
        "  identical  ".to_string()
    } else {
        format!(
            "  +{}  −{}  ~{}  ",
            view.stats.added, view.stats.removed, view.stats.changed
        )
    };
    let position = view
        .position()
        .map(|(here, total)| format!(" {here}/{total} changes "))
        .unwrap_or_default();
    let tail = summary.chars().count() + position.chars().count();
    let start = right_edge.saturating_sub(tail as u16).max(x);
    let mut tx = start;
    let summary_style = Style::default()
        .bg(theme.hint_bg)
        .fg(if view.stats.is_same() {
            theme.dim
        } else {
            theme.topbar_fg
        });
    put(buf, &mut tx, &summary, summary_style);
    put(
        buf,
        &mut tx,
        &position,
        Style::default().bg(theme.hint_bg).fg(theme.mark),
    );
}

fn clear_row(buf: &mut Buffer, rect: Rect, y: u16, theme: &Theme) {
    let style = Style::default().bg(theme.bg).fg(theme.dim);
    for x in rect.x..rect.x + rect.width {
        buf[(x, y)].set_symbol(" ").set_style(style);
    }
}

/// One row of one column.
#[allow(clippy::too_many_arguments)]
fn draw_side(
    buf: &mut Buffer,
    rect: Rect,
    y: u16,
    theme: &Theme,
    tab_width: usize,
    doc: &Document,
    line: Option<usize>,
    kind: Kind,
    side: Side,
    num_w: usize,
) {
    let present = line.is_some();
    let bg = side.background(theme, kind, present);
    let base = Style::default().bg(bg).fg(theme.fg);
    for x in rect.x..rect.x + rect.width {
        buf[(x, y)].set_symbol(" ").set_style(base);
    }
    let Some(line) = line else {
        // A gap: the background is the whole message.
        return;
    };

    let sign_x = rect.x;
    let num_x = rect.x + 1;
    let text_x = num_x + num_w as u16 + 1;
    if text_x >= rect.x + rect.width {
        return;
    }

    let sign = side.sign(kind);
    if sign != ' ' {
        buf[(sign_x, y)].set_symbol(&sign.to_string()).set_style(
            Style::default()
                .bg(bg)
                .fg(side.sign_colour(theme, kind))
                .add_modifier(Modifier::BOLD),
        );
    }

    // The number is the line's own, so a reader can still quote line 412 to a
    // colleague even though the row it is on is not row 412 of the view.
    let label = (line + 1).to_string();
    let pad = num_w.saturating_sub(label.chars().count());
    let mut x = num_x;
    for _ in 0..pad {
        x += 1;
    }
    let num_style = Style::default().bg(bg).fg(if kind.is_change() {
        theme.topbar_fg
    } else {
        theme.gutter_fg
    });
    for c in label.chars() {
        if x >= text_x {
            break;
        }
        let mut tmp = [0u8; 4];
        buf[(x, y)]
            .set_symbol(c.encode_utf8(&mut tmp))
            .set_style(num_style);
        x += 1;
    }

    let text = doc.storage().line_slice(line);
    let limit = (rect.x + rect.width).saturating_sub(text_x) as usize;
    paint_text(buf, text_x, y, limit, &text, base, tab_width);
}

/// Write `text` from `x`, expanding tabs and stopping at `limit` columns.
fn paint_text(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    limit: usize,
    text: &str,
    style: Style,
    tab_width: usize,
) {
    let mut col = 0usize;
    let mut cx = x;
    for c in text.chars() {
        if col >= limit {
            break;
        }
        let w = char_width(c, col, tab_width).max(1);
        if col + w > limit {
            break;
        }
        if c == '\t' || c.is_control() {
            for _ in 0..w {
                buf[(cx, y)].set_symbol(" ").set_style(style);
                cx += 1;
            }
        } else {
            let mut tmp = [0u8; 4];
            buf[(cx, y)]
                .set_symbol(c.encode_utf8(&mut tmp))
                .set_style(style);
            for k in 1..w as u16 {
                buf[(cx + k, y)].set_symbol("").set_style(style);
            }
            cx += w as u16;
        }
        col += w;
    }
}

/// Width of a line-number column, matching the editor's minimum of three.
fn digits(lines: usize) -> usize {
    lines.max(1).to_string().len().max(3)
}

/// Rows the view can show, for tests and for the scroll keys.
pub fn visible_rows(app: &App) -> usize {
    app.diff.as_ref().map_or(0, |v| v.height)
}

/// The colour of a row's background, for tests.
///
/// Exposed rather than private because the interesting assertions about this
/// view are about colour: "an added line is tinted green" is the feature.
#[doc(hidden)]
pub fn row_background(theme: &Theme, side: &str, kind: Kind, present: bool) -> Color {
    let side = if side == "left" {
        Side::Left
    } else {
        Side::Right
    };
    side.background(theme, kind, present)
}
