//! Indent guides: the rule that marks where a block begins and ends.
//!
//! The assertions are made on the rendered screen rather than on the buffer,
//! because this is a drawing feature: what matters is which columns of which
//! rows carry a rule. A rule sits in the last column of its level's
//! indentation, so a body indented by four shows it in column 3 — immediately
//! before the text — and a line outdented to zero shows none, which is how the
//! end of the block is seen.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyModifiers};
use vibe::commands::Command;

/// The columns (0-based, from the first text column) that carry a rule on
/// editor row `row`.
fn guides(app: &mut vibe::app::App, row: usize) -> Vec<usize> {
    let rows = screen(app, 80, 24);
    let area = app.editor_area();
    let doc = app.active_doc().expect("a buffer");
    let gutter = vibe::ui::editor::gutter_width(doc.line_count(), &app.config) as usize;
    let text_x = area.x as usize + gutter;
    rows[area.y as usize + row]
        .chars()
        .enumerate()
        .filter(|(i, c)| *i >= text_x && *c == '│')
        .map(|(i, _)| i - text_x)
        .collect()
}

/// A file with a block, a blank line inside it and a blank line after it.
fn block_file(scratch: &Scratch) -> std::path::PathBuf {
    scratch.file(
        "block.rs",
        "fn f() {\n    let a = 1;\n\n    let b = 2;\n}\n\nfn g() {}\n",
    )
}

#[test]
fn the_rule_runs_from_the_first_line_of_a_block_to_the_last() {
    let scratch = Scratch::new();
    let path = block_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(guides(&mut app, 0), Vec::<usize>::new(), "the header");
    assert_eq!(guides(&mut app, 1), vec![3], "first body line");
    assert_eq!(guides(&mut app, 2), vec![3], "the blank line inside it");
    assert_eq!(guides(&mut app, 3), vec![3], "last body line");
    assert_eq!(
        guides(&mut app, 4),
        Vec::<usize>::new(),
        "the closing brace"
    );
}

#[test]
fn the_rule_stops_at_the_end_of_the_block() {
    // A blank line after the block must not carry the rule: that is the whole
    // difference between "a block" and "a column of dots".
    let scratch = Scratch::new();
    let path = block_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(guides(&mut app, 5), Vec::<usize>::new(), "blank after it");
    assert_eq!(
        guides(&mut app, 6),
        Vec::<usize>::new(),
        "the next function"
    );
}

#[test]
fn a_nested_block_gets_its_own_rule() {
    let scratch = Scratch::new();
    let path = scratch.file(
        "nested.rs",
        "fn f() {\n    if x {\n        y();\n    }\n}\n",
    );
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(guides(&mut app, 1), vec![3], "the outer body");
    assert_eq!(guides(&mut app, 2), vec![3, 7], "and the inner one");
    assert_eq!(guides(&mut app, 3), vec![3], "the inner block is over");
    assert_eq!(
        guides(&mut app, 4),
        Vec::<usize>::new(),
        "and so is the outer"
    );
}

#[test]
fn the_rules_follow_the_files_own_indent_width() {
    // Two-space indentation in a file whose display tab width is four: the
    // rules are two columns apart, because the file says so.
    let scratch = Scratch::new();
    let path = scratch.file("two.py", "def f():\n  if x:\n    return 1\n  return 0\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    assert_eq!(guides(&mut app, 1), vec![1], "one level of two");
    assert_eq!(guides(&mut app, 2), vec![1, 3], "two levels");
    assert_eq!(guides(&mut app, 3), vec![1], "back to one");
}

#[test]
fn the_toggle_turns_the_rules_off_and_on() {
    let scratch = Scratch::new();
    let path = block_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);
    assert!(!guides(&mut app, 1).is_empty(), "on by default");

    app.execute(Command::ToggleIndentGuides);
    let _ = screen(&mut app, 80, 24);
    assert!(guides(&mut app, 1).is_empty(), "the command turns them off");
    assert!(screen_has(&mut app, 80, 24, "Indent guides off"));

    // And the key does the same thing.
    press(
        &mut app,
        KeyCode::Char('g'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    let _ = screen(&mut app, 80, 24);
    assert_eq!(guides(&mut app, 1), vec![3], "the key turns them back on");
}

#[test]
fn the_rule_is_visible_against_the_background() {
    // A rule the colour of the background is not a feature, and that is exactly
    // how this one shipped once. The ratio is a matter of taste; being able to
    // see it is not.
    for theme in [vibe::theme::Theme::dark(), vibe::theme::Theme::light()] {
        let (Some(bg), Some(guide)) = (rgb(theme.bg), rgb(theme.indent_guide)) else {
            panic!("the colours are literal");
        };
        let distance = (bg.0 as i32 - guide.0 as i32).abs()
            + (bg.1 as i32 - guide.1 as i32).abs()
            + (bg.2 as i32 - guide.2 as i32).abs();
        assert!(
            distance >= 90,
            "{}: the guide is {distance} from the background",
            theme.name
        );
    }
}

fn rgb(color: ratatui::style::Color) -> Option<(u8, u8, u8)> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

#[test]
fn a_flat_file_gets_no_rules() {
    let scratch = Scratch::new();
    let path = scratch.file("flat.txt", "one\ntwo\nthree\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let _ = screen(&mut app, 80, 24);

    for row in 0..3 {
        assert!(
            guides(&mut app, row).is_empty(),
            "nothing to mark on row {row}"
        );
    }
}

#[test]
fn a_rule_never_overwrites_a_character() {
    // The rule for a level sits in the last column of that level's own
    // indentation, so the text is always to its right.
    let scratch = Scratch::new();
    let path = scratch.file("tight.rs", "fn f() {\n    x\n}\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    let rows = screen(&mut app, 80, 24);
    let area = app.editor_area();
    let doc = app.active_doc().expect("a buffer");
    let gutter = vibe::ui::editor::gutter_width(doc.line_count(), &app.config) as usize;
    let body = &rows[area.y as usize + 1];
    let text: String = body.chars().skip(area.x as usize + gutter).collect();

    assert_eq!(
        text.trim_end(),
        "   │x",
        "the rule sits just before the text"
    );
}
