//! Integration coverage for the features that had none.
//!
//! The rest of the suite covers editing, search, panes, the UI and recovery.
//! These are the paths a user reaches from the menus, the palette, the file
//! system and the configuration file — the ones that break quietly, because
//! they are not on the path a developer types through every day.

mod common;

use std::path::Path;

use common::*;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;
use vibe::commands::Command;
use vibe::filesystem::{Encoding, Eol};

/// An application that persists into `data`, like the recovery tests use.
fn persistent(dir: &Path, data: &Path) -> vibe::app::App {
    let mut app = editor(dir);
    app.persist = true;
    app.data_dir = data.to_path_buf();
    app
}

fn has_overlay(app: &vibe::app::App) -> bool {
    app.ui.overlay.captures_input()
}

// ---------------------------------------------------------------- files

#[test]
fn reload_replaces_the_buffer_with_what_is_on_disk() {
    let scratch = Scratch::new();
    let path = scratch.file("notes.txt", "before\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    std::fs::write(&path, "after\n").unwrap();
    app.execute(Command::Reload);

    assert_eq!(text(&app), "after\n");
    assert!(!app.buffers[0].is_modified());
}

#[test]
fn a_modified_buffer_asks_before_reloading() {
    // Reload is destructive: the confirmation is the whole feature.
    let scratch = Scratch::new();
    let path = scratch.file("notes.txt", "before\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    type_str(&mut app, "edit");

    app.execute(Command::Reload);
    assert!(has_overlay(&app), "a confirmation is up");

    // The default selection is Cancel, so Enter alone would keep the edit.
    // `y` picks the destructive choice, which is the one to test.
    plain(&mut app, KeyCode::Char('y'));
    assert_eq!(text(&app), "before\n", "the edit is gone");
}

#[test]
fn recent_files_are_newest_first_and_survive_a_restart() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");

    let mut app = persistent(scratch.path(), &data);
    open(&mut app, &a);
    open(&mut app, &b);
    assert_eq!(app.recent[0], b, "the newest is first");

    app.shutdown();

    let mut next = persistent(scratch.path(), &data);
    next.load_recent();
    assert_eq!(
        next.recent,
        vec![b, a],
        "and it came back in the same order"
    );
}

#[test]
fn session_restore_reopens_the_buffers_with_their_positions() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "one\ntwo\nthree\nfour\n");

    let mut app = persistent(scratch.path(), &data);
    open(&mut app, &path);
    plain(&mut app, KeyCode::Down);
    plain(&mut app, KeyCode::Down);
    app.save_session();

    let mut next = persistent(scratch.path(), &data);
    assert!(next.restore_session(), "a session was restored");
    assert_eq!(next.buffers.len(), 1);
    assert_eq!(next.buffers[0].cursor_line_col(), (2, 0));
}

#[test]
fn session_restore_skips_files_that_no_longer_exist() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("gone.txt", "text\n");

    let mut app = persistent(scratch.path(), &data);
    open(&mut app, &path);
    app.save_session();
    std::fs::remove_file(&path).unwrap();

    let mut next = persistent(scratch.path(), &data);
    assert!(!next.restore_session(), "nothing to restore");
}

#[test]
fn a_binary_file_is_refused_rather_than_mangled() {
    let scratch = Scratch::new();
    let path = scratch.path().join("image.png");
    std::fs::write(&path, [0x89u8, 0x50, 0x4e, 0x47, 0x00, 0x01, 0x02]).unwrap();

    let mut app = editor(scratch.path());
    open(&mut app, &path);

    assert!(
        app.buffers[0].text().is_empty(),
        "no binary content in a buffer"
    );
    let rows = screen(&mut app, 100, 24);
    assert!(
        rows.iter()
            .any(|r| r.contains("encoding") || r.contains("UTF")),
        "the refusal is explained: {rows:?}"
    );
}

// ------------------------------------------------------------- configuration

#[test]
fn a_key_override_binds_a_new_chord() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| {
        // Take the palette's own chord and give it to a different command:
        // that is the strongest evidence that rebinding actually works.
        c.keys.insert(
            "ctrl+shift+p".to_string(),
            "edit.toggleAutoPairs".to_string(),
        );
    });

    press(
        &mut app,
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(
        !app.config.editor.auto_pairs,
        "the override ran the command"
    );
    assert!(!has_overlay(&app), "and the palette did not open");
}

#[test]
fn a_colour_override_reaches_the_theme() {
    let scratch = Scratch::new();
    let app = editor_with(scratch.path(), |c| {
        c.colors.insert("bg".to_string(), "#010203".to_string());
        c.colors.insert("mark".to_string(), "#8f8ce7".to_string());
    });

    assert_eq!(app.theme.bg, Color::Rgb(1, 2, 3));
    assert_eq!(app.theme.mark, Color::Rgb(0x8f, 0x8c, 0xe7));
}

// ------------------------------------------------------------------ palette

#[test]
fn quick_open_finds_and_opens_a_file() {
    let scratch = Scratch::new();
    scratch.file("one.txt", "one\n");
    let wanted = scratch.file("two.txt", "two\n");
    let mut app = editor(scratch.path());

    app.execute(Command::QuickOpen);
    // The index is built on a worker; wait for it the way the loader tests do.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.file_index.is_none() {
        assert!(std::time::Instant::now() < deadline, "index never arrived");
        app.poll_jobs();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    type_str(&mut app, "two");
    assert!(
        screen_has(&mut app, 100, 30, "two.txt"),
        "the file is offered: {:?}",
        screen(&mut app, 100, 30)
    );
    plain(&mut app, KeyCode::Enter);
    assert_eq!(
        app.buffers[app.active()].path().map(|p| p.to_path_buf()),
        Some(wanted)
    );
}

// ------------------------------------------------------------------- editing

#[test]
fn trim_trailing_whitespace_cleans_every_line() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "one   \ntwo\t\nthree");

    app.execute(Command::TrimTrailingWhitespace);
    assert_eq!(text(&app), "one\ntwo\nthree");
}

#[test]
fn a_new_buffer_is_an_untitled_scratch() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    let path = scratch.file("notes.txt", "hello\n");
    open(&mut app, &path);

    app.new_buffer();
    let doc = &app.buffers[app.active()];
    assert!(doc.path().is_none(), "no path");
    assert!(doc.text().is_empty(), "and nothing in it");
    assert_eq!(doc.language().name(), "Plain Text");
}

#[test]
fn replace_all_expands_backreferences() {
    // `Alt+R` turns on regex in the replace bar, and `$1`/`$2` refer to the
    // capture groups. The round trip is what makes the feature useful.
    let scratch = Scratch::new();
    let path = scratch.file("pairs.txt", "alpha=beta\ngamma=delta\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    ctrl(&mut app, 'h');
    ctrl(&mut app, 'u');
    type_str(&mut app, r"(\w+)=(\w+)");
    alt(&mut app, KeyCode::Char('r')); // regex
    plain(&mut app, KeyCode::Tab); // replacement field
    type_str(&mut app, "$2=$1");
    alt(&mut app, KeyCode::Enter);
    plain(&mut app, KeyCode::Esc);

    assert_eq!(text(&app), "beta=alpha\ndelta=gamma\n");
}

#[test]
fn a_whole_word_query_does_not_match_inside_a_word() {
    let scratch = Scratch::new();
    let path = scratch.file("words.txt", "cat concatenate cat\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);

    ctrl(&mut app, 'f');
    ctrl(&mut app, 'u');
    type_str(&mut app, "cat");
    assert_eq!(app.search.ranges().len(), 3, "three substrings before");

    alt(&mut app, KeyCode::Char('w')); // whole word
    assert_eq!(app.search.ranges().len(), 2, "only the two standalone cats");
}

// --------------------------------------------------------------- encodings

#[test]
fn a_utf16_file_is_decoded_and_saved_back() {
    let scratch = Scratch::new();
    let path = scratch.path().join("utf16.txt");
    let mut bytes = vec![0xffu8, 0xfe];
    bytes.extend("hello\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
    std::fs::write(&path, bytes).unwrap();

    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert_eq!(text(&app), "hello\n");
    assert_eq!(app.buffers[0].encoding(), Encoding::Utf16Le);

    app.save_active(false);
    let written = std::fs::read(&path).unwrap();
    assert_eq!(&written[..2], &[0xff, 0xfe], "the BOM is preserved");
}

#[test]
fn a_file_with_cr_endings_round_trips() {
    let scratch = Scratch::new();
    let path = scratch.path().join("mac.txt");
    std::fs::write(&path, b"one\rtwo\r").unwrap();

    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert_eq!(app.buffers[0].eol(), Eol::Cr);
    assert_eq!(text(&app), "one\ntwo\n");

    app.save_active(false);
    assert_eq!(std::fs::read(&path).unwrap(), b"one\rtwo\r");
}

// ------------------------------------------------------------------ display

#[test]
fn line_numbers_can_be_toggled() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "one\ntwo");

    app.execute(Command::ToggleLineNumbers);
    assert!(!app.config.editor.line_numbers);
    let rows = screen(&mut app, 80, 24);
    let body = rows[2].trim_end();
    assert!(!body.starts_with(' '), "no gutter to indent by: {body:?}");
}

#[test]
fn whitespace_can_be_made_visible() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "a b\tc");

    app.execute(Command::ToggleWhitespace);
    assert!(app.config.editor.show_whitespace);
    let rows = screen(&mut app, 80, 24);
    assert!(
        rows.iter().any(|r| r.contains('·') || r.contains('→')),
        "the glyphs appear: {rows:?}"
    );
}

#[test]
fn the_cursor_line_highlight_can_be_toggled() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    app.execute(Command::ToggleCursorLine);
    assert!(!app.config.editor.highlight_cursor_line);
    assert!(screen_has(&mut app, 80, 24, "Cursor line"));
}

#[test]
fn the_hint_bar_can_be_hidden() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    app.execute(Command::ToggleHints);
    assert!(!app.config.ui.show_hints);
    let rows = screen(&mut app, 80, 24);
    assert!(
        !rows.iter().any(|r| r.contains("Ctrl+s save")),
        "the hints are gone: {rows:?}"
    );
}

#[test]
fn syntax_highlighting_can_be_toggled() {
    let scratch = Scratch::new();
    let path = scratch.file("a.rs", "fn main() {}\n");
    let mut app = editor(scratch.path());
    open(&mut app, &path);
    assert_eq!(app.buffers[0].language().name(), "Rust");

    app.execute(Command::ToggleSyntax);
    assert!(!app.config.editor.syntax_highlighting);
    assert!(screen_has(&mut app, 80, 24, "highlighting"));
}

#[test]
fn the_language_is_chosen_from_the_extension() {
    let scratch = Scratch::new();
    let rs = scratch.file("a.rs", "fn main() {}\n");
    let py = scratch.file("b.py", "def f():\n    pass\n");
    let mut app = editor(scratch.path());

    open(&mut app, &rs);
    assert_eq!(app.buffers[0].language().name(), "Rust");
    open(&mut app, &py);
    assert_eq!(app.buffers[app.active()].language().name(), "Python");
}

// ------------------------------------------------------------------ statistics

#[test]
fn document_statistics_are_reported() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "one two three\nfour five");

    app.execute(Command::ShowStats);
    let rows = screen(&mut app, 100, 24);
    assert!(
        rows.iter()
            .any(|r| r.contains("lines") || r.contains("words")),
        "the counts are shown: {rows:?}"
    );
}
