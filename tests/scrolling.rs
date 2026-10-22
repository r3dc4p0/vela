//! Scrolling the view independently of the cursor.
//!
//! The renderer normally re-derives the first visible line from the cursor on
//! every frame, which makes it impossible to look anywhere else: the next draw
//! pulls the view back. These tests cover the escape hatch — a wheel gesture
//! detaches the view, the caret keeps its position (even off screen), and any
//! cursor movement re-attaches it.

mod common;

use common::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use vela::app::App;

/// One wheel notch. The column and row are irrelevant: the wheel scrolls the
/// viewport wherever the pointer happens to be.
fn wheel(app: &mut App, kind: MouseEventKind) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }));
}

fn wheel_down(app: &mut App, notches: usize) {
    for _ in 0..notches {
        wheel(app, MouseEventKind::ScrollDown);
    }
}

fn wheel_up(app: &mut App, notches: usize) {
    for _ in 0..notches {
        wheel(app, MouseEventKind::ScrollUp);
    }
}

/// `count` lines of `line 1`, `line 2`, …
fn numbered(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Open a fixture and render once, so the draw pass publishes the layout and
/// the buffer learns how many rows it has.
fn open_fixture(app: &mut App, path: &std::path::Path, width: u16, height: u16) -> Rect {
    open(app, path);
    let _ = screen(app, width, height);
    app.editor_area()
}

/// The last visible line of the editor area, as rendered.
fn last_editor_row(app: &mut App, area: Rect, width: u16, height: u16) -> String {
    let rows = screen(app, width, height);
    rows[(area.y + area.height - 1) as usize]
        .trim_start()
        .to_string()
}

/// The status bar row, found by its position readout.
fn status_row(app: &mut App, width: u16, height: u16) -> String {
    screen(app, width, height)
        .into_iter()
        .find(|row| row.contains(", Col "))
        .expect("the status bar always shows a position")
}

#[test]
fn the_wheel_travels_further_than_a_page_from_the_cursor() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);
    let page = area.height as usize;

    // Ten notches of three lines each. The old clamp kept the cursor within a
    // scroll-off margin of the top edge, so this is what used to be impossible.
    wheel_down(&mut app, 10);
    let rows = screen(&mut app, 80, 24);
    let top = app.buffers[0].scroll_top();

    assert_eq!(top, 30, "every notch is honoured");
    assert!(
        top > page,
        "the view left the cursor's page: {top} vs {page}"
    );
    assert_eq!(cursor(&app), 0, "scrolling never moves the cursor");
    assert!(
        rows[area.y as usize].trim_end().ends_with("line 31"),
        "the top row is line 31: {:?}",
        rows[area.y as usize]
    );
}

#[test]
fn the_status_bar_names_the_cursor_line_when_it_scrolls_away() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open_fixture(&mut app, &path, 80, 24);

    wheel_down(&mut app, 10);
    let status = status_row(&mut app, 80, 24);

    // The plain readout stays — it is still the cursor's position — and the
    // extra span says which way to scroll to see it.
    assert!(status.contains("Ln 1, Col 1"), "status bar: {status:?}");
    assert!(status.contains("↑ Ln 1"), "status bar: {status:?}");
}

#[test]
fn the_arrow_points_down_when_the_cursor_is_below_the_view() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open_fixture(&mut app, &path, 80, 24);

    // Jump to the end, then look back up past the caret.
    ctrl(&mut app, KeyCode::End);
    let _ = screen(&mut app, 80, 24);
    wheel_up(&mut app, 10);
    let status = status_row(&mut app, 80, 24);

    assert!(status.contains("↓ Ln 200"), "status bar: {status:?}");
}

#[test]
fn the_indicator_uses_its_own_colour() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);

    wheel_down(&mut app, 10);
    let buf = cells(&mut app, 80, 24);

    // Find the arrow in the status bar and compare it with the "Ln" of the
    // ordinary position readout a few columns later. The status bar sits
    // directly below the editor area.
    let y = area.y + area.height;
    let row: String = (0..80).map(|x| buf[(x, y)].symbol()).collect();
    let arrow_x = row.find('↑').expect("the indicator is on screen") as u16;
    let plain_x = row.rfind("Ln ").expect("the position readout") as u16;

    let away = buf[(arrow_x, y)].fg;
    let accent = buf[(plain_x, y)].fg;
    assert_ne!(
        away, accent,
        "the indicator must not look like a coordinate"
    );
    assert_eq!(away, app.theme.statusbar_away);
}

#[test]
fn scrolling_back_brings_the_cursor_on_screen_without_moving_it() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);

    wheel_down(&mut app, 10);
    wheel_up(&mut app, 10);
    let status = status_row(&mut app, 80, 24);

    assert_eq!(app.buffers[0].scroll_top(), 0);
    assert!(!status.contains('↑'), "nothing is out of view: {status:?}");
    assert!(
        screen(&mut app, 80, 24)[area.y as usize]
            .trim_end()
            .ends_with("line 1"),
        "the cursor's line is back at the top"
    );
}

#[test]
fn a_cursor_movement_brings_the_view_back() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);

    wheel_down(&mut app, 10);
    assert!(
        app.buffers[0].view_detached(),
        "the wheel detaches the view"
    );

    plain(&mut app, KeyCode::Down);
    assert!(
        !app.buffers[0].view_detached(),
        "a motion re-attaches the view"
    );

    let rows = screen(&mut app, 80, 24);
    assert_eq!(app.buffers[0].scroll_top(), 0, "and the view snaps back");
    assert!(
        rows[area.y as usize].trim_end().ends_with("line 1"),
        "row 0: {:?}",
        rows[area.y as usize]
    );
}

#[test]
fn typing_after_scrolling_away_goes_to_the_cursor() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);

    wheel_down(&mut app, 10);
    type_str(&mut app, "X");

    // The caret never moved, so the character lands on line 1 even though the
    // user is looking at line 31.
    assert!(
        text(&app).starts_with("Xline 1"),
        "text: {:?}",
        &text(&app)[..20]
    );
    assert!(!app.buffers[0].view_detached());
    let rows = screen(&mut app, 80, 24);
    assert_eq!(app.buffers[0].scroll_top(), 0);
    assert!(
        rows[area.y as usize].contains("Xline 1"),
        "row 0: {:?}",
        rows[area.y as usize]
    );
}

#[test]
fn the_wheel_stops_at_the_last_full_page() {
    let scratch = Scratch::new();
    let path = scratch.file("short.txt", &numbered(30));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);

    // Thirty lines in a window of `page` rows: the deepest useful top is
    // `30 - page`, so the screen is never filled with blank rows.
    wheel_down(&mut app, 20);
    let expected = 30usize.saturating_sub(area.height as usize);
    assert_eq!(app.buffers[0].scroll_top(), expected);
    // The row also carries the "opened file" notification, which floats over
    // the bottom of the editor, so only the left edge is the line itself.
    assert!(
        last_editor_row(&mut app, area, 80, 24).starts_with("30 line 30"),
        "the last line sits on the last row"
    );
}

#[test]
fn the_sideways_wheel_survives_the_next_draw() {
    let scratch = Scratch::new();
    let path = scratch.file("wide.txt", &"abcdefghijklmnopqrstuvwxyz".repeat(8));
    let mut app = editor(scratch.path());
    let area = open_fixture(&mut app, &path, 80, 24);

    wheel(&mut app, MouseEventKind::ScrollRight);
    wheel(&mut app, MouseEventKind::ScrollRight);
    let rows = screen(&mut app, 80, 24);

    assert_eq!(app.buffers[0].scroll_left(), 8);
    assert_eq!(cursor(&app), 0, "the cursor stays put horizontally too");
    // The fixture repeats, so the row must be checked from its left edge: the
    // gutter, then the text from column 8 onwards.
    assert!(
        rows[area.y as usize].trim_start().starts_with("1 ijklmnop"),
        "the row starts at column 8: {:?}",
        rows[area.y as usize]
    );
}

/// Render one frame and hand back the cell buffer, for colour assertions.
fn cells(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}
