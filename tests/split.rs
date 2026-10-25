//! Splitting the editing area into panes.
//!
//! A pane is a view, not a buffer: two panes can show one file at two different
//! places, and an edit in either shows up in both. These tests drive the real
//! application — keys, mouse, the palette — and assert on what was painted,
//! because the interesting failures are disagreements between the layout, the
//! painter and the coordinate mapping.

mod common;

use common::*;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use vela::commands::Command;

fn numbered(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split the active pane with the real key, so the binding is covered too.
fn split(app: &mut vela::app::App) {
    press(app, KeyCode::Char('\\'), KeyModifiers::CONTROL);
}

/// One pane's slice of every rendered row.
fn pane_rows(app: &mut vela::app::App, width: u16, height: u16, pane: usize) -> Vec<String> {
    let rows = screen(app, width, height);
    let rect = app.panes[pane].rect;
    rows.iter()
        .skip(rect.y as usize)
        .take(rect.height as usize)
        .map(|row| {
            row.chars()
                .skip(rect.x as usize)
                .take(rect.width as usize)
                .collect()
        })
        .collect()
}

fn click(app: &mut vela::app::App, column: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn wheel(app: &mut vela::app::App, kind: MouseEventKind) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn the_split_key_gives_two_panes_and_a_divider() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "hello");
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.pane_count(), 1, "one pane to begin with");

    split(&mut app);
    let rows = screen(&mut app, 80, 24);
    for (i, row) in rows.iter().enumerate() {
        println!("{i:>2}|{row}|");
    }

    assert_eq!(app.pane_count(), 2);
    assert_eq!(app.active_pane, 1, "the new pane takes the focus");

    let left = app.panes[0].rect;
    let right = app.panes[1].rect;
    assert_eq!(
        left.height, right.height,
        "a split divides columns, not rows"
    );
    assert!(
        left.width.abs_diff(right.width) <= 1,
        "the two panes share the width evenly: {left:?} vs {right:?}"
    );
    assert_eq!(
        right.x,
        left.x + left.width + 1,
        "exactly one column between them"
    );
    assert_eq!(
        left.width + right.width + 1,
        80,
        "panes and divider fill the window"
    );

    // The divider runs the height of the editing area, and nowhere else.
    let column = (left.x + left.width) as usize;
    let divider: String = rows
        .iter()
        .skip(left.y as usize)
        .take(left.height as usize)
        .map(|r| r.chars().nth(column).unwrap_or(' '))
        .collect();
    assert!(
        divider.chars().all(|c| c == '▏'),
        "a hairline runs the height of the split: {divider:?}"
    );
    assert!(
        rows[0].chars().nth(column) != Some('▏'),
        "the top bar is not cut by it: {:?}",
        rows[0]
    );
}

#[test]
fn two_panes_show_one_buffer_at_two_places() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    split(&mut app);
    // The right pane has the focus, so the wheel moves it alone.
    wheel(&mut app, MouseEventKind::ScrollDown);
    wheel(&mut app, MouseEventKind::ScrollDown);
    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);

    assert!(
        left[0].contains("line 1"),
        "the left pane stayed at the top: {:?}",
        left[0]
    );
    assert!(
        !right[0].contains("line 1"),
        "the right pane scrolled: {:?}",
        right[0]
    );
    assert_eq!(
        app.panes[0].buffer, app.panes[1].buffer,
        "both panes show the same file"
    );
    assert_ne!(
        app.panes[0].top, app.panes[1].top,
        "with independent viewports"
    );
}

#[test]
fn an_edit_shows_in_both_panes() {
    let scratch = Scratch::new();
    let path = scratch.file("one.txt", "alpha\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);

    type_str(&mut app, "Z");
    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);
    assert!(left[0].contains("Zalpha"), "{:?}", left[0]);
    assert!(
        right[0].contains("Zalpha"),
        "the other pane is a view of the same text: {:?}",
        right[0]
    );
    assert_eq!(text(&app), "Zalpha\n", "one buffer, one edit");
}

#[test]
fn each_pane_can_show_a_different_buffer() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "first file\n");
    let b = scratch.file("b.txt", "second file\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    let _ = screen(&mut app, 80, 24);

    split(&mut app);
    // Load the first buffer into the focused (right) pane.
    app.focus_buffer(0);
    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);
    assert!(left[0].contains("second file"), "{:?}", left[0]);
    assert!(right[0].contains("first file"), "{:?}", right[0]);

    // Typing lands in the focused pane's buffer only.
    type_str(&mut app, "X");
    assert_eq!(text(&app), "Xfirst file\n");
    assert_eq!(
        app.buffers[1].line_text(0),
        "second file",
        "the other buffer is untouched"
    );
}

#[test]
fn clicking_a_pane_focuses_it() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "one\ntwo\n");
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.active_pane, 1);

    let left = app.panes[0].rect;
    click(&mut app, left.x + 4, left.y);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.active_pane, 0, "the click moved the focus");
    assert_eq!(
        app.editor_area(),
        left,
        "and the coordinate mapping followed it"
    );
}

#[test]
fn closing_a_pane_leaves_one_and_the_last_one_stays() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "text");
    let _ = screen(&mut app, 80, 24);

    split(&mut app);
    assert_eq!(app.pane_count(), 2);
    app.execute(Command::ClosePane);
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.pane_count(), 1);
    assert_eq!(app.active_pane, 0);
    assert_eq!(
        app.panes[0].rect.width, 80,
        "the survivor takes the whole width"
    );

    app.execute(Command::ClosePane);
    assert_eq!(app.pane_count(), 1, "the last pane cannot be closed");
    assert!(
        screen(&mut app, 80, 24)
            .iter()
            .any(|r| r.contains("last pane")),
        "and says so"
    );
}

#[test]
fn the_focus_moves_between_panes_and_wraps() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    assert_eq!(app.active_pane, 1);

    app.execute(Command::FocusPaneLeft);
    assert_eq!(app.active_pane, 0);
    app.execute(Command::FocusPaneLeft);
    assert_eq!(app.active_pane, 1, "the focus wraps at the end");
    app.execute(Command::FocusPaneRight);
    assert_eq!(app.active_pane, 0);
}

#[test]
fn closing_a_buffer_leaves_no_pane_pointing_at_it() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    let _ = screen(&mut app, 80, 24);

    split(&mut app);
    assert_eq!(app.panes[1].buffer, 1);

    // Close the buffer the right pane is showing, through the real command.
    app.focus_buffer(1);
    app.close_active();
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.buffers.len(), 1);
    for (i, pane) in app.panes.iter().enumerate() {
        assert!(
            pane.buffer < app.buffers.len(),
            "pane {i} points at buffer {} of {}",
            pane.buffer,
            app.buffers.len()
        );
    }
    assert!(
        pane_rows(&mut app, 80, 24, 1)[0].contains('a'),
        "the pane that lost its buffer shows the survivor"
    );
}

#[test]
fn the_status_bar_names_the_pane() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    assert!(
        !screen(&mut app, 80, 24)
            .iter()
            .any(|r| r.contains("pane 1/")),
        "a single pane needs no label"
    );

    split(&mut app);
    let rows = screen(&mut app, 80, 24);
    let status = rows
        .iter()
        .find(|r| r.contains(", Col "))
        .expect("a status bar");
    assert!(status.contains("pane 2/2"), "{status:?}");

    app.execute(Command::FocusPaneLeft);
    let rows = screen(&mut app, 80, 24);
    let status = rows
        .iter()
        .find(|r| r.contains(", Col "))
        .expect("a status bar");
    assert!(status.contains("pane 1/2"), "{status:?}");
}

#[test]
fn a_split_is_refused_when_the_panes_would_be_too_narrow() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    // 40 columns: two panes would be 19 each, under the 24-column floor.
    let _ = screen(&mut app, 40, 20);
    split(&mut app);

    assert_eq!(app.pane_count(), 1, "the split was refused");
    assert!(
        screen(&mut app, 40, 20)
            .iter()
            .any(|r| r.contains("No room")),
        "and explained"
    );
}

#[test]
fn the_divider_belongs_to_the_pane_it_introduces() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);

    let left = app.panes[0].rect;
    // The divider column is one to the right of the left pane.
    let divider_x = left.x + left.width;
    click(&mut app, divider_x, left.y);
    assert_eq!(
        app.active_pane, 1,
        "a click on the divider selects the pane it opens"
    );
}

#[test]
fn the_focused_pane_carries_the_accent_on_its_divider() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    let left = app.panes[0].rect;
    let divider_x = left.x + left.width;
    let theme = vela::ui::theme_of(&app).clone();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    assert_eq!(
        buf[(divider_x, left.y)].fg,
        theme.mark,
        "the divider next to the focused pane is the accent"
    );

    app.execute(Command::FocusPaneLeft);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    assert_eq!(
        buf[(divider_x, left.y)].fg,
        theme.line,
        "and steps back when the focus leaves"
    );
}

#[test]
fn a_pane_rect_is_never_empty_for_a_normal_window() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.pane_count(), 3);
    let mut x = 0;
    for pane in &app.panes {
        assert_eq!(pane.rect.x, x, "panes tile from the left edge");
        assert!(pane.rect.width >= 24, "wide enough to read: {pane:?}");
        x += pane.rect.width + 1;
    }
    assert_eq!(x - 1, 80, "and end at the right edge");
}

#[test]
fn tiny_windows_survive_a_split() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "some text");
    for (w, h) in [(20, 6), (30, 5), (80, 2)] {
        let _ = screen(&mut app, w, h);
        split(&mut app);
        let rows = screen(&mut app, w, h);
        assert_eq!(rows.len(), h as usize, "{w}x{h}");
        assert!(
            app.panes
                .iter()
                .all(|p: &vela::app::Pane| p.rect.width <= w),
            "no pane is wider than the window"
        );
    }
}

#[test]
fn the_pane_layout_round_trips_through_a_frame() {
    // Every pane's rect has to be inside the editing area the frame was given,
    // or the mouse mapping and the painter disagree.
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    let span: Rect = app.editor_span();
    for pane in &app.panes {
        assert!(
            pane.rect.x >= span.x && pane.rect.x + pane.rect.width <= span.x + span.width,
            "{:?} inside {span:?}",
            pane.rect
        );
        assert_eq!(pane.rect.y, span.y);
        assert_eq!(pane.rect.height, span.height);
    }
}

