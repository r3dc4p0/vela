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
use vibe::commands::Command;

fn numbered(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split the active pane with the real key, so the binding is covered too.
fn split(app: &mut vibe::app::App) {
    press(app, KeyCode::Char('\\'), KeyModifiers::CONTROL);
}

/// One pane's slice of every rendered row.
fn pane_rows(app: &mut vibe::app::App, width: u16, height: u16, pane: usize) -> Vec<String> {
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

fn click(app: &mut vibe::app::App, column: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn wheel(app: &mut vibe::app::App, kind: MouseEventKind) {
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
    let theme = vibe::ui::theme_of(&app).clone();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    assert_eq!(
        buf[(divider_x, left.y)].fg,
        theme.mark,
        "the divider next to the focused pane is the accent"
    );

    app.execute(Command::FocusPaneLeft);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, &mut app)).unwrap();
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
                .all(|p: &vibe::app::Pane| p.rect.width <= w),
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

// ---------------------------------------------------------------------------
// Two panes on one buffer: the cursor belongs to the document, the viewport
// belongs to the pane. These pin down the boundary between the two.
// ---------------------------------------------------------------------------

#[test]
fn the_other_pane_does_not_chase_the_cursor() {
    // The cursor is shared — one document — but the viewport is not. Moving the
    // cursor must not drag the pane the cursor is not in.
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);

    // The right pane has the focus: walk the cursor down out of the left pane's
    // view, which starts at line 1.
    for _ in 0..60 {
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    }
    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);

    assert_eq!(app.panes[0].top, 0, "the unfocused pane stayed at the top");
    assert!(
        left[0].contains("line 1"),
        "and still shows it: {:?}",
        left[0]
    );
    assert!(
        !right[0].contains("line 1"),
        "while the focused one followed the cursor: {:?}",
        right[0]
    );
}

#[test]
fn scrolling_the_focused_pane_leaves_the_other_alone() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);

    wheel(&mut app, MouseEventKind::ScrollDown);
    wheel(&mut app, MouseEventKind::ScrollDown);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.panes[0].top, 0, "the left pane did not move");
    assert!(app.panes[1].top > 0, "the right one did");
}

#[test]
fn focus_moves_do_not_move_either_view() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);

    wheel(&mut app, MouseEventKind::ScrollDown);
    let _ = screen(&mut app, 80, 24);
    let right_top = app.panes[1].top;
    assert!(right_top > 0);

    app.execute(Command::FocusPaneLeft);
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.panes[1].top, right_top, "leaving did not move it");
    assert_eq!(app.panes[0].top, 0, "and the left pane stayed put too");

    app.execute(Command::FocusPaneRight);
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.panes[1].top, right_top, "coming back did not move it");
}

#[test]
fn typing_does_not_drag_the_other_pane_to_the_cursor() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);

    // Scroll the left pane away from the cursor, then type in the right one.
    app.execute(Command::FocusPaneLeft);
    wheel(&mut app, MouseEventKind::ScrollDown);
    let _ = screen(&mut app, 80, 24);
    let left_top = app.panes[0].top;
    assert!(left_top > 0);

    app.execute(Command::FocusPaneRight);
    let _ = screen(&mut app, 80, 24);
    type_str(&mut app, "X");
    let _ = screen(&mut app, 80, 24);

    assert_eq!(
        app.panes[0].top, left_top,
        "the pane that was not being typed into did not move"
    );
    assert!(
        pane_rows(&mut app, 80, 24, 0)[0].contains(&format!("line {}", left_top + 1)),
        "and still shows the line it was showing"
    );
}

#[test]
fn a_click_maps_against_the_pane_it_lands_in() {
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    // Put the two panes at different places first.
    split(&mut app);
    wheel(&mut app, MouseEventKind::ScrollDown);
    wheel(&mut app, MouseEventKind::ScrollDown);
    let _ = screen(&mut app, 80, 24);

    let left = app.panes[0].rect;
    let right = app.panes[1].rect;
    let row = left.y + 3;

    // Row 3 of the left pane is line 4; row 3 of the right pane is further down
    // because that pane is scrolled.
    click(&mut app, left.x + 6, row);
    let _ = screen(&mut app, 80, 24);
    let (left_line, _) = app.buffers[0].cursor_line_col();
    assert_eq!(left_line, 3, "row 3 of the unscrolled pane is line 4");

    click(&mut app, right.x + 6, row);
    let _ = screen(&mut app, 80, 24);
    let (right_line, _) = app.buffers[0].cursor_line_col();
    assert_ne!(
        right_line, left_line,
        "the same screen row is a different line in a scrolled pane"
    );
    assert_eq!(
        right_line,
        app.panes[1].top + 3,
        "and it is the line that pane is showing"
    );
}

#[test]
fn the_caret_is_placed_in_the_focused_pane() {
    // The caret is the terminal's cursor, not a cell: whichever pane draws last
    // would own it, and an inactive pane drawing the document's cursor is what
    // made it appear in the wrong view.
    let scratch = Scratch::new();
    let path = scratch.file("one.txt", "alpha\nbeta\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, &mut app)).unwrap();
    let caret = terminal.get_cursor_position().expect("a cursor position");
    let right = app.panes[1].rect;
    let left = app.panes[0].rect;
    assert!(
        right.contains(caret),
        "the caret is in the focused pane: {caret:?} not in {right:?}"
    );
    assert!(!left.contains(caret), "and not in the other one");

    // It follows the focus, rather than staying where it was drawn.
    app.execute(Command::FocusPaneLeft);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, &mut app)).unwrap();
    let caret = terminal.get_cursor_position().expect("a cursor position");
    assert!(
        left.contains(caret),
        "the caret moved with the focus: {caret:?} in {left:?}"
    );
}

#[test]
fn the_wheel_scrolls_the_pane_under_the_pointer() {
    // Scrolling is looking, not typing: the pane under the pointer moves, and
    // the focus stays where the keyboard is.
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.active_pane, 1, "the right pane has the focus");

    let left = app.panes[0].rect;
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: left.x + 5,
        row: left.y + 4,
        modifiers: KeyModifiers::NONE,
    }));
    let _ = screen(&mut app, 80, 24);

    assert!(app.panes[0].top > 0, "the pane under the pointer scrolled");
    assert_eq!(app.panes[1].top, 0, "the other one did not");
    assert_eq!(app.active_pane, 1, "and the focus did not move");
}

#[test]
fn the_wheel_over_a_pane_does_not_disturb_the_cursor_view() {
    // The pane that owns the cursor must not be dragged by a wheel gesture in
    // its neighbour.
    let scratch = Scratch::new();
    let path = scratch.file("long.txt", &numbered(200));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);
    let right_top = app.panes[1].top;

    let left = app.panes[0].rect;
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: left.x + 5,
        row: left.y + 4,
        modifiers: KeyModifiers::NONE,
    }));
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.panes[1].top, right_top, "the focused pane stayed put");
    assert_eq!(
        app.buffers[0].scroll_top(),
        right_top,
        "and so did the document the cursor reads from"
    );
}

// ---------------------------------------------------------------------------
// Opening files into a split.
// ---------------------------------------------------------------------------

#[test]
fn opening_a_file_into_a_split_keeps_both_panes() {
    // The blank buffer is on screen twice, so it cannot be swapped for the file
    // behind both panes' backs: the file goes into the pane with the focus and
    // the other keeps the scratch buffer it was showing.
    let scratch = Scratch::new();
    let path = scratch.file("notes.txt", "hello\n");
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    open(&mut app, &path);
    let rows = screen(&mut app, 80, 24);
    for (i, row) in rows.iter().enumerate() {
        println!("{i:>2}|{row}|");
    }

    assert_eq!(app.pane_count(), 2, "the split survived opening a file");
    assert_eq!(app.buffers.len(), 2, "the blank buffer was kept");
    assert_eq!(
        app.panes[1].buffer, 1,
        "the file landed in the pane with the focus"
    );
    assert_eq!(app.panes[0].buffer, 0, "the other pane kept its buffer");

    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);
    assert!(right.iter().any(|r| r.contains("hello")), "{right:?}");
    assert!(
        !left.iter().any(|r| r.contains("hello")),
        "the left pane is still the blank one: {left:?}"
    );
}

#[test]
fn opening_a_second_file_puts_it_beside_the_first() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "first\n");
    let b = scratch.file("b.txt", "second\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    open(&mut app, &b);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.pane_count(), 2);
    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);
    assert!(left.iter().any(|r| r.contains("first")), "{left:?}");
    assert!(right.iter().any(|r| r.contains("second")), "{right:?}");
}

#[test]
fn one_pane_still_replaces_the_blank_buffer() {
    // The old behaviour, unchanged: no stray untitled tab after opening a file.
    let scratch = Scratch::new();
    let path = scratch.file("notes.txt", "hello\n");
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);

    open(&mut app, &path);
    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.buffers.len(), 1, "the blank buffer was replaced");
    assert_eq!(app.buffers[0].path(), Some(path.as_path()));
}

#[test]
fn a_blank_buffer_shown_once_can_be_replaced_even_in_a_split() {
    // One pane on a file, the other on a blank scratch: opening a file replaces
    // the blank rather than leaving an empty tab, because only one view is
    // looking at it.
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "first\n");
    let b = scratch.file("b.txt", "second\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    // A fresh buffer in the focused (right) pane.
    app.new_buffer();
    let _ = screen(&mut app, 80, 24);
    assert_eq!(app.buffers.len(), 2, "a blank scratch beside the file");

    open(&mut app, &b);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.pane_count(), 2, "the split survived");
    assert_eq!(app.buffers.len(), 2, "the blank was replaced, not kept");
    assert!(
        app.buffers.iter().all(|d| d.path().is_some()),
        "no untitled buffer left behind"
    );
    let left = pane_rows(&mut app, 80, 24, 0);
    let right = pane_rows(&mut app, 80, 24, 1);
    assert!(left.iter().any(|r| r.contains("first")), "{left:?}");
    assert!(right.iter().any(|r| r.contains("second")), "{right:?}");
}

#[test]
fn a_big_file_opened_into_a_split_keeps_both_panes() {
    // Above the async threshold the document arrives from a worker, and the
    // hand-over must not collapse the split either.
    let scratch = Scratch::new();
    let mut body = String::new();
    while body.len() < 5 * 1024 * 1024 {
        body.push_str("a line of text that is not short\n");
    }
    let path = scratch.file("big.txt", &body);
    let mut app = editor(scratch.path());
    let _ = screen(&mut app, 80, 24);
    split(&mut app);
    let _ = screen(&mut app, 80, 24);

    app.open_path(path.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.loading.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the open never finished"
        );
        app.poll_jobs();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let _ = screen(&mut app, 80, 24);

    assert_eq!(app.pane_count(), 2, "the split survived the worker");
    assert_eq!(
        app.panes[1].buffer, 1,
        "the file landed in the focused pane"
    );
    assert!(app.buffers[0].path().is_none(), "the scratch was kept");
}
