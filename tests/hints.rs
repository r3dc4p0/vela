//! The hint bar is a row of buttons.
//!
//! Each hint already taught a shortcut; these tests are about the other half —
//! that a click on one runs it, that the pointer lights up what it is over, and
//! that none of it can fire when the bar is not there.

mod common;

use common::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use vibe::app::App;
use vibe::commands::Command;

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn click(app: &mut App, column: u16, row: u16) {
    mouse(
        app,
        MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column,
        row,
    );
}

/// Click the hint whose label is `label`, in the bottom row of a rendered frame.
///
/// The column is taken from what was actually drawn rather than from the hit
/// list, so the test proves the two agree. `find` answers in *bytes* and the
/// bar separates its hints with `·`, which is two of them — so the offset is
/// converted to a display column before it is used as a coordinate.
fn click_label(app: &mut App, width: u16, height: u16, label: &str) {
    let rows = screen(app, width, height);
    let bar = &rows[height as usize - 1];
    let byte = bar
        .find(label)
        .unwrap_or_else(|| panic!("no {label:?} in the hint bar: {bar:?}"));
    let column = bar[..byte].chars().count() as u16;
    click(app, column, height - 1);
}

/// The rendered cells, for assertions about colour.
fn cells(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn clicking_save_saves_the_buffer() {
    let scratch = Scratch::new();
    let path = scratch.file("notes.txt", "one\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "two");
    assert!(modified(&app));

    click_label(&mut app, 100, 20, "save");

    assert!(!modified(&app), "the buffer was saved");
    // `Ctrl+End` puts the caret after the final newline, so the text lands on
    // a new last line.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "one\ntwo",
        "and the file on disk has the text"
    );
}

#[test]
fn clicking_commands_opens_the_palette() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    click_label(&mut app, 100, 20, "commands");
    assert!(
        matches!(app.ui.overlay, vibe::ui::Overlay::Palette(_)),
        "the command palette opened"
    );
}

#[test]
fn clicking_open_opens_the_file_picker() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    click_label(&mut app, 100, 20, "open");
    match &app.ui.overlay {
        vibe::ui::Overlay::Palette(p) => {
            assert_eq!(p.mode, vibe::ui::PaletteMode::Files, "the file picker")
        }
        other => panic!("expected the file picker, got {other:?}"),
    }
}

#[test]
fn clicking_help_opens_the_help_screen() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    click_label(&mut app, 100, 20, "help");
    assert!(matches!(app.ui.overlay, vibe::ui::Overlay::Help(_)));
}

#[test]
fn clicking_find_opens_the_find_bar() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    click_label(&mut app, 100, 20, "find");
    assert!(matches!(app.ui.overlay, vibe::ui::Overlay::Find(_)));
}

#[test]
fn a_click_on_the_empty_part_of_the_bar_does_nothing() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    screen(&mut app, 120, 20);
    // Past the last hint, still inside the bar.
    click(&mut app, 118, 19);
    assert!(app.ui.overlay.is_none(), "no overlay");
    assert_eq!(app.buffers.len(), 1, "and no file was opened");
}

#[test]
fn the_bar_is_not_part_of_the_document() {
    // The bar is the bottom row; a click there must not also place the cursor.
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "one\ntwo\nthree");
    ctrl(&mut app, KeyCode::Home);
    let before = cursor(&app);

    click_label(&mut app, 100, 20, "save");

    assert_eq!(cursor(&app), before, "the caret did not move");
}

#[test]
fn the_pointer_lights_up_the_hint_under_it() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let (width, height) = (100u16, 20u16);
    let rows = screen(&mut app, width, height);
    let column = rows[height as usize - 1].find("find").expect("a find hint") as u16;
    let theme = vibe::theme::Theme::dark();

    // Nothing hovered: the bar is its own flat shade.
    let before = cells(&mut app, width, height);
    assert_eq!(before[(column, height - 1)].bg, theme.hint_bg);

    mouse(&mut app, MouseEventKind::Moved, column, height - 1);
    assert_eq!(app.ui.hovered_hint, Some(1), "the second hint");

    let after = cells(&mut app, width, height);
    assert_eq!(
        after[(column, height - 1)].bg,
        theme.pill_bg,
        "the hint lifted onto a button background"
    );
    // And only that one: its neighbour is still flat.
    assert_eq!(after[(column + 12, height - 1)].bg, theme.hint_bg);
}

#[test]
fn the_light_goes_out_when_the_pointer_leaves() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let (width, height) = (100u16, 20u16);
    let rows = screen(&mut app, width, height);
    let column = rows[height as usize - 1].find("save").expect("a save hint") as u16;

    mouse(&mut app, MouseEventKind::Moved, column, height - 1);
    assert_eq!(app.ui.hovered_hint, Some(0));

    // Up into the text.
    mouse(&mut app, MouseEventKind::Moved, 10, 5);
    assert_eq!(app.ui.hovered_hint, None);

    let theme = vibe::theme::Theme::dark();
    let buf = cells(&mut app, width, height);
    assert_eq!(buf[(column, height - 1)].bg, theme.hint_bg);
}

#[test]
fn a_hidden_bar_has_no_buttons() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| c.ui.show_hints = false);
    // Draw once with the bar, then hide it: the buttons must go with it, or a
    // click on the status bar would press something that is not there.
    let mut shown = editor(scratch.path());
    screen(&mut shown, 100, 20);
    assert!(!shown.ui.hint_hits.is_empty());
    shown.execute(Command::ToggleHints);
    screen(&mut shown, 100, 20);
    assert!(
        shown.ui.hint_hits.is_empty(),
        "the buttons went with the bar"
    );

    let rows = screen(&mut app, 100, 20);
    assert!(
        !rows[19].contains("save"),
        "the bar is not drawn: {:?}",
        rows[19]
    );
    assert!(app.ui.hint_hits.is_empty(), "and there is nothing to press");

    // The row it would have occupied is the status bar, and a click there must
    // not fire anything.
    click(&mut app, 5, 19);
    assert!(app.ui.overlay.is_none());
}

#[test]
fn the_buttons_follow_the_text_when_the_bar_is_narrow() {
    // At forty columns the hints are cut off; a click at the right edge must
    // still land on whatever is drawn there, and never outside the screen.
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let (width, height) = (40u16, 16u16);
    screen(&mut app, width, height);

    for hit in &app.ui.hint_hits {
        assert!(
            hit.end <= width,
            "a hit ends inside the bar: {hit:?} in {width} columns"
        );
        assert!(hit.start < hit.end, "and is not empty: {hit:?}");
    }
    // The last visible hint is clipped, so its hit is shorter than its text.
    let last = app.ui.hint_hits.last().copied().expect("some hints");
    assert!(last.end - last.start < 20, "clipped: {last:?}");

    click(&mut app, last.start, last.row);
    assert!(
        !app.ui.overlay.is_none(),
        "clicking the clipped hint still presses it"
    );
}

#[test]
fn the_bar_offers_nothing_to_press_while_a_palette_is_open() {
    // The palette's own hints describe keys, not commands: there is no button
    // to press, because the overlay takes the pointer.
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.open_palette(vibe::ui::PaletteMode::Commands);
    screen(&mut app, 100, 20);
    assert!(
        app.ui.hint_hits.is_empty(),
        "no buttons while the palette is up"
    );
}

#[test]
fn clicking_close_in_the_diff_view_closes_it() {
    let scratch = Scratch::new();
    let old = scratch.file("old.txt", "one\ntwo\n");
    let new = scratch.file("new.txt", "one\nthree\n");
    let mut app = editor(scratch.path());
    open(&mut app, &old);
    open(&mut app, &new);
    assert!(app.start_diff(0, 1));

    click_label(&mut app, 100, 20, "close");
    assert!(!app.diff_open(), "the view closed");
}

#[test]
fn the_buttons_are_the_commands_the_hints_name() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    screen(&mut app, 100, 20);
    let commands: Vec<Command> = app.ui.hint_hits.iter().map(|h| h.command).collect();
    assert_eq!(
        commands,
        vec![
            Command::Save,
            Command::Find,
            Command::QuickOpen,
            Command::CommandPalette,
            Command::Help,
        ]
    );
}
