//! Indent detection, driven through the real input path.
//!
//! What is asserted is what a user gets: a file is opened, keys are pressed, and
//! the text that lands is the file's own indentation rather than the setting's.
//! The detection rules themselves are unit-tested in `src/indent.rs`.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyModifiers};
use vibe::commands::Command;

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
    app.execute(vibe::commands::Command::Indent);
    let doc = app.active_doc().expect("a buffer");
    assert_eq!(doc.line_text(0), "  def f():");

    // And back off again, by the same amount.
    ctrl(&mut app, 'a');
    app.execute(vibe::commands::Command::Dedent);
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

// ------------------------------------------------------- changing the style

/// Open the indentation prompt the way a user does.
fn indent_prompt(app: &mut vibe::app::App) {
    press(
        app,
        KeyCode::Char('i'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
}

/// What the prompt is seeded with, when it is open.
fn prompt_text(app: &vibe::app::App) -> Option<String> {
    match &app.ui.overlay {
        vibe::ui::Overlay::Indent(input) => Some(input.value().to_string()),
        _ => None,
    }
}

#[test]
fn the_prompt_is_seeded_with_the_style_in_force() {
    let scratch = Scratch::new();
    let spaces = two_space_file(&scratch);
    let tabs = tab_file(&scratch);

    let mut app = editor(scratch.path());
    open(&mut app, &spaces);
    indent_prompt(&mut app);
    assert_eq!(prompt_text(&app).as_deref(), Some("2"));

    let mut app = editor(scratch.path());
    open(&mut app, &tabs);
    indent_prompt(&mut app);
    assert_eq!(prompt_text(&app).as_deref(), Some("tab"));
}

#[test]
fn the_prompt_changes_the_width_of_this_buffer() {
    let scratch = Scratch::new();
    let path = tab_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert!(!app.indent_style().use_spaces, "a tab file to begin with");

    indent_prompt(&mut app);
    ctrl(&mut app, 'u');
    type_str(&mut app, "3");
    plain(&mut app, KeyCode::Enter);

    assert!(app.indent_style().use_spaces);
    assert_eq!(app.indent_style().width, 3);
    assert!(screen_has(&mut app, 100, 24, "Spaces: 3"));

    // And Tab now inserts what was asked for.
    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Tab);
    assert_eq!(app.active_doc().unwrap().line_text(0), "   func main() {");
}

#[test]
fn the_prompt_can_switch_a_file_to_tabs() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    indent_prompt(&mut app);
    ctrl(&mut app, 'u');
    type_str(&mut app, "tab");
    plain(&mut app, KeyCode::Enter);

    assert!(!app.indent_style().use_spaces);
    assert!(screen_has(&mut app, 100, 24, "Tabs"));

    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Tab);
    assert_eq!(app.active_doc().unwrap().line_text(0), "\tdef f():");
}

#[test]
fn a_bad_answer_keeps_the_prompt_open() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    indent_prompt(&mut app);
    ctrl(&mut app, 'u');
    type_str(&mut app, "wide");
    plain(&mut app, KeyCode::Enter);

    assert!(prompt_text(&app).is_some(), "still asking");
    assert!(screen_has(&mut app, 100, 24, "Not an indentation"));
    assert_eq!(app.indent_style().width, 2, "and nothing changed");
}

#[test]
fn escape_leaves_the_style_alone() {
    let scratch = Scratch::new();
    let path = two_space_file(&scratch);
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    indent_prompt(&mut app);
    ctrl(&mut app, 'u');
    type_str(&mut app, "8");
    plain(&mut app, KeyCode::Esc);

    assert!(prompt_text(&app).is_none(), "the prompt closed");
    assert_eq!(app.indent_style().width, 2, "and the file kept its own");
}

#[test]
fn the_prompt_is_a_command_like_any_other() {
    assert_eq!(Command::SetIndentation.id(), "edit.setIndentation");
    assert!(Command::SetIndentation
        .default_keys()
        .contains(&"ctrl+shift+i"));

    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.open_palette(vibe::ui::PaletteMode::Commands);
    type_str(&mut app, "indentation");
    assert!(screen_has(&mut app, 100, 30, "Set Indentation"));
}
