//! Copying out of the editor.
//!
//! The clipboard itself is tested in `src/clipboard.rs`; these drive it through
//! the real key path — select, `Ctrl+C`, and check what was handed to the
//! terminal — because that is where the sequence has to come from and where a
//! wrong mode would quietly keep the text inside the editor.

mod common;

use common::*;
use crossterm::event::KeyCode;
use vibe::clipboard::ClipboardMode;

/// An editor whose clipboard asks the terminal to do the copy, with no
/// pasteboard involved (the test process must not touch the real one).
fn terminal_clipboard(dir: &std::path::Path) -> vibe::app::App {
    let mut app = editor(dir);
    app.clipboard.set_mode(ClipboardMode::Terminal);
    app
}

#[test]
fn copying_hands_the_terminal_an_osc52_sequence() {
    let scratch = Scratch::new();
    let mut app = terminal_clipboard(scratch.path());
    type_str(&mut app, "hello");
    ctrl(&mut app, 'a');

    ctrl(&mut app, 'c');

    let sequence = app
        .clipboard
        .take_terminal_sequence()
        .expect("the terminal was asked to copy");
    // `OSC 52 ; c ; <base64 of "hello"> ST`
    assert_eq!(sequence, "\x1b]52;c;aGVsbG8=\x1b\\");
    assert!(
        app.clipboard.take_terminal_sequence().is_none(),
        "and asked once: a second frame must not send it again"
    );
    assert_eq!(
        app.clipboard.internal(),
        "hello",
        "the editor keeps its own copy too"
    );
}

#[test]
fn a_copy_that_cannot_leave_says_so() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "hello");
    ctrl(&mut app, 'a');

    ctrl(&mut app, 'c');

    assert!(
        app.clipboard.take_terminal_sequence().is_none(),
        "the headless clipboard never touches anything outside the process"
    );
    assert!(
        screen_has(&mut app, 100, 20, "inside the editor only"),
        "and the message does not claim more than it did"
    );
}

#[test]
fn copying_in_terminal_mode_says_plain_copied() {
    let scratch = Scratch::new();
    let mut app = terminal_clipboard(scratch.path());
    type_str(&mut app, "hello");
    ctrl(&mut app, 'a');
    ctrl(&mut app, 'c');

    let rows = screen(&mut app, 100, 20);
    assert!(
        rows.iter().any(|r| r.contains("Copied 5 characters")),
        "the count is reported:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows.iter().any(|r| r.contains("inside the editor only")),
        "and not qualified, because it did leave"
    );
}

#[test]
fn cutting_also_reaches_the_terminal() {
    let scratch = Scratch::new();
    let mut app = terminal_clipboard(scratch.path());
    type_str(&mut app, "abc");
    ctrl(&mut app, 'a');

    ctrl(&mut app, 'x');

    assert_eq!(text(&app), "", "the text was cut");
    let sequence = app
        .clipboard
        .take_terminal_sequence()
        .expect("the cut text went to the terminal");
    assert!(sequence.contains("YWJj"), "base64 of \"abc\": {sequence}");
}

#[test]
fn cutting_a_line_without_a_selection_also_copies_it() {
    let scratch = Scratch::new();
    let mut app = terminal_clipboard(scratch.path());
    type_str(&mut app, "one\ntwo");

    ctrl(&mut app, 'x');

    // No selection means "the line the caret is on", which is `two` here.
    let sequence = app
        .clipboard
        .take_terminal_sequence()
        .expect("the line went to the terminal");
    assert!(
        sequence.contains("dHdvCg=="),
        "base64 of \"two\\n\": {sequence}"
    );
}

#[test]
fn copying_with_no_selection_copies_the_line() {
    // The same rule `Ctrl+C` has always had: a caret with nothing selected
    // copies its line, and that line now leaves the editor like any other.
    let scratch = Scratch::new();
    let mut app = terminal_clipboard(scratch.path());
    type_str(&mut app, "one\ntwo");
    ctrl(&mut app, KeyCode::Home);

    ctrl(&mut app, 'c');

    let sequence = app
        .clipboard
        .take_terminal_sequence()
        .expect("the line went to the terminal");
    assert!(
        sequence.contains("b25lCg=="),
        "base64 of \"one\\n\": {sequence}"
    );
}

#[test]
fn the_setting_chooses_the_channel() {
    use vibe::config::Config;
    let mut config = Config::default();
    assert_eq!(config.clipboard_mode(), ClipboardMode::Auto, "the default");

    config.editor.clipboard = "terminal".to_string();
    assert_eq!(config.clipboard_mode(), ClipboardMode::Terminal);

    config.editor.clipboard = "internal".to_string();
    assert_eq!(config.clipboard_mode(), ClipboardMode::Internal);

    // A typo costs the setting, not the whole configuration.
    config.editor.clipboard = "pasteboard".to_string();
    assert_eq!(config.clipboard_mode(), ClipboardMode::Auto);
}
