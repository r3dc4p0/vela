//! Indent detection, driven through the real input path.
//!
//! What is asserted is what a user gets: a file is opened, keys are pressed, and
//! the text that lands is the file's own indentation rather than the setting's.
//! The detection rules themselves are unit-tested in `src/indent.rs`.

mod common;

use common::*;
use crossterm::event::KeyCode;

/// A file with a two-space body, opened in an editor whose settings say four.
fn two_space_file(scratch: &Scratch) -> std::path::PathBuf {
    scratch.file("two.py", "def f():\n  if x:\n    return 1\n  return 0\n")
}

fn tab_file(scratch: &Scratch) -> std::path::PathBuf {
    scratch.file("tab.go", "func main() {\n\tif x {\n\t\tgo()\n\t}\n}\n")
}

#[test]
fn a_file_written_with_two_spaces_indents_with_two() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    // The caret starts at the top of the file: one Tab, one indent level.
    plain(&mut app, KeyCode::Tab);
    assert_eq!(
        app.active_doc().expect("a buffer").line_text(0),
        "  def f():",
        "the file says two spaces"
    );
}

#[test]
fn a_file_written_with_tabs_indents_with_a_tab() {
    let scratch = Scratch::new();
    let path = tab_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    plain(&mut app, KeyCode::Tab);
    assert_eq!(
        app.active_doc().expect("a buffer").line_text(0),
        "\tfunc main() {"
    );
}

#[test]
fn the_file_beats_the_settings() {
    // The setting is what a new buffer uses; a file that says otherwise wins.
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor_with(scratch.path(), |c| {
        c.editor.tab_width = 8;
        c.editor.use_spaces = true;
    });
    open(&mut app, &path);

    assert_eq!(app.indent_style().width, 2);
    assert!(app.indent_style().use_spaces);
}

#[test]
fn a_new_buffer_uses_the_settings() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| {
        c.editor.tab_width = 3;
        c.editor.use_spaces = true;
    });

    assert_eq!(app.indent_style().width, 3, "nothing to detect in a blank");
    plain(&mut app, KeyCode::Tab);
    assert_eq!(text(&app), "   ");
}

#[test]
fn detection_can_be_turned_off() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor_with(scratch.path(), |c| {
        c.editor.detect_indent = false;
        c.editor.tab_width = 4;
        c.editor.use_spaces = true;
    });
    open(&mut app, &path);

    assert_eq!(app.indent_style().width, 4, "the setting, not the file");
    plain(&mut app, KeyCode::Tab);
    assert_eq!(
        app.active_doc().expect("a buffer").line_text(0),
        "    def f():"
    );
}

#[test]
fn a_file_with_no_indentation_uses_the_settings() {
    let scratch = Scratch::new();
    let path = scratch.file("flat.txt", "one\ntwo\nthree\n");
    let mut app = editor_with(scratch.path(), |c| c.editor.tab_width = 2);
    open(&mut app, &path);

    assert_eq!(app.indent_style().width, 2);
    assert!(app.active_doc().expect("a buffer").indent_style().is_none());
}

#[test]
fn the_status_bar_names_the_style() {
    let scratch = Scratch::new();
    let two = two_space_file(&scratch);
    let tabbed = tab_file(&scratch);

    let mut app = editor(scratch.path());
    open(&mut app, &two);
    assert!(
        screen_has(&mut app, 100, 24, "Spaces: 2"),
        "the bar says what Tab will do: {:?}",
        screen(&mut app, 100, 24)
    );

    let mut app = editor(scratch.path());
    open(&mut app, &tabbed);
    assert!(screen_has(&mut app, 100, 24, "Tabs"));
}

#[test]
fn enter_after_a_brace_opens_the_block_at_the_files_width() {
    // Auto-indent and the pair expansion both take their unit from the buffer,
    // so a two-space file opens its blocks two columns in.
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "{");
    plain(&mut app, KeyCode::Enter);

    let doc = app.active_doc().expect("a buffer");
    assert_eq!(doc.line_text(4), "{");
    assert_eq!(doc.line_text(5), "  ", "one level, two columns");
    assert_eq!(doc.line_text(6), "}", "and the closer on its own line");
}

#[test]
fn dedent_removes_the_files_width() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    // Line 2 is indented by four — two levels of two.
    plain(&mut app, KeyCode::Down);
    plain(&mut app, KeyCode::Down);
    assert_eq!(
        app.active_doc().expect("a buffer").line_text(2),
        "    return 1"
    );

    shift(&mut app, KeyCode::Tab);
    assert_eq!(
        app.active_doc().expect("a buffer").line_text(2),
        "  return 1",
        "one level came off, and a level here is two"
    );
}

#[test]
fn indent_and_dedent_agree_with_the_file() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    // Select the whole file and indent it: every line gains two spaces.
    ctrl(&mut app, 'a');
    app.execute(vela::commands::Command::Indent);
    let doc = app.active_doc().expect("a buffer");
    assert_eq!(doc.line_text(0), "  def f():");

    // And back off again, by the same amount.
    ctrl(&mut app, 'a');
    app.execute(vela::commands::Command::Dedent);
    let doc = app.active_doc().expect("a buffer");
    assert_eq!(doc.line_text(0), "def f():");
}

#[test]
fn two_files_in_one_session_keep_their_own_styles() {
    // The style belongs to the buffer, not to the session: switching tabs must
    // switch what Tab inserts.
    let scratch = Scratch::new();
    let spaces = two_space_file(&scratch);
    let tabs = tab_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &spaces);
    open(&mut app, &tabs);

    assert!(!app.indent_style().use_spaces, "the tab file is in front");

    app.focus_buffer(0);
    assert!(app.indent_style().use_spaces, "and now the space file");
    assert_eq!(app.indent_style().width, 2);
}
