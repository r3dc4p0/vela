//! Integration tests for the editing surface: every test drives the real
//! `App` through `handle_event`, so the key translation, command dispatch and
//! document mutation layers are all exercised together.

mod common;

use common::*;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;

#[test]
fn typing_inserts_at_the_cursor() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "hello");
    assert_eq!(text(&app), "hello");
    assert_eq!(cursor(&app), 5);
    assert!(modified(&app), "typing must mark the buffer dirty");
}

#[test]
fn enter_at_the_end_of_a_file_lands_on_the_new_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abc");
    plain(&mut app, KeyCode::Enter);

    assert_eq!(text(&app), "abc\n");
    assert_eq!(cursor(&app), 4);
    assert_eq!(
        app.buffers[0].cursor_line_col(),
        (1, 0),
        "a cursor just past the newline belongs to the new, empty line"
    );

    // And what is typed next must land on that line, not the previous one.
    type_str(&mut app, "x");
    assert_eq!(text(&app), "abc\nx");
}

#[test]
fn enter_splits_a_line_and_keeps_the_indentation() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "    indent");
    plain(&mut app, KeyCode::Enter);
    type_str(&mut app, "next");
    assert_eq!(text(&app), "    indent\n    next");
}

#[test]
fn auto_indent_can_be_disabled() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| c.editor.auto_indent = false);
    type_str(&mut app, "    x");
    plain(&mut app, KeyCode::Enter);
    type_str(&mut app, "y");
    assert_eq!(text(&app), "    x\ny");
}

#[test]
fn backspace_joins_lines() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "one\ntwo");
    // Cursor is at the end of "two"; go to the start of the line and delete the
    // newline.
    plain(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "onetwo");
}

#[test]
fn delete_forward_removes_the_next_character() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abc");
    plain(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Delete);
    assert_eq!(text(&app), "bc");
}

#[test]
fn undo_and_redo_round_trip() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "alpha");
    assert_eq!(text(&app), "alpha");

    ctrl(&mut app, 'z');
    assert_eq!(
        text(&app),
        "",
        "one undo must remove the coalesced typing run"
    );

    ctrl(&mut app, 'y');
    assert_eq!(text(&app), "alpha", "redo must restore the same text");
    assert_eq!(cursor(&app), 5, "redo must restore the cursor position");
}

#[test]
fn undo_restores_the_cursor_position() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abc");
    plain(&mut app, KeyCode::Home);
    type_str(&mut app, "X");
    assert_eq!(text(&app), "Xabc");
    assert_eq!(cursor(&app), 1);

    ctrl(&mut app, 'z');
    assert_eq!(text(&app), "abc");
    assert_eq!(
        cursor(&app),
        0,
        "undo must put the cursor back where it was"
    );
}

#[test]
fn select_all_then_typing_replaces_everything() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "old contents");
    ctrl(&mut app, 'a');
    assert!(app.active_doc().unwrap().has_selection());
    type_str(&mut app, "new");
    assert_eq!(text(&app), "new");
}

#[test]
fn shift_selection_then_delete() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abcdef");
    for _ in 0..3 {
        shift(&mut app, KeyCode::Left);
    }
    assert_eq!(
        app.active_doc().unwrap().selected_text(),
        "def",
        "shift+left must extend the selection"
    );
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "abc");
}

#[test]
fn word_motion_walks_word_boundaries() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "alpha beta gamma");
    ctrl(&mut app, KeyCode::Home);
    assert_eq!(cursor(&app), 0);

    ctrl(&mut app, KeyCode::Right);
    assert_eq!(cursor(&app), 6, "Ctrl+Right stops at the next word start");
    ctrl(&mut app, KeyCode::Right);
    assert_eq!(cursor(&app), 11);
    ctrl(&mut app, KeyCode::Left);
    assert_eq!(cursor(&app), 6, "Ctrl+Left returns to the previous word");
}

#[test]
fn home_and_end_move_within_the_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "first\nsecond");
    plain(&mut app, KeyCode::Home);
    assert_eq!(cursor_1(&app), (2, 1));
    plain(&mut app, KeyCode::End);
    assert_eq!(cursor_1(&app), (2, 7));
    ctrl(&mut app, KeyCode::Home);
    assert_eq!(
        cursor_1(&app),
        (1, 1),
        "Ctrl+Home goes to the document start"
    );
    ctrl(&mut app, KeyCode::End);
    assert_eq!(cursor_1(&app), (2, 7), "Ctrl+End goes to the document end");
}

#[test]
fn copy_and_paste_round_trip() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "copy me");
    for _ in 0..7 {
        shift(&mut app, KeyCode::Left);
    }
    ctrl(&mut app, 'c');
    ctrl(&mut app, KeyCode::End);
    ctrl(&mut app, 'v');
    assert_eq!(text(&app), "copy mecopy me");
}

#[test]
fn cut_removes_the_selection() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "abcdef");
    for _ in 0..3 {
        shift(&mut app, KeyCode::Left);
    }
    ctrl(&mut app, 'x');
    assert_eq!(text(&app), "abc");
    ctrl(&mut app, 'v');
    assert_eq!(text(&app), "abcdef");
}

#[test]
fn unicode_editing_is_character_based() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "héllo 世界");
    assert_eq!(cursor(&app), 8, "char index, not byte index");
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "héllo 世");
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "héllo世", "the space before the wide char goes");
    // The two deletions are not adjacent in the buffer, so they stay two
    // separate undo steps.
    ctrl(&mut app, 'z');
    assert_eq!(text(&app), "héllo 世");
    ctrl(&mut app, 'z');
    assert_eq!(text(&app), "héllo 世界");
}

#[test]
fn tab_inserts_spaces_when_configured() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    plain(&mut app, KeyCode::Tab);
    assert_eq!(text(&app), "    ", "use_spaces is on by default");
}

#[test]
fn tab_indents_a_selection() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "a\nb");
    ctrl(&mut app, 'a');
    plain(&mut app, KeyCode::Tab);
    assert_eq!(text(&app), "    a\n    b");
    shift(&mut app, KeyCode::Tab);
    assert_eq!(text(&app), "a\nb", "Shift+Tab dedents");
}

#[test]
fn save_writes_the_file_and_clears_the_dirty_flag() {
    let scratch = Scratch::new();
    let path = scratch.file("notes.txt", "start\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert!(!modified(&app), "a freshly opened file is clean");

    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "added");
    assert!(modified(&app));

    ctrl(&mut app, 's');
    assert!(
        !modified(&app),
        "a successful save must clear the dirty flag"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "start\nadded",
        "the bytes on disk must match the buffer"
    );
}

#[test]
fn save_preserves_crlf_line_endings() {
    let scratch = Scratch::new();
    let path = scratch.file("dos.txt", "one\r\ntwo\r\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert_eq!(app.active_doc().unwrap().eol(), vela::filesystem::Eol::CrLf);

    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Down);
    plain(&mut app, KeyCode::End);
    type_str(&mut app, "!");
    ctrl(&mut app, 's');

    let bytes = std::fs::read(&path).unwrap();
    let body = String::from_utf8(bytes).unwrap();
    assert_eq!(body, "one\r\ntwo!\r\n", "CRLF must survive a round trip");
}

#[test]
fn undoing_back_to_the_save_point_clears_the_dirty_flag() {
    let scratch = Scratch::new();
    let path = scratch.file("dirty.txt", "base\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, " more");
    ctrl(&mut app, 's');
    assert!(!modified(&app));

    type_str(&mut app, " and more");
    assert!(
        modified(&app),
        "editing after a save makes the buffer dirty"
    );
    ctrl(&mut app, 'z');
    assert!(
        !modified(&app),
        "undoing back to the saved revision must be clean again"
    );
}

#[test]
fn save_as_prompt_writes_a_new_file() {
    let scratch = Scratch::new();
    let target = scratch.path().join("brand-new.txt");
    let mut app = editor(scratch.path());
    type_str(&mut app, "fresh content");
    save_as(&mut app, &target);

    assert!(target.exists(), "Save As must create the file");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "fresh content");
    assert!(!modified(&app));
    assert_eq!(
        app.active_doc().unwrap().path().unwrap(),
        target,
        "the buffer must adopt the new path"
    );
}

#[test]
fn close_with_unsaved_changes_asks_before_discarding() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "unsaved work");
    ctrl(&mut app, 'w');
    assert!(
        app.ui.overlay.captures_input(),
        "closing a dirty buffer must open a confirmation"
    );
    // Escape cancels: the buffer and its contents survive.
    plain(&mut app, KeyCode::Esc);
    assert_eq!(app.buffers.len(), 1);
    assert_eq!(text(&app), "unsaved work");
}

#[test]
fn toggle_comment_uses_the_language_syntax() {
    let scratch = Scratch::new();
    let path = scratch.file("code.rs", "fn main() {}\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    app.execute(vela::commands::Command::ToggleComment);
    assert_eq!(text(&app), "// fn main() {}\n");
    app.execute(vela::commands::Command::ToggleComment);
    assert_eq!(text(&app), "fn main() {}\n");
}

#[test]
fn duplicate_line_copies_the_current_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "a\nb\nc");
    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Down);
    app.execute(vela::commands::Command::DuplicateLine);
    assert_eq!(text(&app), "a\nb\nb\nc");
    assert_eq!(cursor_1(&app).0, 3, "the cursor stays on the duplicate");
}

#[test]
fn move_line_up_and_down() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "a\nb\nc");
    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Down);
    alt(&mut app, KeyCode::Up);
    assert_eq!(text(&app), "b\na\nc", "Alt+Up moves the line up");
    assert_eq!(cursor_1(&app).0, 1, "the cursor follows the line");
    alt(&mut app, KeyCode::Down);
    assert_eq!(text(&app), "a\nb\nc", "Alt+Down moves it back");
}

#[test]
fn delete_line_removes_the_whole_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "a\nb\nc");
    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Down);
    app.execute(vela::commands::Command::DeleteLine);
    assert_eq!(text(&app), "a\nc");
}

#[test]
fn goto_line_jumps_to_the_requested_position() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "1\n2\n3\n4\n5");
    ctrl(&mut app, 'g');
    for ch in "4:2".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    plain(&mut app, KeyCode::Enter);
    assert_eq!(cursor_1(&app), (4, 2));
}
