//! Window chrome: top bar, buffer tabs, status bar, shortcut hints, messages.
//!
//! These are pure functions of application state rendered into a `Buffer`; they
//! never mutate anything, which keeps layout bugs out of the editing path.
//!
//! Structure is carried by three things, in this order of preference:
//!
//! 1. **Surfaces.** Every bar is its own shade — the tab strip a step under the
//!    top bar, the hints a step under the status bar — and the active tab is
//!    the document's own surface, so it opens into the text. A boundary between
//!    two shades needs no line drawn on it.
//! 2. **Vertical hairlines.** A divider costs a column, not a row, so it is
//!    affordable: `▏` between tabs and in front of the line numbers.
//! 3. **Horizontal hairlines.** Only where they are free — inside a panel,
//!    whose first and last rows are border rows either way. A rule *between*
//!    two bars would have to either spend a whole row on it or underline the
//!    text above it, and both of those read as a defect rather than a border.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use crate::app::App;
use crate::commands::Command;
use crate::config::UiStyle;
use crate::theme::Theme;
use crate::ui::Level;
use crate::VERSION;

/// The handful of decisions the two drawing styles disagree on.
///
/// Everything else — layout, colours, the words on screen — is shared, because
/// the styles are two ways to paint one UI, not two UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deco {
    /// Hairline panel borders instead of a box of `┌─┐│└┘`.
    pub hairline_panels: bool,
    /// Paint a hint's key brighter than the label it explains.
    pub two_tone_hints: bool,
    /// A progress bar is a filled rectangle on a surface rather than a run of
    /// shaded `░`.
    pub solid_track: bool,
    /// The percentage in the status bar is a badge on its own background rather
    /// than another run of text.
    pub pill: bool,
}

impl Deco {
    pub fn of(style: UiStyle) -> Self {
        match style {
            UiStyle::Pixel => Deco {
                hairline_panels: true,
                two_tone_hints: true,
                solid_track: true,
                pill: true,
            },
            UiStyle::Cell => Deco {
                hairline_panels: false,
                two_tone_hints: false,
                solid_track: false,
                pill: false,
            },
        }
    }

    pub fn for_app(app: &App) -> Self {
        Self::of(app.config.ui_style())
    }
}

/// The hairline between two tabs, and the column it costs.
///
/// Both styles use it: a tab is a container and a container has an edge, and
/// the design draws that edge as a rule. Unlike the borders the two styles
/// disagree about, this one costs a column rather than a row, and `▏` is a
/// fraction of a column.
const TAB_DIVIDER: &str = "▏";
const TAB_GAP: u16 = 1;

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

/// Append a segment, preceded by the divider when it is not the first.
fn add_segment(
    segments: &mut Vec<(String, Color)>,
    divider: &str,
    divider_colour: Color,
    text: String,
    colour: Color,
) {
    if !segments.is_empty() {
        segments.push((divider.to_string(), divider_colour));
    }
    segments.push((text, colour));
}

/// Top bar: identity on the left, session facts on the right.
pub fn draw_topbar(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let deco = Deco::for_app(app);
    fill(
        buf,
        area,
        Style::default().bg(theme.topbar_bg).fg(theme.topbar_fg),
    );

    // The wordmark carries the accent colour of the design: the same gold as
    // the caret, the active tab's edge and the modified dot, so the identity
    // and the things that say "this one" read as one family.
    let wordmark = if deco.hairline_panels {
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
    // The modified marker is the last character of the title; giving it the
    // accent colour is a repaint of one cell.
    if deco.hairline_panels && shown.ends_with('●') {
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

/// Which columns each visible tab occupies.
///
/// Shared by the painter and by mouse hit-testing, so clicking a tab can never
/// disagree with where the tab was drawn. Every tab owns the divider column to
/// its left, including the first one, so the strip is framed and the active tab
/// has an edge to carry its accent. A click on a divider selects the tab it
/// belongs to, which is what a pointer inside the tab's outline should do.
pub fn tab_spans(app: &App, area: Rect) -> Vec<TabSpan> {
    let right = area.x + area.width;
    let mut x = area.x;
    let mut spans = Vec::new();
    for (buffer, doc) in app.buffers.iter().enumerate() {
        let width = tab_label(doc).chars().count() as u16 + TAB_GAP;
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
/// One row, always. The strip is a shade under the top bar, so the two are told
/// apart without a rule between them; the tabs are told apart by their dividers
/// and by the active one being the colour of the document it opens into.
pub fn draw_tabs(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    fill(
        buf,
        area,
        Style::default()
            .bg(theme.tab_inactive_bg)
            .fg(theme.tab_inactive_fg),
    );

    let spans = tab_spans(app, area);
    let mut x = area.x;
    for span in &spans {
        let active = span.buffer == app.active;
        buf.set_string(
            span.x,
            area.y,
            TAB_DIVIDER,
            Style::default()
                .fg(if active { theme.mark } else { theme.line })
                .bg(theme.tab_inactive_bg),
        );
        let style = if active {
            Style::default()
                .bg(theme.tab_active_bg)
                .fg(theme.tab_active_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(theme.tab_inactive_bg)
                .fg(theme.tab_inactive_fg)
        };
        let label = tab_label(&app.buffers[span.buffer]);
        buf.set_string(span.x + TAB_GAP, area.y, &label, style);
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
    let deco = Deco::for_app(app);
    fill(
        buf,
        area,
        Style::default()
            .bg(theme.statusbar_bg)
            .fg(theme.statusbar_fg),
    );

    let Some(doc) = app.active_doc() else {
        buf.set_string(
            area.x + 1,
            area.y,
            "no buffer open — Ctrl+O to open a file",
            Style::default().fg(theme.dim),
        );
        return;
    };

    // The design separates these segments with space rather than with a rule:
    // the only line in this bar is the badge at the end. `deco` is read here for
    // the badge alone, which is why the separators do not vary with it.
    let sep = "  ";
    let sep_colour = theme.statusbar_fg;
    let mut left: Vec<(String, Color)> = Vec::new();
    let language = doc.language().name().to_string();
    // The language is the one thing in this bar that says *what* is open rather
    // than *where*, so it carries the accent. The dot in front of it is the
    // only part that means "unsaved".
    if doc.is_modified() {
        add_segment(
            &mut left,
            sep,
            sep_colour,
            format!("● {language}"),
            theme.mark,
        );
    } else {
        add_segment(&mut left, sep, sep_colour, language, theme.mark);
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
    // then placed as one unit. The percentage is the only span with a
    // background: it is a badge, the way the design draws it.
    let gap = "   ";
    // The badge carries its own padding; a plain readout is followed by the
    // blank the bar has always ended with.
    let tail = if deco.pill { " " } else { "   " };
    let mut spans: Vec<(String, Style)> = Vec::new();
    if app.search.active && !app.search.query.is_empty() {
        spans.push((
            format!("{}{gap}", app.search.counter()),
            Style::default().fg(theme.statusbar_accent),
        ));
    }
    if doc.caret_count() > 1 {
        spans.push((
            format!("{} cursors{gap}", doc.caret_count()),
            Style::default().fg(theme.statusbar_accent),
        ));
    }
    if let Some(arrow) = away {
        spans.push((
            format!("{arrow} Ln {line}{gap}"),
            Style::default().fg(theme.statusbar_away),
        ));
    }
    spans.push((
        format!("Ln {line}, Col {col}{gap}"),
        Style::default().fg(theme.statusbar_accent),
    ));
    if deco.pill {
        spans.push((
            format!(" {pct}% {tail}"),
            Style::default()
                .fg(theme.statusbar_accent)
                .bg(theme.pill_bg),
        ));
    } else {
        spans.push((
            format!("{pct}%{tail}"),
            Style::default().fg(theme.statusbar_accent),
        ));
    }

    // The block is inset by one column on the right, so the badge at its end
    // does not touch the edge of the screen — the bar has a margin on both
    // sides, like the left-hand segments do.
    let edge = area.x + area.width.saturating_sub(1);
    let width: u16 = spans
        .iter()
        .map(|(text, _)| text.chars().count() as u16)
        .sum();
    if width < edge.saturating_sub(area.x) {
        let mut x = edge - width;
        for (text, style) in &spans {
            buf.set_string(x, area.y, text, *style);
            x += text.chars().count() as u16;
        }
    }
}

/// Contextual shortcut hints. These follow the current mode so the bar always
/// teaches the next useful key instead of dumping the whole keymap.
pub fn draw_hints(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let deco = Deco::for_app(app);
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
        _ => vec![("Esc".into(), "cancel")],
    };

    if !deco.two_tone_hints {
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

    // The key is the thing to press and the label only says what it does, so
    // the key keeps the brighter colour and the label drops back.
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
/// Both styles return the same inner rect — the border occupies the first and
/// last row and the first and last column either way — so a caller never has to
/// know which one is in use. Only what the border is *made of* changes.
pub fn panel(buf: &mut Buffer, area: Rect, title: &str, theme: &Theme, deco: Deco) -> Rect {
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

    if deco.hairline_panels {
        panel_hairline(buf, area, title, theme);
    } else {
        panel_box(buf, area, title, theme);
    }
    inner
}

/// A panel framed by four hairlines: the outermost eighth of the first and last
/// row, the leftmost and rightmost eighth of the columns between them.
///
/// This is the one place a horizontal rule is free: a panel already spends its
/// first and last row on the border, so drawing it thinner costs nothing. The
/// title is painted first and the rule skips its cells, which is what makes it
/// read as a legend cut into the line rather than a label sitting on top of one.
fn panel_hairline(buf: &mut Buffer, area: Rect, title: &str, theme: &Theme) {
    wipe(buf, area, theme.overlay_bg);

    let mut title_span = None;
    if area.width > 6 {
        let t = fit(&format!(" {title} "), area.width as usize - 2);
        let x = area.x + 1;
        buf.set_string(
            x,
            area.y,
            &t,
            Style::default()
                .fg(theme.overlay_title)
                .bg(theme.overlay_bg)
                .add_modifier(Modifier::BOLD),
        );
        title_span = Some((x, x + t.chars().count() as u16));
    }

    rule(
        buf,
        area.x,
        area.x + area.width,
        area.y,
        "▔",
        theme.border,
        title_span,
    );
    if area.height > 1 {
        rule(
            buf,
            area.x,
            area.x + area.width,
            area.y + area.height - 1,
            "▁",
            theme.border,
            None,
        );
    }
    // The side rules start below the top rule and stop above the bottom one, so
    // the corners belong to the horizontals. The seam that leaves is one dot.
    for y in area.y + 1..area.y + area.height.saturating_sub(1) {
        edge(buf, area.x, y, "▏", theme.border);
        edge(buf, area.x + area.width - 1, y, "▕", theme.border);
    }
}

/// Paint `glyph` across `x0..x1` on row `y`, leaving `skip` alone.
fn rule(
    buf: &mut Buffer,
    x0: u16,
    x1: u16,
    y: u16,
    glyph: &str,
    colour: Color,
    skip: Option<(u16, u16)>,
) {
    for x in x0..x1 {
        if let Some((lo, hi)) = skip {
            if x >= lo && x < hi {
                continue;
            }
        }
        edge(buf, x, y, glyph, colour);
    }
}

fn edge(buf: &mut Buffer, x: u16, y: u16, glyph: &str, colour: Color) {
    let cell = &mut buf[(x, y)];
    cell.set_symbol(glyph);
    cell.fg = colour;
}

/// A panel framed by a full cell of box drawing, with the title on the border.
fn panel_box(buf: &mut Buffer, area: Rect, title: &str, theme: &Theme) {
    wipe(buf, area, theme.overlay_bg);
    buf.set_style(area, Style::default().fg(theme.fg));

    // The top edge is filled with `─` before the title is painted over it. It
    // used to be built from the title alone, which left the top-right corner
    // sitting just after the title instead of at the panel's edge — a box whose
    // lid stops a third of the way across.
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

    #[test]
    fn the_styles_differ_in_a_handful_of_ways() {
        let pixel = Deco::of(UiStyle::Pixel);
        let cell = Deco::of(UiStyle::Cell);
        assert!(pixel.hairline_panels && !cell.hairline_panels);
        assert!(pixel.two_tone_hints && !cell.two_tone_hints);
        assert!(pixel.solid_track && !cell.solid_track);
    }

    #[test]
    fn a_hairline_panel_frames_itself_and_keeps_its_title() {
        let theme = Theme::dark();
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 4));
        panel(
            &mut buf,
            Rect::new(0, 0, 12, 4),
            "Hi",
            &theme,
            Deco::of(UiStyle::Pixel),
        );

        let row = |y: u16| -> String { (0..12).map(|x| buf[(x, y)].symbol()).collect() };
        // The corner keeps the rule, the title's padding is left alone, and the
        // rule picks up again where the title ends.
        assert_eq!(row(0), "▔ Hi ▔▔▔▔▔▔▔");
        assert_eq!(row(1), "▏          ▕");
        assert_eq!(row(3), "▁▁▁▁▁▁▁▁▁▁▁▁");
        // The frame is one dot thick: the cell keeps its background.
        assert_eq!(buf[(0, 0)].bg, theme.overlay_bg);
    }
}
