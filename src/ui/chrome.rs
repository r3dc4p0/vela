//! Window chrome: top bar, buffer tabs, status bar, shortcut hints, messages.
//!
//! These are pure functions of application state rendered into a `Buffer`; they
//! never mutate anything, which keeps layout bugs out of the editing path.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use super::canvas::{Canvas, VEdge};
use crate::app::App;
use crate::commands::Command;
use crate::config::UiStyle;
use crate::theme::Theme;
use crate::ui::Level;
use crate::VERSION;

/// Truncate to `width` display cells, appending an ellipsis when shortened.
pub fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    buf.set_style(area, style);
}

/// Blank a rectangle and give it a background, so nothing underneath shows
/// through.
///
/// `Buffer::set_style` only patches colours — the symbols already in the cells
/// survive. A panel that just sets a background therefore leaves the editor's
/// text visible in every cell it does not write itself, and the overlay reads
/// as if it were printed on top of the file rather than over it.
pub fn wipe(buf: &mut Buffer, area: Rect, bg: Color) {
    let area = area.intersection(buf.area);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_style(Style::default().bg(bg));
        }
    }
}

/// A hairline along the bottom edge of a row, drawn by underlining it.
///
/// The block glyphs draw a line *inside* a cell and a cell holds exactly one
/// glyph, so a row that is full of text cannot also carry dots. The terminal's
/// underline attribute is the one hairline that coexists with a glyph: it is
/// painted at the baseline, under the text, and costs neither a row nor a cell.
///
/// The colour comes from SGR 58. Terminals that ignore it draw the underline in
/// the cell's foreground instead, which is why the row's foreground is set to
/// the rule colour here, before any text is written over it.
pub fn underline_rule(buf: &mut Buffer, area: Rect, colour: Color) {
    buf.set_style(
        area,
        Style::default()
            .fg(colour)
            .underline_color(colour)
            .add_modifier(Modifier::UNDERLINED),
    );
}

/// Paint `(text, colour)` segments left to right, clipped at `right`.
fn paint_segments(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    right: u16,
    bg: Color,
    segments: &[(String, Color)],
) {
    let mut x = x;
    for (text, colour) in segments {
        if x >= right {
            break;
        }
        let shown = fit(text, (right - x) as usize);
        buf.set_string(x, y, &shown, Style::default().fg(*colour).bg(bg));
        x += shown.chars().count() as u16;
    }
}

/// The rule between the editing area and the status bar.
///
/// One row, of which only the top eighth is painted: the rest is the status
/// bar's own background, so the rule reads as that bar's top border with a
/// little padding above its text rather than as a separate row of chrome.
pub fn draw_rule(buf: &mut Buffer, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let mut canvas = Canvas::new(buf, area);
    canvas.fill(area, theme.statusbar_bg);
    canvas.hline(area.x, area.x + area.width, area.y, VEdge::Top, theme.line);
    canvas.flush();
}

/// Top bar: identity on the left, session facts on the right.
pub fn draw_topbar(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let pixel = app.config.ui_style() == UiStyle::Pixel;
    fill(
        buf,
        area,
        Style::default().bg(theme.topbar_bg).fg(theme.topbar_fg),
    );
    if pixel {
        underline_rule(buf, area, theme.line);
    }

    // In the pixel style the wordmark carries the accent colour of the design:
    // the same gold as the cursor and the active tab, so the identity, the
    // caret and the selection all read as one family.
    let wordmark = if pixel {
        theme.mark
    } else {
        theme.topbar_accent
    };
    buf.set_string(
        area.x,
        area.y,
        fit(" VELA ", area.width as usize),
        Style::default()
            .bg(theme.topbar_bg)
            .fg(wordmark)
            .add_modifier(Modifier::BOLD),
    );

    let doc = app.active_doc();
    let title = match doc {
        Some(d) => {
            let path = d.display_path(Some(&app.cwd));
            if d.is_modified() {
                format!("· {path} ●")
            } else {
                format!("· {path}")
            }
        }
        None => "· no buffer".to_string(),
    };
    let avail = area.width.saturating_sub(6) as usize;
    let shown = fit(&title, avail);
    buf.set_string(
        area.x + 6,
        area.y,
        &shown,
        Style::default().bg(theme.topbar_bg).fg(theme.topbar_fg),
    );
    // The modified marker is one character at the end of the title; giving it
    // the accent colour is a repaint of a single cell.
    if pixel && shown.ends_with('●') {
        buf.set_string(
            area.x + 6 + shown.chars().count() as u16 - 1,
            area.y,
            "●",
            Style::default().bg(theme.topbar_bg).fg(theme.mark),
        );
    }

    let right = format!(
        "{} buffer{}  v{} ",
        app.buffers.len(),
        if app.buffers.len() == 1 { "" } else { "s" },
        VERSION
    );
    let right_len = right.chars().count() as u16;
    if area.width > right_len + 8 {
        buf.set_string(
            area.x + area.width - right_len,
            area.y,
            right,
            Style::default().bg(theme.topbar_bg).fg(theme.dim),
        );
    }
}

/// One tab's hit box: where it starts, how wide it is, and which buffer it
/// selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabSpan {
    pub x: u16,
    pub width: u16,
    pub buffer: usize,
}

/// The label a tab shows, including its one-cell padding on each side.
fn tab_label(doc: &crate::buffer::Document) -> String {
    format!(
        " {}{} ",
        doc.display_name(),
        if doc.is_modified() { " ●" } else { "" }
    )
}

/// The column a tab spends on the hairline that separates it from the one
/// before it. Zero in the cell style, where tabs are told apart by background
/// alone and a `│` between them would be a whole cell of divider.
pub fn tab_gap(app: &App) -> u16 {
    match app.config.ui_style() {
        UiStyle::Pixel => 1,
        UiStyle::Cell => 0,
    }
}

/// Which columns each visible tab occupies.
///
/// Shared by the painter and by mouse hit-testing, so clicking a tab can never
/// disagree with where the tab was drawn. The separator column belongs to the
/// tab it precedes: a click on the hairline selects that tab, which is what a
/// pointer anywhere inside the tab's outline is expected to do.
pub fn tab_spans(app: &App, area: Rect) -> Vec<TabSpan> {
    let gap = tab_gap(app);
    let right = area.x + area.width;
    let mut x = area.x;
    let mut spans = Vec::new();
    for (buffer, doc) in app.buffers.iter().enumerate() {
        let lead = if buffer > 0 { gap } else { 0 };
        let width = tab_label(doc).chars().count() as u16 + lead;
        if x + width > right {
            break;
        }
        spans.push(TabSpan { x, width, buffer });
        x += width;
    }
    spans
}

/// Buffer tab strip.
///
/// One row, always. The active tab is told apart by weight, colour and a
/// hairline of accent under it — the whole point of the pixel style is that
/// none of those costs a row, so there is no second strip and no separator
/// glyph in the text.
pub fn draw_tabs(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let pixel = app.config.ui_style() == UiStyle::Pixel;
    fill(
        buf,
        area,
        Style::default()
            .bg(theme.tab_inactive_bg)
            .fg(theme.tab_inactive_fg),
    );
    if pixel {
        underline_rule(buf, area, theme.line);
    }

    let gap = tab_gap(app);
    let spans = tab_spans(app, area);
    let mut x = area.x;
    for span in &spans {
        let lead = if span.buffer > 0 { gap } else { 0 };
        if lead > 0 {
            buf.set_string(
                span.x,
                area.y,
                "▏",
                Style::default().fg(theme.line).bg(theme.tab_inactive_bg),
            );
        }
        let style = if span.buffer == app.active {
            let active = Style::default()
                .bg(theme.tab_active_bg)
                .fg(theme.tab_active_fg)
                .add_modifier(Modifier::BOLD);
            if pixel {
                active.underline_color(theme.mark)
            } else {
                active
            }
        } else {
            Style::default()
                .bg(theme.tab_inactive_bg)
                .fg(theme.tab_inactive_fg)
        };
        let label = tab_label(&app.buffers[span.buffer]);
        buf.set_string(span.x + lead, area.y, &label, style);
        x = span.x + span.width;
    }

    let hidden = app.buffers.len() - spans.len();
    if hidden > 0 && x + 4 <= area.x + area.width {
        buf.set_string(
            x,
            area.y,
            format!(" +{hidden} "),
            Style::default().fg(theme.dim).bg(theme.tab_inactive_bg),
        );
    }
}

/// Status bar: document facts on the left, cursor position on the right.
pub fn draw_status(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let pixel = app.config.ui_style() == UiStyle::Pixel;
    fill(
        buf,
        area,
        Style::default()
            .bg(theme.statusbar_bg)
            .fg(theme.statusbar_fg),
    );
    if pixel {
        underline_rule(buf, area, theme.line);
    }

    let Some(doc) = app.active_doc() else {
        buf.set_string(
            area.x + 1,
            area.y,
            "no buffer open — Ctrl+O to open a file",
            Style::default().fg(theme.dim),
        );
        return;
    };

    // Segments are separated by a hairline in the pixel style and by blank
    // space in the cell style, where a divider would cost a whole cell.
    let sep = if pixel { " ▏ " } else { "  " };
    let sep_colour = if pixel {
        theme.line
    } else {
        theme.statusbar_fg
    };
    let mut left: Vec<(String, Color)> = Vec::new();
    let language = doc.language().name().to_string();
    if doc.is_modified() {
        add_segment(
            &mut left,
            sep,
            sep_colour,
            format!("● {language}"),
            theme.mark,
        );
    } else {
        add_segment(&mut left, sep, sep_colour, language, theme.statusbar_fg);
    }
    add_segment(
        &mut left,
        sep,
        sep_colour,
        doc.eol().label().to_string(),
        theme.statusbar_fg,
    );
    add_segment(
        &mut left,
        sep,
        sep_colour,
        doc.encoding().label().to_string(),
        theme.statusbar_fg,
    );
    if doc.externally_modified() {
        add_segment(
            &mut left,
            sep,
            sep_colour,
            "⚠ changed on disk".to_string(),
            theme.warn,
        );
    }
    if !app.enhanced_keys {
        add_segment(&mut left, sep, sep_colour, "~keys".to_string(), theme.dim);
    }
    paint_segments(
        buf,
        area.x + 1,
        area.y,
        area.x + area.width,
        theme.statusbar_bg,
        &left,
    );

    let (line, col) = doc.cursor_line_col_1();
    let total = doc.line_count();
    let pct = if total <= 1 {
        100
    } else {
        ((line as f32 / total as f32) * 100.0).round() as usize
    };

    // The view can be scrolled away from the caret, and then the position
    // readout alone is misleading: it names a line the user cannot see. Say
    // where the caret actually is, in its own colour and with an arrow for the
    // direction to scroll back.
    //
    // With wrapping on, the last visible line is not `top + rows` — a single
    // line can fill the window — so it has to be measured the same way the
    // renderer laid the rows out.
    let cur_line = line.saturating_sub(1);
    let top = doc.scroll_top();
    let rows = doc.viewport_rows();
    let last_visible = doc.last_visible_line(app.wrap_width(), app.config.tab_width(), rows);
    let away = if cur_line < top {
        Some('↑')
    } else if cur_line > last_visible {
        Some('↓')
    } else {
        None
    };

    // Right-aligned, so it is built as spans from the left of its own block and
    // then placed as one unit.
    let gap = if pixel { " ▏ " } else { "   " };
    let tail = if pixel { " " } else { "   " };
    let mut spans: Vec<(String, Color)> = Vec::new();
    if app.search.active && !app.search.query.is_empty() {
        spans.push((
            format!("{}{gap}", app.search.counter()),
            theme.statusbar_accent,
        ));
    }
    if doc.caret_count() > 1 {
        spans.push((
            format!("{} cursors{gap}", doc.caret_count()),
            theme.statusbar_accent,
        ));
    }
    if let Some(arrow) = away {
        spans.push((format!("{arrow} Ln {line}{gap}"), theme.statusbar_away));
    }
    spans.push((
        format!("Ln {line}, Col {col}{gap}{pct}%{tail}"),
        theme.statusbar_accent,
    ));

    let width: u16 = spans
        .iter()
        .map(|(text, _)| text.chars().count() as u16)
        .sum();
    if width < area.width {
        let mut x = area.x + area.width - width;
        for (text, color) in &spans {
            buf.set_string(x, area.y, text, Style::default().fg(*color));
            x += text.chars().count() as u16;
        }
    }
}

/// Append a segment, preceded by the separator when it is not the first.
fn add_segment(
    segments: &mut Vec<(String, Color)>,
    sep: &str,
    sep_colour: Color,
    text: String,
    colour: Color,
) {
    if !segments.is_empty() {
        segments.push((sep.to_string(), sep_colour));
    }
    segments.push((text, colour));
}

/// Contextual shortcut hints. These follow the current mode so the bar always
/// teaches the next useful key instead of dumping the whole keymap.
pub fn draw_hints(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    fill(
        buf,
        area,
        Style::default().bg(theme.hint_bg).fg(theme.hint_fg),
    );

    let km = &app.keymap;
    let pairs: Vec<(String, &str)> = match app.ui.overlay.kind() {
        OverlayKind::None => vec![
            (km.label_for(Command::Save), "save"),
            (km.label_for(Command::Find), "find"),
            (km.label_for(Command::QuickOpen), "open"),
            (km.label_for(Command::CommandPalette), "commands"),
            (km.label_for(Command::Help), "help"),
        ],
        OverlayKind::Palette => vec![
            ("↑↓".into(), "select"),
            ("Enter".into(), "run"),
            ("Esc".into(), "cancel"),
        ],
        OverlayKind::Find | OverlayKind::Replace => vec![
            ("Enter".into(), "next"),
            ("Shift+Enter".into(), "previous"),
            ("Esc".into(), "close"),
        ],
        _ => vec![("Esc".into(), "cancel")],
    };

    if app.config.ui_style() == UiStyle::Cell {
        let mut spans: Vec<String> = Vec::new();
        for (key, label) in pairs {
            if key.is_empty() {
                continue;
            }
            spans.push(format!("{key} {label}"));
        }
        let text = format!(" {} ", spans.join("   ·   "));
        buf.set_string(
            area.x,
            area.y,
            fit(&text, area.width as usize),
            Style::default().fg(theme.hint_fg),
        );
        return;
    }

    // Pixel style: the key is the thing to press and the label only says what
    // it does, so the key keeps the brighter colour and the label drops back.
    let mut segments: Vec<(String, Color)> = vec![(" ".to_string(), theme.hint_fg)];
    let mut first = true;
    for (key, label) in pairs {
        if key.is_empty() {
            continue;
        }
        if !first {
            segments.push(("   ·   ".to_string(), theme.dim));
        }
        first = false;
        segments.push((key, theme.hint_fg));
        segments.push((format!(" {label}"), theme.dim));
    }
    segments.push((" ".to_string(), theme.hint_fg));
    paint_segments(
        buf,
        area.x,
        area.y,
        area.x + area.width,
        theme.hint_bg,
        &segments,
    );
}

/// Transient notifications, stacked above the hints bar.
pub fn draw_messages(buf: &mut Buffer, area: Rect, app: &App) {
    let messages = app.ui.visible_messages();
    if messages.is_empty() || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let max_w = area.width.saturating_sub(2).min(60) as usize;
    let mut y = area.y + area.height;
    for msg in messages.iter().rev().take(3) {
        if y == area.y {
            break;
        }
        y -= 1;
        let icon = match msg.level {
            Level::Info => "i",
            Level::Success => "✓",
            Level::Warn => "!",
            Level::Error => "✗",
        };
        let color = match msg.level {
            Level::Info => theme.info,
            Level::Success => theme.success,
            Level::Warn => theme.warn,
            Level::Error => theme.error,
        };
        let text = format!(" {icon} {} ", fit(&msg.text, max_w.saturating_sub(4)));
        let w = text.chars().count() as u16;
        if w >= area.width {
            continue;
        }
        let x = area.x + area.width - w - 1;
        let rect = Rect::new(x, y, w, 1);
        buf.set_style(rect, Style::default().bg(theme.overlay_bg).fg(color));
        buf.set_string(x, y, &text, Style::default().bg(theme.overlay_bg).fg(color));
    }
}

/// Which overlay is open; mirrored in [`crate::ui::overlays`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    None,
    Palette,
    Find,
    Replace,
    GotoLine,
    Open,
    SaveAs,
    Confirm,
    Help,
}

/// Draw a bordered panel and return its inner area.
///
/// Both styles return the same inner rect: the border always occupies the first
/// and last row and the first and last column, so a caller never has to know
/// which one is in use. Only what the border is *made of* changes — four
/// hairlines on the edges of those cells, or a full cell of box drawing.
pub fn panel(buf: &mut Buffer, area: Rect, title: &str, theme: &Theme, style: UiStyle) -> Rect {
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    if area.width < 4 || area.height < 3 {
        wipe(buf, area, theme.overlay_bg);
        return area;
    }

    match style {
        UiStyle::Pixel => {
            let mut canvas = Canvas::new(buf, area);
            canvas.fill(area, theme.overlay_bg);
            // The title claims its cells *before* the border is drawn, so the
            // top rule stops where the title starts and picks up again after
            // it. The result is a legend cut into the line rather than a label
            // sitting on top of one.
            if area.width > 6 {
                let t = fit(&format!(" {title} "), area.width as usize - 2);
                canvas.text(
                    area.x + 1,
                    area.y,
                    &t,
                    Style::default()
                        .fg(theme.overlay_title)
                        .bg(theme.overlay_bg)
                        .add_modifier(Modifier::BOLD),
                );
            }
            canvas.stroke(area, theme.border);
            canvas.flush();
        }
        UiStyle::Cell => {
            wipe(buf, area, theme.overlay_bg);
            buf.set_style(area, Style::default().fg(theme.fg));

            // The top edge is filled with `─` before the title is painted over
            // it. It used to be built from the title alone, which left the
            // top-right corner sitting just after the title instead of at the
            // panel's edge — a box whose lid stops a third of the way across.
            let w = area.width as usize;
            let inner_w = w.saturating_sub(2);
            let mut top = String::from("┌");
            top.push_str(&"─".repeat(inner_w));
            top.push('┐');
            buf.set_string(area.x, area.y, &top, Style::default().fg(theme.border));

            for y in 1..area.height.saturating_sub(1) {
                buf.set_string(area.x, area.y + y, "│", Style::default().fg(theme.border));
                buf.set_string(
                    area.x + area.width - 1,
                    area.y + y,
                    "│",
                    Style::default().fg(theme.border),
                );
            }
            let mut bottom = String::from("└");
            bottom.push_str(&"─".repeat(inner_w));
            bottom.push('┘');
            buf.set_string(
                area.x,
                area.y + area.height - 1,
                &bottom,
                Style::default().fg(theme.border),
            );

            // Title in accent colour, over the top border.
            if area.width > 6 {
                let t = fit(&format!(" {} ", title), inner_w);
                buf.set_string(
                    area.x + 1,
                    area.y,
                    &t,
                    Style::default()
                        .fg(theme.overlay_title)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    inner
}

/// Centre a rect of the given percentage size inside `area`.
pub fn centered(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let w = (area.width as u32 * width_pct as u32 / 100) as u16;
    let h = (area.height as u32 * height_pct as u32 / 100) as u16;
    let w = w.clamp(20, area.width).min(area.width);
    let h = h.clamp(3, area.height).min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Render text into a one-line prompt field, returning the cursor column.
pub fn draw_field(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    value: &str,
    cursor: usize,
    theme: &Theme,
    focused: bool,
) -> Option<u16> {
    if area.height == 0 {
        return None;
    }
    let label_style = Style::default().fg(theme.dim);
    buf.set_string(area.x, area.y, label, label_style);
    let x0 = area.x + label.chars().count() as u16;
    let w = (area.x + area.width).saturating_sub(x0) as usize;
    if w == 0 {
        return None;
    }

    let chars: Vec<char> = value.chars().collect();
    // Keep the cursor visible by scrolling the field horizontally.
    let mut offset = 0usize;
    if cursor >= w {
        offset = cursor + 1 - w;
    }
    let visible: String = chars.iter().skip(offset).take(w).collect();
    buf.set_string(x0, area.y, &visible, Style::default().fg(theme.fg));

    // Fill the remainder so the field looks like a field.
    let used = visible.chars().count() as u16;
    if used < w as u16 {
        let rest = Rect::new(x0 + used, area.y, w as u16 - used, 1);
        buf.set_style(rest, Style::default().bg(theme.overlay_bg));
    }

    if !focused {
        return None;
    }
    let cx = x0 + (cursor.saturating_sub(offset)) as u16;
    if cx < area.x + area.width {
        Some(cx)
    } else {
        None
    }
}

/// Convert a list of styled strings into `Line`s (used by overlays).
pub fn to_lines(items: &[String]) -> Vec<Line<'_>> {
    items.iter().map(|s| Line::raw(s.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_truncates_with_ellipsis() {
        assert_eq!(fit("abcdef", 10), "abcdef");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abcdef", 1), "…");
        assert_eq!(fit("abcdef", 0), "");
    }

    #[test]
    fn centered_clamps_to_area() {
        let area = Rect::new(0, 0, 20, 5);
        let r = centered(area, 80, 50);
        assert!(r.width <= area.width && r.height <= area.height);
        assert!(r.x + r.width <= area.right());
        assert!(r.y + r.height <= area.bottom());
    }
}
