//! Rendering tests: the whole UI is drawn into ratatui's `TestBackend`, which
//! is a virtual terminal, so these assertions check what the user would
//! actually see (layout, chrome, gutter, overlays) without a real tty.

mod common;

use common::*;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;
use vibe::commands::Command;

fn dump(app: &mut vibe::app::App, width: u16, height: u16) {
    for (i, row) in screen(app, width, height).iter().enumerate() {
        println!("{i:>2}|{row}|");
    }
}

/// Render one frame and hand back the cell buffer, for assertions that need
/// colours rather than text.
fn cells(app: &mut vibe::app::App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn a_new_line_at_the_end_of_the_file_moves_the_cursor_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abc");
    plain(&mut app, KeyCode::Enter);

    // The status bar reads the same cursor position the renderer uses.
    let rows = screen(&mut app, 80, 24);
    let status = rows
        .iter()
        .find(|row| row.contains("Ln "))
        .expect("status bar");
    assert!(status.contains("Ln 2, Col 1"), "status bar: {status}");

    // And the highlighted cursor line is the new, empty one.
    let highlight = app.theme.cursor_line_bg;
    let buf = cells(&mut app, 80, 24);
    let editor = app.editor_area();
    let lit: Vec<u16> = (editor.y..editor.y + editor.height)
        .filter(|&y| (editor.x..editor.x + editor.width).any(|x| buf[(x, y)].bg == highlight))
        .collect();
    assert_eq!(
        lit,
        vec![editor.y + 1],
        "the second editor row is the cursor line"
    );
}

#[test]
fn draws_topbar_status_and_text() {
    let scratch = Scratch::new();
    let path = scratch.file("hello.txt", "hello world\nsecond line\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);

    assert!(rows[0].contains("VIBE"), "top bar shows the app name");
    assert!(rows[0].contains("hello.txt"), "top bar shows the file name");
    let text_row = rows
        .iter()
        .position(|r| r.contains("hello world"))
        .expect("the first line is rendered");
    assert!(
        rows[text_row].contains("1"),
        "the gutter shows line numbers"
    );
    let status = rows
        .iter()
        .find(|r| r.contains("Ln "))
        .expect("a status bar is drawn");
    assert!(status.contains("Ln 1, Col 1"), "got: {status}");
    assert!(status.contains("UTF-8"), "status bar shows the encoding");
    assert!(status.contains("LF"), "status bar shows the line endings");
}

#[test]
fn dirty_buffers_are_marked_in_the_topbar() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "x");
    let rows = screen(&mut app, 80, 24);
    assert!(
        rows[0].contains('●'),
        "a modified buffer gets a dot in the top bar: {}",
        rows[0]
    );
}

#[test]
fn tabs_appear_with_more_than_one_buffer() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    assert!(rows[1].contains("a.txt"), "both tabs are listed");
    assert!(rows[1].contains("b.txt"));
}

#[test]
fn tildes_fill_the_area_past_the_end_of_the_buffer() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "only line");
    let rows = screen(&mut app, 60, 20);
    assert!(rows[5].contains('~'), "empty rows show tildes");
}

#[test]
fn relative_line_numbers_are_configurable() {
    let scratch = Scratch::new();
    let path = scratch.file("many.txt", "1\n2\n3\n4\n5\n6\n");
    let mut app = editor_with(scratch.path(), |c| c.editor.relative_numbers = true);
    open(&mut app, &path);
    plain(&mut app, KeyCode::Down);
    plain(&mut app, KeyCode::Down);
    let rows = screen(&mut app, 60, 20);
    dump(&mut app, 60, 20);
    // Row 0 is the top bar, so line 1 is row 1 and the cursor is on row 3.
    assert!(rows[1].contains('2'), "line 1 is two rows above the cursor");
    assert!(rows[2].contains('1'), "line 2 is one row above");
    assert!(
        rows[3].contains('3'),
        "the cursor line keeps its absolute number"
    );
}

#[test]
fn long_lines_are_scrolled_horizontally() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, &"x".repeat(200));
    let rows = screen(&mut app, 40, 12);
    // The cursor is at the end of the line, so the view must have scrolled.
    let row = rows
        .iter()
        .find(|r| r.contains("xxx"))
        .expect("the tail of the long line is visible");
    assert!(
        !row.contains(&"x".repeat(60)),
        "the row is clipped to the terminal width"
    );
}

#[test]
fn whitespace_and_control_characters_get_visible_glyphs() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| c.editor.show_whitespace = true);
    app.handle_event(crossterm::event::Event::Paste("\ta\u{7}b c".to_string()));
    let rows = screen(&mut app, 40, 12);
    dump(&mut app, 40, 12);
    let row = rows.iter().find(|r| r.contains('→')).expect("the text row");
    assert!(row.contains('→'), "a tab is drawn as an arrow: {row:?}");
    assert!(
        row.contains("^G"),
        "a control character is caret-escaped: {row:?}"
    );
    assert!(row.contains('·'), "spaces are marked: {row:?}");
}

#[test]
fn overlay_find_bar_is_drawn_and_captures_the_screen() {
    let scratch = Scratch::new();
    let path = scratch.file("find.txt", "needle in a haystack\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, 'f');
    ctrl(&mut app, 'u');
    for ch in "needle".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    assert!(
        rows.iter().any(|r| r.contains("needle")),
        "the query is echoed in the find bar"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("1/1") || r.contains("1 of 1")),
        "the match counter is shown"
    );
}

#[test]
fn command_palette_renders_rows() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.open_palette(vibe::ui::PaletteMode::Commands);
    let rows = screen(&mut app, 90, 30);
    dump(&mut app, 90, 30);
    assert!(
        rows.iter().any(|r| r.contains("Command")),
        "the palette has a title"
    );
    assert!(
        rows.iter().any(|r| r.contains("Save")),
        "commands are listed"
    );
}

#[test]
fn a_panel_hides_the_text_underneath_it() {
    let scratch = Scratch::new();
    // A marker that appears nowhere in the palette: the leak this guards
    // against showed the file's own characters inside the panel's background.
    let path = scratch.file("under.txt", &vec!["¤".repeat(70); 40].join("\n"));
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert!(
        screen(&mut app, 80, 24).iter().any(|r| r.contains('¤')),
        "the file is on screen before the overlay opens"
    );

    app.open_palette(vibe::ui::PaletteMode::Commands);
    let buf = cells(&mut app, 80, 24);
    dump(&mut app, 80, 24);

    let panel = vibe::ui::chrome::centered(ratatui::layout::Rect::new(0, 0, 80, 24), 80, 60);
    for y in panel.y + 1..panel.y + panel.height - 1 {
        for x in panel.x + 1..panel.x + panel.width - 1 {
            assert_ne!(
                buf[(x, y)].symbol(),
                "¤",
                "the file shows through the panel at ({x}, {y})"
            );
        }
    }
}

#[test]
fn the_find_bar_replaces_the_hint_row_instead_of_mixing_with_it() {
    let scratch = Scratch::new();
    let path = scratch.file("find.txt", "needle\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert!(
        screen(&mut app, 80, 24).last().unwrap().contains('·'),
        "the hint bar is on the last row to begin with"
    );

    ctrl(&mut app, 'f');
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    let bar = rows.last().expect("a last row");

    assert!(bar.contains("Find"), "the bar is drawn: {bar:?}");
    assert!(
        !bar.contains('·'),
        "the hints underneath are wiped, not blended: {bar:?}"
    );
}

#[test]
fn confirm_dialog_renders_its_choices() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "unsaved");
    ctrl(&mut app, 'q');
    let rows = screen(&mut app, 90, 30);
    dump(&mut app, 90, 30);
    assert!(rows.iter().any(|r| r.contains("unsaved changes")));
    assert!(rows.iter().any(|r| r.contains("Discard")));
    assert!(rows.iter().any(|r| r.contains("Cancel")));
}

#[test]
fn tiny_terminals_do_not_panic() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "some text here\nand more");
    for (w, h) in [(1, 1), (2, 2), (5, 3), (10, 4), (20, 6), (80, 1)] {
        let rows = screen(&mut app, w, h);
        assert_eq!(rows.len(), h as usize);
    }
}

#[test]
fn resizing_keeps_the_ui_consistent() {
    let scratch = Scratch::new();
    let path = scratch.file("resize.txt", "alpha\nbeta\ngamma\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 120, 40);
    let small = screen(&mut app, 30, 8);
    assert_eq!(small.len(), 8);
    assert!(small.iter().any(|r| r.contains("alpha")));
    let large = screen(&mut app, 200, 50);
    assert_eq!(large.len(), 50);
    assert!(large[0].contains("VIBE"));
}

#[test]
fn toggling_the_theme_changes_the_rendered_colours() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "colour");

    let sample = |app: &mut vibe::app::App| {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal.draw(|f| vibe::ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..10)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].bg)
            .find(|c| *c != ratatui::style::Color::Reset)
    };

    let dark = sample(&mut app);
    app.execute(Command::ToggleTheme);
    let light = sample(&mut app);
    assert!(dark.is_some() && light.is_some());
    assert_ne!(dark, light, "the background colour follows the theme");
}

#[test]
fn messages_are_shown_above_the_hints() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.execute(Command::WordCount);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    assert!(
        rows.iter().any(|r| r.contains("words")),
        "the word count message is visible"
    );
}
