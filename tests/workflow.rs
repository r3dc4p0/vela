//! End-to-end workflows: multiple buffers, search and replace, the command
//! palette, quitting, and external-change protection.

mod common;

use common::*;
use crossterm::event::{KeyCode, KeyModifiers};
use vela::commands::Command;

#[test]
fn multiple_buffers_can_be_switched() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "alpha\n");
    let b = scratch.file("b.txt", "beta\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    assert_eq!(app.buffers.len(), 2, "the blank buffer is reused");
    assert_eq!(text(&app), "beta\n");

    ctrl(&mut app, KeyCode::Tab);
    assert_eq!(text(&app), "alpha\n", "Ctrl+Tab cycles backwards");

    ctrl(&mut app, KeyCode::Tab);
    assert_eq!(text(&app), "beta\n", "and wraps around");
}

#[test]
fn reopening_the_same_file_focuses_the_existing_buffer() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "alpha\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &a);
    assert_eq!(app.buffers.len(), 1, "no duplicate buffer");
    assert_eq!(
        app.last_message().unwrap(),
        format!("{} is already open", a.display())
    );
}

#[test]
fn opening_a_missing_path_creates_an_empty_buffer() {
    let scratch = Scratch::new();
    let target = scratch.path().join("does-not-exist.md");
    let mut app = editor(scratch.path());
    open(&mut app, &target);
    assert_eq!(app.buffers.len(), 1);
    assert_eq!(text(&app), "");
    assert_eq!(app.active_doc().unwrap().path().unwrap(), target);

    type_str(&mut app, "# notes");
    ctrl(&mut app, 's');
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# notes");
}

#[test]
fn opening_a_directory_is_reported_not_crashed() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    open(&mut app, scratch.path());
    assert_eq!(app.last_level(), Some(vela::ui::Level::Error));
    assert_eq!(text(&app), "", "the editor keeps a usable buffer");
}

#[test]
fn find_moves_to_the_next_match() {
    let scratch = Scratch::new();
    let path = scratch.file("hay.txt", "one two one two one\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::Home);

    ctrl(&mut app, 'f');
    ctrl(&mut app, 'u'); // clear the word Ctrl+F pre-filled
    for ch in "one".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    // Incremental search already highlights the first match as you type.
    assert_eq!(
        app.active_doc().unwrap().selected_text(),
        "one",
        "the first match is selected while typing"
    );
    assert_eq!(cursor(&app), 3, "and the cursor is at its end");

    plain(&mut app, KeyCode::Enter);
    assert_eq!(cursor(&app), 11, "Enter advances to the next match");
    plain(&mut app, KeyCode::Enter);
    assert_eq!(cursor(&app), 19, "and to the last one");

    plain(&mut app, KeyCode::Enter);
    assert_eq!(cursor(&app), 3, "wrapping back to the first match");

    plain(&mut app, KeyCode::Esc);
    assert!(!app.ui.overlay.captures_input(), "Esc closes the find bar");
}

#[test]
fn find_respects_case_sensitivity_toggle() {
    let scratch = Scratch::new();
    let path = scratch.file("case.txt", "Alpha alpha ALPHA\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::Home);

    ctrl(&mut app, 'f');
    ctrl(&mut app, 'u');
    for ch in "Alpha".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    // Alt+C turns case sensitivity on: only the first match remains.
    alt(&mut app, KeyCode::Char('c'));
    assert_eq!(app.search.matches.len(), 1);
    plain(&mut app, KeyCode::Enter);
    assert_eq!(app.active_doc().unwrap().selected_text(), "Alpha");
    assert_eq!(cursor(&app), 5);

    plain(&mut app, KeyCode::Enter);
    assert_eq!(
        app.active_doc().unwrap().selected_text(),
        "Alpha",
        "a single match wraps onto itself"
    );
}

#[test]
fn replace_all_is_a_single_undo_step() {
    let scratch = Scratch::new();
    let path = scratch.file("rep.txt", "cat cat cat\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    ctrl(&mut app, 'h');
    ctrl(&mut app, 'u');
    for ch in "cat".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    plain(&mut app, KeyCode::Tab); // move focus to the replacement field
    for ch in "dog".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    alt(&mut app, KeyCode::Enter); // Alt+Enter = replace all
    plain(&mut app, KeyCode::Esc);

    assert_eq!(text(&app), "dog dog dog\n");
    ctrl(&mut app, 'z');
    assert_eq!(
        text(&app),
        "cat cat cat\n",
        "replace all must undo as one transaction"
    );
}

#[test]
fn command_palette_runs_the_selected_command() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let before = app.theme.name.clone();

    press(
        &mut app,
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(app.ui.overlay.captures_input(), "the palette is open");

    for ch in "toggle theme".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    plain(&mut app, KeyCode::Enter);
    assert_ne!(app.theme.name, before, "the command actually ran");
    assert!(!app.ui.overlay.captures_input(), "the palette closes");
}

#[test]
fn command_palette_filters_and_ignores_unknown_commands() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.open_palette(vela::ui::PaletteMode::Commands);
    for ch in "zzzzz".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    plain(&mut app, KeyCode::Enter);
    assert!(
        app.ui.overlay.captures_input(),
        "with no matches Enter does nothing and the palette stays open"
    );
    plain(&mut app, KeyCode::Esc);
    assert!(!app.ui.overlay.captures_input());
}

#[test]
fn quitting_with_unsaved_work_requires_confirmation() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "precious");
    ctrl(&mut app, 'q');
    assert!(app.ui.overlay.captures_input(), "a confirmation is shown");
    assert!(app.running, "the editor has not quit yet");

    // Esc cancels.
    plain(&mut app, KeyCode::Esc);
    assert!(app.running);
    assert_eq!(text(&app), "precious", "the work is still there");

    // Ctrl+Q, then Right to "Discard", then Enter.
    ctrl(&mut app, 'q');
    // The default choice is "Cancel"; Left moves to "Discard and quit".
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Enter);
    assert!(!app.running, "discarding quits");
}

#[test]
fn quitting_a_clean_editor_is_immediate() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    ctrl(&mut app, 'q');
    assert!(!app.running);
    assert!(!app.ui.overlay.captures_input());
}

#[test]
fn external_modification_is_detected_before_overwriting() {
    let scratch = Scratch::new();
    let path = scratch.file("shared.txt", "original\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    // Another process rewrites the file with a different length.
    std::fs::write(&path, "changed by someone else\n").unwrap();

    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "mine");
    ctrl(&mut app, 's');
    assert!(
        app.ui.overlay.captures_input(),
        "the overwrite is blocked by a confirmation"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "changed by someone else\n",
        "nothing was written before the user chose"
    );

    // Left, Left from "Cancel" lands on "Overwrite", then Enter.
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Enter);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "original\nmine",
        "the buffer wins once the user confirms"
    );
}

#[test]
fn word_count_reports_statistics() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "three little words");
    app.execute(Command::WordCount);
    let message = app.last_message().unwrap().to_string();
    assert!(message.starts_with("3 words"), "got: {message}");
}

#[test]
fn help_overlay_lists_the_shortcuts() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    plain(&mut app, KeyCode::F(1));
    assert!(app.ui.overlay.captures_input(), "F1 opens help");
    assert!(
        screen_has(&mut app, 100, 40, "Save"),
        "the help screen lists commands and their keys"
    );
    plain(&mut app, KeyCode::Esc);
    assert!(!app.ui.overlay.captures_input());
}

#[test]
fn save_all_writes_every_modified_buffer() {
    let scratch = Scratch::new();
    let a = scratch.file("one.txt", "one\n");
    let b = scratch.file("two.txt", "two\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);

    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "!");
    ctrl(&mut app, KeyCode::Tab);
    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "?");
    assert_eq!(app.unsaved_count(), 2);

    app.execute(Command::SaveAll);
    assert_eq!(app.unsaved_count(), 0);
    // Ctrl+Tab moved the focus from `two.txt` back to `one.txt`.
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\n?");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "two\n!");
}

#[test]
fn closing_a_clean_buffer_keeps_the_editor_usable() {
    let scratch = Scratch::new();
    let a = scratch.file("solo.txt", "solo\n");
    let mut app = editor(scratch.path());
    open(&mut app, &a);
    ctrl(&mut app, 'w');
    assert_eq!(
        app.buffers.len(),
        1,
        "closing the last buffer opens a blank one"
    );
    assert!(app.active_doc().unwrap().path().is_none());
}

#[test]
fn paste_event_inserts_multiline_text() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.handle_event(crossterm::event::Event::Paste(
        "line one\r\nline two\r\n".to_string(),
    ));
    assert_eq!(
        text(&app),
        "line one\nline two\n",
        "CRLF from the clipboard is normalised"
    );
}

#[test]
fn a_failed_save_keeps_the_buffer_dirty_and_reports_the_error() {
    let scratch = Scratch::new();
    let dir = scratch.path().join("locked");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.txt");
    std::fs::write(&path, "original\n").unwrap();

    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, " more");

    // Make the directory unwritable: the atomic save needs to create a temp
    // file next to the target.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    }

    ctrl(&mut app, 's');

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Restore permissions before asserting so the temp dir can be cleaned up.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    assert!(
        modified(&app),
        "a failed save must not clear the dirty flag"
    );
    assert_eq!(app.last_level(), Some(vela::ui::Level::Error));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "original\n",
        "the original file is untouched"
    );
    assert_eq!(text(&app), "original\n more", "the buffer keeps the work");
}

#[test]
fn quitting_refuses_to_discard_when_saving_fails() {
    let scratch = Scratch::new();
    let dir = scratch.path().join("locked2");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.txt");
    std::fs::write(&path, "keep me\n").unwrap();

    let mut app = editor(scratch.path());
    open(&mut app, &path);
    ctrl(&mut app, KeyCode::End);
    type_str(&mut app, "!");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    }

    ctrl(&mut app, 'q');
    // Left twice: "Save all and quit", then Enter.
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Enter);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    assert!(
        app.running,
        "the editor must stay open when it could not save"
    );
    assert_eq!(text(&app), "keep me\n!", "the work is still in the buffer");
    assert_eq!(app.last_level(), Some(vela::ui::Level::Warn));
}
