//! Word wrap, driven through the real UI.
//!
//! Every test renders a frame and asserts on what was painted, because the
//! interesting failures here are disagreements between the parts: the renderer,
//! the motions and the mouse mapping each decide where a row ends, and they only
//! agree if they all read the same wrap.

mod common;

use common::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use vela::commands::Command;

/// Columns taken by the line-number gutter: three digits plus one space.
const GUTTER: usize = 4;

/// A line long enough to need several rows at the widths used below.
const LONG: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima";

fn editor_with_wrap(dir: &std::path::Path, wrap: bool) -> vela::app::App {
    editor_with(dir, |c| c.editor.word_wrap = wrap)
}

/// The rows the editor area was given, gutter included.
fn editor_rows(app: &mut vela::app::App, width: u16, height: u16) -> Vec<String> {
    let rows = screen(app, width, height);
    let area = app.editor_area();
    (area.y..area.y + area.height)
        .map(|y| rows[y as usize].clone())
        .collect()
}

/// The editor rows with the gutter removed and trailing blanks trimmed.
fn body_rows(app: &mut vela::app::App, width: u16, height: u16) -> Vec<String> {
    editor_rows(app, width, height)
        .into_iter()
        .map(|row| {
            let text: String = row.chars().skip(GUTTER).collect();
            text.trim_end().to_string()
        })
        .collect()
}

/// Render one frame and hand back the cell buffer, for assertions that need
/// colours rather than text.
fn cells(app: &mut vela::app::App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn send(app: &mut vela::app::App, kind: MouseEventKind, column: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn a_long_line_is_painted_over_several_rows() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    type_str(&mut app, LONG);

    let rows = body_rows(&mut app, 40, 20);
    let painted: Vec<&str> = rows
        .iter()
        .map(String::as_str)
        .filter(|r| !r.trim().is_empty())
        .collect();
    assert!(
        painted.len() > 2,
        "a 72-column line in a 36-column window needs several rows: {painted:?}"
    );

    // No word may be lost or split: wrapping happens between words.
    let joined = painted.join(" ");
    for word in LONG.split(' ') {
        assert!(joined.contains(word), "{word} missing from {painted:?}");
    }

    // And nothing overflows the text area.
    let area = app.editor_area();
    for row in &painted {
        assert!(
            row.chars().count() <= area.width as usize - GUTTER,
            "row wider than the editor: {row:?}"
        );
    }
}

#[test]
fn the_line_number_is_painted_once_per_line() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    type_str(&mut app, LONG);

    let rows = editor_rows(&mut app, 40, 20);
    let numbered = rows
        .iter()
        .filter(|r| r.chars().take(GUTTER).collect::<String>().contains('1'))
        .count();
    assert_eq!(
        numbered, 1,
        "a wrapped line is still one line, so it gets one number: {rows:?}"
    );
}

#[test]
fn wrapping_off_truncates_the_line_instead() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), false);
    type_str(&mut app, LONG);
    // Typing leaves the caret at the end, which scrolls the view right to keep
    // it visible. Start from the top of the line so the tail really is hidden.
    ctrl(&mut app, KeyCode::Home);

    let rows = body_rows(&mut app, 40, 20);
    assert!(
        !rows.iter().any(|r| r.contains("lima")),
        "without wrapping the tail is off screen: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains('›')),
        "and the truncation marker says so: {rows:?}"
    );
}

#[test]
fn the_toggle_command_switches_modes_and_repaints() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), false);
    type_str(&mut app, LONG);
    ctrl(&mut app, KeyCode::Home);
    assert!(!body_rows(&mut app, 40, 20)
        .iter()
        .any(|r| r.contains("lima")));

    app.execute(Command::ToggleWordWrap);
    assert!(app.config.editor.word_wrap);
    assert!(
        body_rows(&mut app, 40, 20)
            .iter()
            .any(|r| r.contains("lima")),
        "the same text now wraps onto the following rows"
    );

    app.execute(Command::ToggleWordWrap);
    assert!(!app.config.editor.word_wrap);
    assert!(
        !body_rows(&mut app, 40, 20)
            .iter()
            .any(|r| r.contains("lima")),
        "and the tail goes off screen again"
    );
}

#[test]
fn the_default_binding_toggles_wrapping() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    alt(&mut app, KeyCode::Char('w'));
    assert!(app.config.editor.word_wrap, "alt+w turns wrapping on");
    alt(&mut app, KeyCode::Char('w'));
    assert!(!app.config.editor.word_wrap);
}

#[test]
fn the_view_follows_the_cursor_down_a_wrapped_line() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    // Three copies, so the single line is taller than the five-row editor.
    type_str(&mut app, &format!("{LONG} {LONG} {LONG}"));
    ctrl(&mut app, KeyCode::Home);
    let _ = screen(&mut app, 40, 8);

    // Walk down the wrapped rows until the caret has to leave the window.
    for _ in 0..6 {
        plain(&mut app, KeyCode::Down);
    }
    // The draw pass is what moves the viewport, so render before asserting on
    // it — a motion only moves the cursor.
    let _ = screen(&mut app, 40, 8);

    let area = app.editor_area();
    let doc = &app.buffers[0];
    let (line, off) = doc.cursor_line_col();
    assert_eq!(line, 0, "still on the one long line");
    assert!(
        doc.scroll_row() > 0,
        "the viewport moved down inside the line rather than jumping to another line"
    );

    // The caret is on screen: its row is between the viewport top and bottom.
    let width = area.width as usize - GUTTER;
    let layout = vela::buffer::wrap::LineLayout::new(&doc.line_text(line), width, 4);
    let row = layout.row_of(off);
    assert!(
        row >= doc.scroll_row() && row < doc.scroll_row() + area.height as usize,
        "caret row {row} is outside the window starting at {}",
        doc.scroll_row()
    );
}

#[test]
fn the_wheel_moves_by_rows_when_wrapped() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    // A window six rows tall leaves three editor rows, and the line needs far
    // more than that, so there is somewhere to scroll to.
    type_str(
        &mut app,
        &format!("{LONG} {LONG} {LONG} {LONG} {LONG} {LONG}"),
    );
    ctrl(&mut app, KeyCode::Home);
    let _ = screen(&mut app, 40, 6);

    send(&mut app, MouseEventKind::ScrollDown, 0, 0);
    assert_eq!(
        (app.buffers[0].scroll_top(), app.buffers[0].scroll_row()),
        (0, 3),
        "three rows down, which is still the first line"
    );
    // The second notch continues from where the first stopped: a viewport that
    // forgot its sub-row would restart at the top of the line and stay at 3.
    send(&mut app, MouseEventKind::ScrollDown, 0, 0);
    assert_eq!(
        (app.buffers[0].scroll_top(), app.buffers[0].scroll_row()),
        (0, 6),
        "the next three rows, not the same three again"
    );
}

#[test]
fn the_wheel_does_not_scroll_sideways_when_wrapped() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    type_str(&mut app, LONG);
    let _ = screen(&mut app, 40, 20);

    send(&mut app, MouseEventKind::ScrollRight, 0, 0);
    assert_eq!(
        app.buffers[0].scroll_left(),
        0,
        "a wrapped view has no horizontal offset to move"
    );
}

#[test]
fn clicking_a_continuation_row_places_the_cursor_on_that_character() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    type_str(&mut app, LONG);
    let rows = body_rows(&mut app, 40, 20);
    let area = app.editor_area();

    // Click a character on the second painted row and check that the cursor
    // landed on exactly that character — the row-to-buffer mapping and the
    // painter have to agree for this to hold.
    let row = 1usize;
    let col = 4usize;
    let painted = rows[row].chars().nth(col).expect("a character there");
    assert!(
        !painted.is_whitespace(),
        "pick a visible character: {rows:?}"
    );

    send(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        area.x + GUTTER as u16 + col as u16,
        area.y + row as u16,
    );
    let doc = &app.buffers[0];
    let (line, offset) = doc.cursor_line_col();
    assert_eq!(line, 0);
    let hit = doc.line_text(0).chars().nth(offset).expect("a character");
    assert_eq!(
        hit, painted,
        "the cursor is on the character that was clicked"
    );
}

#[test]
fn a_drag_along_a_wrapped_line_selects_contiguously() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    type_str(&mut app, LONG);
    let _ = screen(&mut app, 40, 20);
    let area = app.editor_area();

    let x = area.x + GUTTER as u16 + 2;
    send(&mut app, MouseEventKind::Down(MouseButton::Left), x, area.y);
    send(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        x,
        area.y + 1,
    );
    let doc = &app.buffers[0];
    let sel = doc.selection().expect("a selection from the drag");
    assert_eq!(sel.start, 2, "the drag started at the clicked column");
    assert!(
        sel.end > 31,
        "dragging into the next row selects past the end of the first row, \
         which is 31 columns wide: {sel:?}"
    );
    let selected = doc.selected_text();
    assert_eq!(
        selected,
        LONG[sel.start..sel.end],
        "the selection is one contiguous run of the line, across the wrap"
    );
}

#[test]
fn toggling_wrap_clears_a_stale_horizontal_offset() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), false);
    type_str(&mut app, LONG);
    let _ = screen(&mut app, 40, 20);
    send(&mut app, MouseEventKind::ScrollRight, 0, 0);
    assert!(app.buffers[0].scroll_left() > 0, "scrolled sideways first");

    app.execute(Command::ToggleWordWrap);
    let _ = screen(&mut app, 40, 20);
    assert_eq!(
        app.buffers[0].scroll_left(),
        0,
        "a wrapped view starts at column 0"
    );
}

#[test]
fn a_wrapped_file_still_reports_the_caret_position() {
    let scratch = Scratch::new();
    let mut app = editor_with_wrap(scratch.path(), true);
    type_str(&mut app, LONG);
    let rows = screen(&mut app, 40, 20);
    let status = rows
        .iter()
        .find(|row| row.contains("Ln "))
        .expect("status bar");
    let expected = format!("Ln 1, Col {}", LONG.chars().count() + 1);
    assert!(
        status.contains(&expected),
        "wrapping is a display concern and must not change the position readout: {status}"
    );
}

#[test]
fn a_token_that_spans_the_wrap_keeps_its_colour_on_both_rows() {
    let scratch = Scratch::new();
    let path = scratch.file(
        "sample.rs",
        "fn main() {\n    // a comment long enough that it has to be wrapped\n}\n",
    );
    let mut app = editor_with_wrap(scratch.path(), true);
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::Home);

    let buf = cells(&mut app, 44, 12);
    let area = app.editor_area();
    let comment = app.theme.syntax.comment;

    // The comment starts on the first row of its line and continues onto the
    // next. Both rows must be painted in the comment colour: the lexer has to
    // run over the whole line and each row take its clipped share, rather than
    // lexing a row in isolation and starting it in the default state.
    for row in 1..3u16 {
        let painted: Vec<&str> = (area.x + GUTTER as u16..area.x + area.width)
            .filter(|&x| buf[(x, area.y + row)].symbol() != " ")
            .map(|x| buf[(x, area.y + row)].symbol())
            .collect();
        assert!(!painted.is_empty(), "row {row} has text");
        let coloured = (area.x + GUTTER as u16..area.x + area.width).any(|x| {
            buf[(x, area.y + row)].symbol() != " " && buf[(x, area.y + row)].fg == comment
        });
        assert!(
            coloured,
            "row {row} is not painted as a comment: {:?}",
            painted
        );
    }
}
