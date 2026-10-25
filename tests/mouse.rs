//! Mouse behaviour, driven through the real input path.
//!
//! Each test renders a frame first, because the draw pass is what publishes the
//! editor and tab rects; the assertions then click on coordinates derived from
//! that layout rather than from hard-coded guesses.

mod common;

use common::*;
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Columns taken by the line-number gutter.
///
/// Asked of the same function the renderer uses, so a change to the gutter —
/// a wider column count, a divider column — moves the clicks with it instead of
/// leaving every test pointing at the wrong cell.
fn gutter(app: &vela::app::App) -> u16 {
    let doc = app.active_doc().expect("a buffer");
    vela::ui::editor::gutter_width(doc.line_count(), &app.config)
}

/// Render a frame and hand back the rects the draw pass published.
fn frame(app: &mut vela::app::App, width: u16, height: u16) -> (Rect, Rect) {
    let _ = screen(app, width, height);
    (app.editor_area(), app.tabs_area())
}

fn send(app: &mut vela::app::App, kind: MouseEventKind, column: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

/// A press without the release, so a drag can follow.
fn press(app: &mut vela::app::App, column: u16, row: u16) {
    send(app, MouseEventKind::Down(MouseButton::Left), column, row);
}

/// A full press-and-release at one cell.
fn click(app: &mut vela::app::App, column: u16, row: u16) {
    press(app, column, row);
    release(app, column, row);
}

fn drag(app: &mut vela::app::App, column: u16, row: u16) {
    send(app, MouseEventKind::Drag(MouseButton::Left), column, row);
}

fn release(app: &mut vela::app::App, column: u16, row: u16) {
    send(app, MouseEventKind::Up(MouseButton::Left), column, row);
}

#[test]
fn click_places_the_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "hello world");
    let (editor, _) = frame(&mut app, 80, 24);
    let g = gutter(&app);
    assert!(editor.height > 0, "the draw pass publishes the editor rect");

    click(&mut app, editor.x + g + 6, editor.y);
    assert_eq!(app.buffers[0].cursor(), 6);
}

#[test]
fn clicking_the_gutter_leaves_the_cursor_alone() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "hello");
    let (editor, _) = frame(&mut app, 80, 24);
    let before = app.buffers[0].cursor();

    click(&mut app, editor.x + 1, editor.y);
    assert_eq!(
        app.buffers[0].cursor(),
        before,
        "the gutter is not text, so a click there does nothing"
    );
}

#[test]
fn dragging_selects_across_lines() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abc\ndef\nghi");
    let (editor, _) = frame(&mut app, 80, 24);
    let g = gutter(&app);

    press(&mut app, editor.x + g + 1, editor.y);
    drag(&mut app, editor.x + g + 2, editor.y + 2);
    release(&mut app, editor.x + g + 2, editor.y + 2);

    let doc = &app.buffers[0];
    assert_eq!(doc.selection(), Some(1..10), "from `b` down to `i`");
    assert_eq!(doc.cursor(), 10, "the cursor follows the pointer");
    assert_eq!(doc.anchor(), Some(1), "the anchor stays at the press");
}

#[test]
fn dragging_upwards_keeps_the_cursor_on_the_upper_edge() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abc\ndef\nghi");
    let (editor, _) = frame(&mut app, 80, 24);
    let g = gutter(&app);

    press(&mut app, editor.x + g + 2, editor.y + 2);
    drag(&mut app, editor.x + g + 1, editor.y);
    release(&mut app, editor.x + g + 1, editor.y);

    let doc = &app.buffers[0];
    assert_eq!(doc.selection(), Some(1..10));
    assert_eq!(
        doc.cursor(),
        1,
        "a backwards drag leaves the cursor on the upper edge, so the next \
         arrow key moves the way the pointer did"
    );
}

#[test]
fn dragging_past_the_bottom_edge_autoscrolls() {
    let scratch = Scratch::new();
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let path = scratch.file("long.txt", &text);
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let (editor, _) = frame(&mut app, 80, 12);
    let g = gutter(&app);

    let before = app.buffers[app.active()].scroll_top();
    press(&mut app, editor.x + g, editor.y);
    drag(&mut app, editor.x + g, editor.y + editor.height + 5);

    let doc = &app.buffers[app.active()];
    assert!(
        doc.scroll_top() > before,
        "dragging below the text scrolls the buffer"
    );
    let selection = doc.selection().expect("the drag keeps a selection");
    assert!(selection.end > selection.start);
}

#[test]
fn double_click_selects_a_word_and_triple_click_the_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "foo bar\nbaz qux");
    let (editor, _) = frame(&mut app, 80, 24);
    let g = gutter(&app);
    let column = editor.x + g + 1;

    click(&mut app, column, editor.y);
    assert_eq!(app.buffers[0].selection(), None, "one click just places it");

    click(&mut app, column, editor.y);
    assert_eq!(
        app.buffers[0].selection(),
        Some(0..3),
        "the second click selects the word"
    );

    click(&mut app, column, editor.y);
    assert_eq!(
        app.buffers[0].selection(),
        Some(0..7),
        "the third click selects the whole line"
    );
}

#[test]
fn wheel_scrolls_the_buffer() {
    let scratch = Scratch::new();
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let path = scratch.file("long.txt", &text);
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = frame(&mut app, 80, 12);

    send(&mut app, MouseEventKind::ScrollDown, 10, 5);
    assert_eq!(app.buffers[app.active()].scroll_top(), 3);
    send(&mut app, MouseEventKind::ScrollUp, 10, 5);
    assert_eq!(app.buffers[app.active()].scroll_top(), 0);
}

#[test]
fn clicking_a_tab_switches_buffers() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    assert_eq!(app.active(), 1, "the last opened buffer is active");

    let (_, tabs) = frame(&mut app, 80, 24);
    assert!(tabs.height > 0, "tabs are shown for two buffers");
    let spans = vela::ui::chrome::tab_spans(&app, tabs);
    assert_eq!(spans.len(), 2);

    click(&mut app, spans[0].x + 1, tabs.y);
    assert_eq!(app.active(), 0, "clicking the first tab focuses it");

    click(&mut app, spans[1].x + 1, tabs.y);
    assert_eq!(app.active(), 1, "clicking the second tab focuses it");
}

#[test]
fn clicking_a_tab_does_not_move_the_cursor() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "hello\n");
    let b = scratch.file("b.txt", "world\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);

    let (_, tabs) = frame(&mut app, 80, 24);
    let spans = vela::ui::chrome::tab_spans(&app, tabs);
    let before = app.buffers[1].cursor();

    click(&mut app, spans[1].x + 1, tabs.y);
    assert_eq!(
        app.buffers[1].cursor(),
        before,
        "a press on the tab strip must not be treated as a click in the text"
    );
}

#[test]
fn mouse_can_be_disabled() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| c.ui.mouse = false);
    type_str(&mut app, "hello");
    let (editor, _) = frame(&mut app, 80, 24);
    let g = gutter(&app);
    let before = app.buffers[0].cursor();

    click(&mut app, editor.x + g, editor.y);
    drag(&mut app, editor.x + g + 3, editor.y);
    assert_eq!(app.buffers[0].cursor(), before);
}
