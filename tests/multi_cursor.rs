//! Multi-cursor behaviour, driven through the real input path.
//!
//! Every test sends key events the way a terminal would, so what is asserted is
//! the behaviour a user gets, not an internal helper: key chords resolve through
//! the keymap, edits go through `App`, and the cursor set is read back from the
//! document.

mod common;

use common::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

const CTRL_ALT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT);

fn ctrl_alt(app: &mut vela::app::App, code: KeyCode) {
    press(app, code, CTRL_ALT);
}

fn caret_positions(app: &vela::app::App) -> Vec<usize> {
    app.active_doc()
        .map(|d| d.carets().iter().map(|c| c.pos).collect())
        .unwrap_or_default()
}

fn caret_count(app: &vela::app::App) -> usize {
    app.active_doc().map(|d| d.caret_count()).unwrap_or(0)
}

/// Alt+Click at a buffer position, assuming an editor area of 80x20 at 0,0 and
/// a three-digit gutter.
fn alt_click(app: &mut vela::app::App, line: usize, col: usize) {
    app.set_editor_area(Rect::new(0, 0, 80, 20));
    let gutter = 4u16; // three digits + one space
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: gutter + col as u16,
        row: line as u16,
        modifiers: KeyModifiers::ALT,
    }));
}

#[test]
fn typing_reaches_every_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "aaa\nbbb\nccc");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    ctrl_alt(&mut app, KeyCode::Down);

    assert_eq!(caret_count(&app), 3, "two Alt+Up presses add two cursors");
    assert_eq!(caret_positions(&app), vec![0, 4, 8]);

    type_str(&mut app, ">");
    assert_eq!(text(&app), ">aaa\n>bbb\n>ccc");
    assert_eq!(caret_positions(&app), vec![1, 6, 11]);
}

#[test]
fn arrows_move_every_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "abc\ndef\nghi");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    ctrl_alt(&mut app, KeyCode::Down);

    plain(&mut app, KeyCode::Right);
    assert_eq!(caret_positions(&app), vec![1, 5, 9]);

    plain(&mut app, KeyCode::Down);
    // The bottom cursor is already on the last line, so it stays put and
    // merges with the one that moves down onto it.
    assert_eq!(caret_positions(&app), vec![5, 9]);
}

#[test]
fn enter_auto_indents_every_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    // The second line inherits the indentation from the first, so no spaces are
    // typed for it: the buffer really is "  one\n  two".
    type_str(&mut app, "  one");
    plain(&mut app, KeyCode::Enter);
    type_str(&mut app, "two");
    assert_eq!(text(&app), "  one\n  two");

    plain(&mut app, KeyCode::End);
    ctrl_alt(&mut app, KeyCode::Up);
    assert_eq!(caret_count(&app), 2);

    plain(&mut app, KeyCode::Enter);
    assert_eq!(text(&app), "  one\n  \n  two\n  ");
}

#[test]
fn escape_collapses_to_one_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "a\nb\nc");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    ctrl_alt(&mut app, KeyCode::Down);
    assert_eq!(caret_count(&app), 3);

    plain(&mut app, KeyCode::Esc);
    assert_eq!(caret_count(&app), 1);
    assert_eq!(text(&app), "a\nb\nc", "collapsing does not edit the buffer");
}

#[test]
fn next_occurrence_then_typing() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "foo bar foo baz foo");
    ctrl(&mut app, KeyCode::Home);

    ctrl(&mut app, 'd');
    assert_eq!(caret_count(&app), 2);
    assert_eq!(
        app.active_doc().unwrap().selection(),
        Some(0..3),
        "the first press selects the word under the cursor"
    );

    ctrl(&mut app, 'd');
    assert_eq!(caret_count(&app), 3);

    ctrl(&mut app, 'd');
    assert_eq!(caret_count(&app), 3, "no further occurrence, no new cursor");

    type_str(&mut app, "X");
    assert_eq!(text(&app), "X bar X baz X");
}

#[test]
fn next_occurrence_keeps_stacked_cursors() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "foo bar foo\nfoo bar foo");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    assert_eq!(caret_count(&app), 2);

    // Selecting the word under the primary cursor must not throw the stacked
    // cursors away.
    ctrl(&mut app, 'd');
    assert_eq!(caret_count(&app), 3);
    assert_eq!(app.active_doc().unwrap().selection(), Some(0..3));
    assert_eq!(
        caret_positions(&app),
        vec![3, 12, 23],
        "the new caret sits on the match on the second line"
    );
}

#[test]
fn undo_restores_the_text_and_the_cursors() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "aaa\nbbb");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    type_str(&mut app, ">");
    assert_eq!(text(&app), ">aaa\n>bbb");

    ctrl(&mut app, 'z');
    assert_eq!(text(&app), "aaa\nbbb");
    assert_eq!(caret_count(&app), 2, "the cursors come back with the text");

    ctrl(&mut app, 'y');
    assert_eq!(text(&app), ">aaa\n>bbb");
    assert_eq!(caret_count(&app), 2);
}

#[test]
fn alt_click_adds_and_removes_a_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "aaa\nbbb\nccc");
    assert_eq!(caret_count(&app), 1);

    alt_click(&mut app, 1, 1);
    assert_eq!(caret_count(&app), 2);
    assert_eq!(caret_positions(&app), vec![5, 11]);

    type_str(&mut app, "Z");
    assert_eq!(text(&app), "aaa\nbZbb\ncccZ");

    // The caret that was added at column 1 has moved to column 2; clicking it
    // again removes it.
    alt_click(&mut app, 1, 2);
    assert_eq!(caret_count(&app), 1);
}

#[test]
fn alt_click_on_the_primary_cursor_is_ignored() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "abc");
    plain(&mut app, KeyCode::Home);
    alt_click(&mut app, 0, 0);
    assert_eq!(caret_count(&app), 1, "the last cursor is never removed");
}

#[test]
fn copy_collects_every_selection() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "foo bar foo");
    ctrl(&mut app, KeyCode::Home);
    ctrl(&mut app, 'd');

    ctrl(&mut app, 'c');
    assert_eq!(
        app.clipboard.get().as_deref(),
        Some("foo\nfoo"),
        "one selection per line"
    );
}

#[test]
fn status_bar_reports_the_cursor_count() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "aaa\nbbb\nccc");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    ctrl_alt(&mut app, KeyCode::Down);

    // Look at the status bar specifically: the transient notification also
    // mentions the cursor count and would mask the difference.
    let rows = screen(&mut app, 100, 12);
    let status = rows
        .iter()
        .find(|row| row.contains("Ln "))
        .expect("status bar")
        .clone();
    assert!(status.contains("3 cursors"), "status bar: {status}");

    plain(&mut app, KeyCode::Esc);
    let rows = screen(&mut app, 100, 12);
    let status = rows
        .iter()
        .find(|row| row.contains("Ln "))
        .expect("status bar")
        .clone();
    assert!(!status.contains("cursors"), "status bar: {status}");
}

#[test]
fn secondary_cursors_are_painted_as_blocks() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "aaa\nbbb\nccc");
    ctrl(&mut app, KeyCode::Home);
    ctrl_alt(&mut app, KeyCode::Down);
    ctrl_alt(&mut app, KeyCode::Down);

    let cursor_bg = app.theme.cursor_bg;
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();

    let mut blocks = 0;
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if buffer[(x, y)].bg == cursor_bg {
                blocks += 1;
            }
        }
    }
    // The primary cursor is drawn by the terminal itself, so only the two
    // secondary carets are painted as blocks.
    assert_eq!(blocks, 2, "one block per secondary cursor");
}
