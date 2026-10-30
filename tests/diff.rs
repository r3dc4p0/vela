//! The diff view, driven the way a user drives it.
//!
//! The engine's own tests (`src/diff.rs`) cover the alignment; these cover what
//! the view does with it: two columns, one scroll offset, colours that say what
//! happened, and a read-only mode that cannot edit the buffer behind it.

mod common;

use common::*;
use crossterm::event::KeyCode;
use vibe::app::App;
use vibe::commands::Command;
use vibe::diff::Kind;

/// Two files that differ in one rewritten line, one insertion and one deletion.
fn pair(scratch: &Scratch) -> (std::path::PathBuf, std::path::PathBuf) {
    // One rewritten line, one deletion and one insertion, each separated by a
    // line both files share — so the diff has three blocks and the alignment
    // has something to prove.
    let old = scratch.file(
        "old.rs",
        "fn main() {\n    let a = 1;\n    let b = 2;\n    gone();\n    keep();\n    println!(\"{a}\");\n}\n",
    );
    let new = scratch.file(
        "new.rs",
        "fn main() {\n    let a = 1;\n    let c = 3;\n    keep();\n    println!(\"{a}\");\n    extra();\n}\n",
    );
    (old, new)
}

/// An editor with both files open and the comparison already showing.
fn comparing(scratch: &Scratch) -> App {
    let (old, new) = pair(scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &old);
    open(&mut app, &new);
    assert!(app.start_diff(0, 1), "the comparison opened");
    app
}

/// The rendered cells, for assertions about colour.
fn cells(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vibe::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// Index of the first screen row containing `needle`.
fn row_index_with(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("no row containing {needle:?} in:\n{}", rows.join("\n")))
}

/// The first screen row containing `needle`.
fn row_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
    &rows[row_index_with(rows, needle)]
}

/// The left and right halves of a rendered row, split at the divider.
fn halves(row: &str) -> (&str, &str) {
    match row.find('▏') {
        Some(at) => (&row[..at], &row[at + '▏'.len_utf8()..]),
        None => (row, ""),
    }
}

#[test]
fn a_comparison_shows_both_files_side_by_side() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let rows = screen(&mut app, 100, 16);

    let header = row_with(&rows, "→");
    assert!(header.contains("old.rs"), "the left name: {header:?}");
    assert!(header.contains("new.rs"), "the right name: {header:?}");
    assert!(header.contains("changes"), "and how many: {header:?}");

    // The same line appears in both halves of one row.
    let row = rows
        .iter()
        .find(|r| {
            let (l, r) = halves(r);
            l.contains("let a = 1;") && r.contains("let a = 1;")
        })
        .expect("the unchanged line is drawn twice, side by side");
    assert!(
        halves(row).0.contains('2') && halves(row).1.contains('2'),
        "with a line number on each side: {row:?}"
    );
}

#[test]
fn a_changed_line_is_tinted_on_both_sides() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let (width, height) = (100u16, 16u16);
    let rows = screen(&mut app, width, height);
    let y = row_index_with(&rows, "let b = 2;") as u16;

    let theme = vibe::theme::Theme::dark();
    let buf = cells(&mut app, width, height);
    assert_eq!(
        buf[(0, y)].bg,
        theme.diff_del_bg,
        "the old line is tinted as a deletion"
    );
    let right_x = width / 2;
    assert_eq!(
        buf[(right_x + 1, y)].bg,
        theme.diff_add_bg,
        "the new line is tinted as an insertion"
    );
    assert!(
        rows[y as usize].contains('~'),
        "and both are marked as changed: {:?}",
        rows[y as usize]
    );
}

#[test]
fn an_inserted_line_leaves_a_gap_on_the_left() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let (width, height) = (100u16, 16u16);
    let rows = screen(&mut app, width, height);
    let y = row_index_with(&rows, "extra();");

    let (left, right) = halves(&rows[y]);
    assert!(right.contains('+'), "the insertion is marked: {right:?}");
    assert!(
        left.trim().is_empty(),
        "the left side of that row is empty: {left:?}"
    );

    // The gap is drawn as a darker cell rather than as a blank line, so it
    // cannot be mistaken for a line that happens to be empty.
    let theme = vibe::theme::Theme::dark();
    let buf = cells(&mut app, width, height);
    assert_eq!(buf[(0, y as u16)].bg, theme.diff_filler_bg);
    assert_eq!(buf[(5, y as u16)].symbol(), " ", "and holds no text");
}

#[test]
fn the_columns_stay_aligned_across_an_insertion() {
    // The point of pairing rows: after an inserted line the two files are one
    // line out of step, and the closing brace still sits beside the closing
    // brace rather than beside whatever happens to be at the same index.
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let rows = screen(&mut app, 100, 16);

    let brace = rows
        .iter()
        .find(|r| {
            let (l, r) = halves(r);
            l.trim_end().ends_with('}') && r.trim_end().ends_with('}')
        })
        .expect("the brace is on one row");
    let (l, r) = halves(brace);
    // One line was deleted and one inserted, so both files still end at seven:
    // the brace is line 7 on each side, on one row.
    assert!(l.contains('7'), "left line 7: {l:?}");
    assert!(r.contains('7'), "right line 7: {r:?}");

    // The proof of alignment is a line that *moved*: `keep()` is line 5 on the
    // left and line 4 on the right, because a line was deleted above it.
    let moved = rows
        .iter()
        .find(|r| {
            let (l, r) = halves(r);
            l.contains("keep();") && r.contains("keep();")
        })
        .expect("keep() is on one row");
    let (l, r) = halves(moved);
    assert!(l.contains('5'), "left line 5: {l:?}");
    assert!(r.contains('4'), "right line 4: {r:?}");
}

#[test]
fn scrolling_moves_both_columns_together() {
    let scratch = Scratch::new();
    let old: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let new = old.replace("line 30\n", "line 30 changed\n");
    let a = scratch.file("a.txt", &old);
    let b = scratch.file("b.txt", &new);
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    app.start_diff(0, 1);

    let height = 16u16;
    // The view opens on the change; go to the top first.
    screen(&mut app, 80, height);
    plain(&mut app, KeyCode::Home);
    let before = screen(&mut app, 80, height);
    assert!(
        before.iter().any(|r| r.contains("line 0")),
        "at the top after Home:\n{}",
        before.join("\n")
    );

    plain(&mut app, KeyCode::PageDown);
    let after = screen(&mut app, 80, height);
    assert!(
        !after.iter().any(|r| r.contains("line 0")),
        "the page moved"
    );
    // Both sides moved by the same rows, because there is one offset.
    let row = after
        .iter()
        .find(|r| r.contains("line 20"))
        .expect("line 20 is on screen");
    let (l, r) = halves(row);
    assert!(l.contains("line 20"), "left: {l:?}");
    assert!(r.contains("line 20"), "right: {r:?}");
}

#[test]
fn the_change_is_scrolled_into_view_when_the_view_opens() {
    // A difference at line 900 is the reason to open the view; starting at the
    // top would hide it.
    let scratch = Scratch::new();
    let old: String = (0..1000).map(|i| format!("line {i}\n")).collect();
    let new = old.replace("line 900\n", "line 900 edited\n");
    let a = scratch.file("long-a.txt", &old);
    let b = scratch.file("long-b.txt", &new);
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    app.start_diff(0, 1);

    let rows = screen(&mut app, 80, 20);
    assert!(
        rows.iter().any(|r| r.contains("line 900 edited")),
        "the change is visible without scrolling:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        app.diff.as_ref().unwrap().scroll,
        898,
        "two rows of context"
    );
}

#[test]
fn next_and_previous_change_step_through_the_blocks() {
    let scratch = Scratch::new();
    let old: String = (0..80).map(|i| format!("line {i}\n")).collect();
    let mut new = old.clone();
    new = new.replace("line 20\n", "line 20 edited\n");
    new = new.replace("line 60\n", "line 60 edited\n");
    let a = scratch.file("a.txt", &old);
    let b = scratch.file("b.txt", &new);
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    app.start_diff(0, 1);
    screen(&mut app, 80, 20);

    // Opens on the first change.
    assert_eq!(app.diff.as_ref().unwrap().position(), Some((1, 2)));

    plain(&mut app, KeyCode::Char(']'));
    let view = app.diff.as_ref().unwrap();
    assert_eq!(view.position(), Some((2, 2)), "the second change");
    assert_eq!(view.scroll, 58, "with its two rows of context");

    // Past the last one it wraps, rather than doing nothing.
    plain(&mut app, KeyCode::Char(']'));
    assert_eq!(app.diff.as_ref().unwrap().position(), Some((1, 2)));

    plain(&mut app, KeyCode::Char('['));
    assert_eq!(
        app.diff.as_ref().unwrap().position(),
        Some((2, 2)),
        "and back the other way, wrapping too"
    );
}

#[test]
fn a_tall_change_is_stepped_through_rather_than_skipped() {
    // A three-hundred-line rewrite is not one keystroke: "next" from inside it
    // moves a page down within it.
    let scratch = Scratch::new();
    let old: String = (0..300).map(|i| format!("old {i}\n")).collect();
    let new: String = (0..300).map(|i| format!("new {i}\n")).collect();
    let a = scratch.file("a.txt", &old);
    let b = scratch.file("b.txt", &new);
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    app.start_diff(0, 1);
    screen(&mut app, 80, 20);

    let first = app.diff.as_ref().unwrap().scroll;
    plain(&mut app, KeyCode::Char(']'));
    let second = app.diff.as_ref().unwrap().scroll;
    assert!(
        second > first && second < 100,
        "moved within the block: {first} -> {second}"
    );
}

#[test]
fn the_scroll_keys_stop_at_the_ends() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    screen(&mut app, 100, 16);

    plain(&mut app, KeyCode::Home);
    assert_eq!(app.diff.as_ref().unwrap().scroll, 0);
    plain(&mut app, KeyCode::Up);
    assert_eq!(
        app.diff.as_ref().unwrap().scroll,
        0,
        "no scrolling past the top"
    );

    plain(&mut app, KeyCode::End);
    let last = app.diff.as_ref().unwrap().scroll;
    plain(&mut app, KeyCode::Down);
    assert_eq!(
        app.diff.as_ref().unwrap().scroll,
        last,
        "nor past the bottom"
    );
}

#[test]
fn escape_closes_the_view_and_leaves_the_buffers_alone() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    assert!(app.diff_open());

    plain(&mut app, KeyCode::Esc);
    assert!(!app.diff_open(), "the view closed");
    assert_eq!(app.buffers.len(), 2, "and the files are still open");
    assert!(
        screen_has(&mut app, 100, 16, "let c = 3;"),
        "the editor is back, showing the active buffer"
    );
}

#[test]
fn typing_while_the_diff_is_open_does_not_edit_the_buffer() {
    // The buffer behind the view is not on screen, so an edit would change
    // something the user cannot see.
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let before = app.active_doc().unwrap().text();

    type_str(&mut app, "hello");
    plain(&mut app, KeyCode::Backspace);
    plain(&mut app, KeyCode::Enter);
    plain(&mut app, KeyCode::Tab);

    assert_eq!(app.active_doc().unwrap().text(), before);
    assert!(app.diff_open(), "and the view is still up");
}

#[test]
fn the_same_buffer_cannot_be_compared_with_itself() {
    let scratch = Scratch::new();
    let (old, _) = pair(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &old);

    assert!(!app.start_diff(0, 0));
    assert!(!app.diff_open());
    assert!(screen_has(&mut app, 100, 16, "cannot be compared"));
}

#[test]
fn comparing_with_a_file_that_is_already_open_reuses_it() {
    let scratch = Scratch::new();
    let (old, new) = pair(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &old);
    open(&mut app, &new);
    // The second file is the one in front; compare it with the first.
    app.set_active_buffer(1);

    app.compare_with_path(old.clone());
    assert!(app.diff_open(), "the comparison opened");
    assert_eq!(app.buffers.len(), 2, "and no third copy was opened");
    let view = app.diff.as_ref().unwrap();
    assert_eq!(
        (view.left, view.right),
        (1, 0),
        "left is what was on screen"
    );
}

#[test]
fn comparing_with_a_file_that_is_not_open_opens_it() {
    let scratch = Scratch::new();
    let (old, new) = pair(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &old);

    app.compare_with_path(new.clone());
    assert!(
        app.diff_open(),
        "the comparison opened once the file loaded"
    );
    assert_eq!(app.buffers.len(), 2);
    let view = app.diff.as_ref().unwrap();
    assert_eq!((view.left, view.right), (0, 1));
}

#[test]
fn closing_one_of_the_compared_files_closes_the_view() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    app.set_active_buffer(1);
    app.close_active();
    assert!(
        !app.diff_open(),
        "a comparison against a buffer that is gone has nothing to draw"
    );
}

#[test]
fn identical_files_say_so() {
    let scratch = Scratch::new();
    let path = scratch.file("same.txt", "one\ntwo\nthree\n");
    let other = scratch.file("copy.txt", "one\ntwo\nthree\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    open(&mut app, &other);
    app.start_diff(0, 1);

    let rows = screen(&mut app, 100, 16);
    assert!(
        rows.iter().any(|r| r.contains("identical")),
        "the header says it:\n{}",
        rows.join("\n")
    );
    assert_eq!(app.diff.as_ref().unwrap().stats.changed, 0);
}

#[test]
fn the_header_counts_what_changed() {
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let rows = screen(&mut app, 100, 16);
    let header = row_with(&rows, "→");
    // One insertion, one deletion, one rewritten line — the fixture.
    assert!(header.contains("+1"), "{header:?}");
    assert!(header.contains("−1"), "{header:?}");
    assert!(header.contains("~1"), "{header:?}");
}

#[test]
fn the_wheel_scrolls_the_view() {
    let scratch = Scratch::new();
    let old: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let a = scratch.file("a.txt", &old);
    let b = scratch.file("b.txt", &old.replace("line 30\n", "line 30 x\n"));
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    app.start_diff(0, 1);
    screen(&mut app, 80, 16);
    let before = app.diff.as_ref().unwrap().scroll;

    use crossterm::event::{Event, MouseEvent, MouseEventKind};
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 6,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
    assert!(app.diff.as_ref().unwrap().scroll > before);
}

#[test]
fn the_view_keeps_its_shape_when_the_window_is_narrow() {
    // Two columns in forty cells: the numbers and a few characters of text.
    let scratch = Scratch::new();
    let mut app = comparing(&scratch);
    let rows = screen(&mut app, 40, 12);
    assert!(rows.iter().any(|r| r.contains('▏')), "the divider survives");
    assert_eq!(rows.len(), 12, "and nothing is drawn outside the screen");
}

#[test]
fn the_commands_are_discoverable() {
    assert_eq!(Command::CompareWith.id(), "file.compareWith");
    assert!(Command::CompareWith.default_keys().contains(&"alt+d"));
    assert_eq!(Command::CloseDiff.id(), "view.closeDiff");
    assert_eq!(Command::DiffNextChange.id(), "view.nextChange");

    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.open_palette(vibe::ui::PaletteMode::Commands);
    type_str(&mut app, "compare");
    assert!(
        screen_has(&mut app, 100, 30, "Compare with File"),
        "the palette offers it"
    );
}

#[test]
fn the_palette_for_comparing_says_so() {
    let scratch = Scratch::new();
    let (old, _) = pair(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &old);

    app.compare_prompt();
    assert!(
        screen_has(&mut app, 100, 30, "Compare with"),
        "the picker title"
    );
}

#[test]
fn a_buffer_with_no_file_has_nothing_to_compare() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "unsaved");
    app.compare_prompt();
    assert!(
        screen_has(&mut app, 100, 24, "Save the buffer first"),
        "it says why rather than opening a picker"
    );
}

#[test]
fn the_engine_and_the_view_agree_about_the_rows() {
    // The view draws row `n` of each column, so a row list that skipped a line
    // would show a file with lines missing — and nothing else would notice.
    let scratch = Scratch::new();
    let app = comparing(&scratch);
    let view = app.diff.as_ref().unwrap();
    let left: Vec<usize> = view.rows.iter().filter_map(|r| r.left_line()).collect();
    let right: Vec<usize> = view.rows.iter().filter_map(|r| r.right_line()).collect();
    assert_eq!(left, (0..view.left_lines).collect::<Vec<_>>());
    assert_eq!(right, (0..view.right_lines).collect::<Vec<_>>());
    assert_eq!(view.stats.removed, 1);
    assert_eq!(view.stats.added, 1);
    assert_eq!(view.stats.changed, 1);
    assert_eq!(
        view.rows.iter().filter(|r| r.kind == Kind::Changed).count(),
        1
    );
}
