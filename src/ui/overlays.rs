//! Overlays: command palette, find/replace bar, prompts, confirmations, help.
//!
//! Every overlay is drawn on top of the editor into the same buffer, so no
//! terminal-level tricks are needed and the editor stays visible underneath
//! (dimmed by the panel background).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use super::{App, Confirm, Overlay, Palette, PaletteMode, PathPrompt};
use crate::commands::{Category, Command};
use crate::keymap::KeyChord;
use crate::theme::Theme;

use super::chrome::{centered, draw_field, fit, panel};

/// Draw whichever overlay is open. Returns the cursor position, if any.
///
/// The overlay value is moved out of `App` for the duration of the call so the
/// renderer can hold `&mut Buffer` and `&mut Overlay` at once without unsafe
/// code or interior mutability.
pub fn draw(frame: &mut Frame, app: &mut App) -> Option<(u16, u16)> {
    let area = frame.area();
    let theme = app.theme.clone();
    let counter = app.search.matches.counter();
    let overlay = std::mem::replace(&mut app.ui.overlay, Overlay::None);
    let buf = frame.buffer_mut();

    let (result, restored) = match overlay {
        Overlay::None => (None, Overlay::None),
        Overlay::Palette(mut p) => {
            let r = draw_palette(buf, area, &theme, &mut p, &counter);
            (r, Overlay::Palette(p))
        }
        Overlay::Find(mut f) => {
            let r = draw_find_bar(buf, area, &theme, &mut f, &counter);
            (r, Overlay::Find(f))
        }
        Overlay::Replace(mut r) => {
            let c = draw_replace_bar(buf, area, &theme, &mut r, &counter);
            (c, Overlay::Replace(r))
        }
        Overlay::GotoLine(input) => {
            let value = input.value().to_string();
            let cursor = input.cursor();
            let r = draw_prompt(buf, area, &theme, " Go to line: ", &value, cursor, "");
            (r, Overlay::GotoLine(input))
        }
        Overlay::Open(mut p) => {
            let r = draw_path_prompt(buf, area, &theme, " Open file: ", &mut p);
            (r, Overlay::Open(p))
        }
        Overlay::SaveAs(mut p) => {
            let r = draw_path_prompt(buf, area, &theme, " Save as: ", &mut p);
            (r, Overlay::SaveAs(p))
        }
        Overlay::Confirm(c) => {
            let r = draw_confirm(buf, area, &theme, &c);
            (r, Overlay::Confirm(c))
        }
        Overlay::Help(mut h) => {
            let lines = help_lines(app);
            let r = draw_help(buf, area, &theme, &lines, &mut h);
            (r, Overlay::Help(h))
        }
    };

    app.ui.overlay = restored;
    result
}

fn draw_palette(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    palette: &mut Palette,
    _counter: &str,
) -> Option<(u16, u16)> {
    let rect = centered(area, 80, 60);
    let title = match palette.mode {
        PaletteMode::Commands => "Commands",
        PaletteMode::Files => "Open File",
    };
    let inner = panel(buf, rect, title, theme);
    if inner.height < 2 {
        return None;
    }

    let field_row = Rect::new(inner.x, inner.y, inner.width, 1);
    buf.set_style(field_row, Style::default().bg(theme.overlay_bg));
    let cursor = draw_field(
        buf,
        field_row,
        "› ",
        palette.input.value(),
        palette.input.cursor(),
        theme,
        true,
    );

    // Separator
    if inner.height >= 3 {
        buf.set_string(
            inner.x,
            inner.y + 1,
            "─".repeat(inner.width as usize),
            Style::default().fg(theme.border),
        );
    }

    let list_top = inner.y + 2;
    let list_h = inner.height.saturating_sub(3) as usize;
    if list_h == 0 {
        return cursor.map(|x| (x, field_row.y));
    }

    // Keep the selection visible.
    if palette.selected < palette.scroll {
        palette.scroll = palette.selected;
    } else if palette.selected >= palette.scroll + list_h {
        palette.scroll = palette.selected + 1 - list_h;
    }

    for row in 0..list_h {
        let idx = palette.scroll + row;
        if idx >= palette.items.len() {
            break;
        }
        let y = list_top + row as u16;
        let is_sel = idx == palette.selected;
        let style = if is_sel {
            Style::default()
                .bg(theme.overlay_sel_bg)
                .fg(theme.overlay_sel_fg)
        } else {
            Style::default().bg(theme.overlay_bg).fg(theme.fg)
        };
        buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);

        let item = &palette.items[idx];
        let w = inner.width as usize;
        let detail_w = item.detail.chars().count() + 1;
        let label_w = w.saturating_sub(detail_w + 1);
        let shown = fit(&item.label, label_w);
        buf.set_string(inner.x, y, &shown, style);
        if !item.detail.is_empty() && detail_w < w {
            buf.set_string(
                inner.x + (w - detail_w) as u16,
                y,
                fit(&item.detail, detail_w),
                if is_sel {
                    style
                } else {
                    Style::default().bg(theme.overlay_bg).fg(theme.dim)
                },
            );
        }
        // Highlight matched characters.
        for &pos in &item.positions {
            if pos >= shown.chars().count() {
                break;
            }
            let x = inner.x + pos as u16;
            if x >= inner.x + inner.width {
                break;
            }
            let ch = shown.chars().nth(pos).unwrap_or(' ');
            let hl = Style::default()
                .bg(if is_sel {
                    theme.overlay_sel_bg
                } else {
                    theme.overlay_bg
                })
                .fg(theme.overlay_title)
                .add_modifier(Modifier::BOLD);
            buf[(x, y)].set_symbol(&ch.to_string()).set_style(hl);
        }
    }

    // Footer count.
    let footer = format!(" {}/{} ", palette.selected + 1, palette.items.len());
    let y = inner.y + inner.height - 1;
    let fw = footer.chars().count() as u16;
    if fw < inner.width {
        buf.set_string(
            inner.x + inner.width - fw,
            y,
            &footer,
            Style::default().fg(theme.dim).bg(theme.overlay_bg),
        );
    }

    cursor.map(|x| (x, field_row.y))
}

fn draw_find_bar(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    bar: &mut super::FindBar,
    counter: &str,
) -> Option<(u16, u16)> {
    if area.height < 2 {
        return None;
    }
    let row = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    buf.set_style(row, Style::default().bg(theme.overlay_bg));
    let label = flags_label(" Find", bar.case_sensitive, bar.whole_word, bar.regex);
    let cx = draw_field(
        buf,
        row,
        &label,
        bar.input.value(),
        bar.input.cursor(),
        theme,
        true,
    );
    draw_counter(buf, row, theme, counter);
    cx.map(|x| (x, row.y))
}

fn draw_replace_bar(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    bar: &mut super::ReplaceBar,
    counter: &str,
) -> Option<(u16, u16)> {
    if area.height < 3 {
        return None;
    }
    let find_row = Rect::new(area.x, area.y + area.height - 2, area.width, 1);
    let rep_row = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    buf.set_style(find_row, Style::default().bg(theme.overlay_bg));
    buf.set_style(rep_row, Style::default().bg(theme.overlay_bg));

    let label = flags_label(" Find", bar.case_sensitive, bar.whole_word, bar.regex);
    let c1 = draw_field(
        buf,
        find_row,
        &label,
        bar.input.value(),
        bar.input.cursor(),
        theme,
        !bar.focus_replace,
    );
    draw_counter(buf, find_row, theme, counter);

    let c2 = draw_field(
        buf,
        rep_row,
        " Replace: ",
        bar.replace.value(),
        bar.replace.cursor(),
        theme,
        bar.focus_replace,
    );
    if bar.focus_replace {
        c2.map(|x| (x, rep_row.y))
    } else {
        c1.map(|x| (x, find_row.y))
    }
}

fn flags_label(prefix: &str, case_sensitive: bool, whole_word: bool, regex: bool) -> String {
    let mut flags = Vec::new();
    if case_sensitive {
        flags.push("Aa");
    }
    if whole_word {
        flags.push("W");
    }
    if regex {
        flags.push(".*");
    }
    if flags.is_empty() {
        format!("{prefix}: ")
    } else {
        format!("{prefix} [{}]: ", flags.join(" "))
    }
}

fn draw_counter(buf: &mut Buffer, row: Rect, theme: &Theme, counter: &str) {
    let cw = counter.chars().count() as u16 + 2;
    if cw < row.width {
        buf.set_string(
            row.x + row.width - cw,
            row.y,
            format!(" {counter} "),
            Style::default().fg(theme.dim).bg(theme.overlay_bg),
        );
    }
}

fn draw_prompt(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    label: &str,
    value: &str,
    cursor: usize,
    hint: &str,
) -> Option<(u16, u16)> {
    if area.height < 2 {
        return None;
    }
    let row = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    buf.set_style(row, Style::default().bg(theme.overlay_bg));
    let cx = draw_field(buf, row, label, value, cursor, theme, true);
    if !hint.is_empty() {
        let hw = hint.chars().count() as u16 + 1;
        if hw < row.width {
            buf.set_string(
                row.x + row.width - hw,
                row.y,
                hint,
                Style::default().fg(theme.dim).bg(theme.overlay_bg),
            );
        }
    }
    cx.map(|x| (x, row.y))
}

fn draw_path_prompt(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    label: &str,
    prompt: &mut PathPrompt,
) -> Option<(u16, u16)> {
    if area.height < 2 {
        return None;
    }
    // Completion list floats above the prompt.
    let visible = prompt.completion.len().min(6);
    if visible > 0 && area.height as usize > visible + 3 {
        let h = visible as u16 + 2;
        let w = area.width.min(60);
        let rect = Rect::new(area.x, area.y + area.height - 2 - h + 1, w, h);
        let inner = panel(buf, rect, "Completions", theme);
        for i in 0..visible {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }
            let is_sel = i == prompt.selected;
            let style = if is_sel {
                Style::default()
                    .bg(theme.overlay_sel_bg)
                    .fg(theme.overlay_sel_fg)
            } else {
                Style::default().bg(theme.overlay_bg).fg(theme.fg)
            };
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);
            buf.set_string(
                inner.x,
                y,
                fit(&prompt.completion[i], inner.width as usize),
                style,
            );
        }
    }

    let row = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    buf.set_style(row, Style::default().bg(theme.overlay_bg));
    let cx = draw_field(
        buf,
        row,
        label,
        prompt.input.value(),
        prompt.input.cursor(),
        theme,
        true,
    );
    cx.map(|x| (x, row.y))
}

fn draw_confirm(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    confirm: &Confirm,
) -> Option<(u16, u16)> {
    let w = area.width.clamp(20, 70);
    let h = (confirm.items.len() as u16 + 4).min(area.height);
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    );
    let inner = panel(buf, rect, "Confirm", theme);
    if inner.height < 2 {
        return None;
    }
    buf.set_string(
        inner.x,
        inner.y,
        fit(&confirm.prompt, inner.width as usize),
        Style::default().fg(theme.fg),
    );
    for (i, item) in confirm.items.iter().enumerate() {
        let y = inner.y + 2 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let is_sel = i == confirm.selected;
        let style = if is_sel {
            Style::default()
                .bg(theme.overlay_sel_bg)
                .fg(theme.overlay_sel_fg)
        } else {
            Style::default().bg(theme.overlay_bg).fg(theme.fg)
        };
        buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);
        buf.set_string(
            inner.x,
            y,
            fit(&format!("  {item}"), inner.width as usize),
            style,
        );
    }
    None
}

fn draw_help(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    lines: &[String],
    help: &mut super::HelpState,
) -> Option<(u16, u16)> {
    let rect = centered(area, 90, 85);
    let inner = panel(buf, rect, "Keyboard Shortcuts — Esc to close", theme);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    if help.scroll > max_scroll {
        help.scroll = max_scroll;
    }
    for row in 0..inner.height {
        let idx = help.scroll + row as usize;
        if idx >= lines.len() {
            break;
        }
        let y = inner.y + row;
        let text = &lines[idx];
        let style = if text.starts_with("    ") {
            Style::default().fg(theme.fg)
        } else if text.is_empty() {
            Style::default()
        } else {
            Style::default()
                .fg(theme.overlay_title)
                .add_modifier(Modifier::BOLD)
        };
        buf.set_string(
            inner.x,
            y,
            fit(text, inner.width as usize),
            style.bg(theme.overlay_bg),
        );
    }
    None
}

/// The help screen contents, generated from the command registry so it can never
/// drift from the actual keymap.
pub fn help_lines(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "Vela {} — {} — {} terminal",
        crate::VERSION,
        crate::platform::os_name(),
        crate::platform::term_name()
    ));
    if !app.enhanced_keys {
        out.push(String::new());
        out.push("  Note: this terminal does not report modifier keys precisely.".to_string());
        out.push("  Ctrl+Shift+<key> may arrive as Ctrl+<key>.".to_string());
        out.push("  Set [terminal] keyboard_enhancement = \"on\" to force the".to_string());
        out.push("  extended protocol if your terminal supports it.".to_string());
    }
    out.push(String::new());

    let categories = [
        Category::File,
        Category::Edit,
        Category::Navigate,
        Category::Search,
        Category::View,
        Category::Help,
    ];

    for cat in categories {
        let mut any = false;
        let mut section: Vec<String> = Vec::new();
        for cmd in Command::ALL.iter().copied() {
            if cmd.category() != cat {
                continue;
            }
            let chords = app.keymap.chords_for(cmd);
            if chords.is_empty() {
                continue;
            }
            if !any {
                section.push(format!("  {}", cat.label()));
                any = true;
            }
            let keys = chords
                .iter()
                .map(KeyChord::label)
                .collect::<Vec<_>>()
                .join(", ");
            section.push(format!("    {:<22} {}", keys, cmd.title()));
        }
        if any {
            out.extend(section);
            out.push(String::new());
        }
    }

    out.push("  Editing".to_string());
    out.push("    Tab                    Indent (or insert a tab stop)".to_string());
    out.push("    Shift+Tab              Outdent".to_string());
    out.push("    Enter                  New line with auto-indent".to_string());
    out.push("    Ctrl+Left / Ctrl+Right Move by word".to_string());
    out.push("    Shift+Arrows           Select".to_string());
    out.push("    Ctrl+Home / Ctrl+End   Buffer start / end".to_string());
    out.push(String::new());
    out.push("  Search bar".to_string());
    out.push("    Enter                  Next match".to_string());
    out.push("    Shift+Enter            Previous match".to_string());
    out.push("    Alt+C / Alt+W / Alt+R  Toggle case / whole word / regex".to_string());
    out.push("    Tab                    Switch to the replace field".to_string());
    out.push("    Ctrl+Enter             Replace current (replace mode)".to_string());
    out.push("    Alt+Enter              Replace all (replace mode)".to_string());
    out
}
