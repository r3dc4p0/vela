//! Auto-closing brackets and quotes, driven through the real input path.
//!
//! Every test types with key events, so what is asserted is what a user gets:
//! the character goes through `input::translate`, then `App::type_text`, then
//! the document. The rules themselves are unit-tested in `src/autopair.rs`; what
//! these cover is the wiring — that typing reaches them, that a paste does not,
//! and that the caret ends up where the next keystroke expects it.

mod common;

use common::*;
use crossterm::event::{Event, KeyCode, KeyModifiers};
use vibe::commands::Command;

/// The buffer's text with the caret marked as `|`, which is the only way to
/// assert on a caret that sits inside a pair.
fn marked(app: &vibe::app::App) -> String {
    let doc = app.active_doc().expect("a buffer");
    let mut out = doc.text();
    let at = doc.cursor();
    if at <= out.chars().count() {
        let byte = out
            .char_indices()
            .nth(at)
            .map(|(i, _)| i)
            .unwrap_or(out.len());
        out.insert(byte, '|');
    }
    out
}

fn app_with(scratch: &Scratch, body: &str) -> vibe::app::App {
    let mut app = editor(scratch.path());
    type_str(&mut app, body);
    app
}

// --------------------------------------------------------------- openers

#[test]
fn an_opener_completes_itself_and_leaves_the_caret_inside() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "(");
    assert_eq!(marked(&app), "(|)");

    // The caret is inside, so the next character lands between the two.
    type_str(&mut app, "a");
    assert_eq!(marked(&app), "(a|)");
}

#[test]
fn every_bracket_kind_pairs() {
    for (typed, expected) in [('(', "(|)"), ('[', "[|]"), ('{', "{|}")] {
        let scratch = Scratch::new();
        let mut app = editor(scratch.path());
        type_str(&mut app, &typed.to_string());
        assert_eq!(marked(&app), expected);
    }
}

#[test]
fn a_closer_that_is_already_there_is_stepped_over_instead_of_doubled() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "(");
    // The caret sits before the `)` that the opener wrote: typing it now
    // should move past it, not produce `())`.
    type_str(&mut app, ")");
    assert_eq!(text(&app), "()");
    assert_eq!(marked(&app), "()|");
}

#[test]
fn a_closer_with_nothing_to_close_is_just_a_character() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "abc");

    type_str(&mut app, ")");
    assert_eq!(text(&app), "abc)");
}

// ---------------------------------------------------------------- quotes

#[test]
fn a_quote_after_a_space_starts_a_string() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "let x = ");

    type_str(&mut app, "\"");
    assert_eq!(marked(&app), "let x = \"|\"");
}

#[test]
fn a_quote_inside_a_word_is_punctuation() {
    // `don't`: the apostrophe must not become `don''t`.
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "don");

    type_str(&mut app, "'t");
    assert_eq!(text(&app), "don't");
}

#[test]
fn a_lifetime_is_not_a_string() {
    // `&'a str` is the case that makes an unconditional quote rule unusable in
    // the files this editor is opened on.
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "&");

    type_str(&mut app, "'a");
    assert_eq!(text(&app), "&'a");
}

#[test]
fn a_quote_closes_the_one_it_is_inside() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "\"hi");
    assert_eq!(marked(&app), "\"hi|\"");
    type_str(&mut app, "\"");
    assert_eq!(marked(&app), "\"hi\"|");
}

#[test]
fn a_bracket_typed_inside_a_word_stays_one_character() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "hello");
    // Between the two `l`s.
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Left);
    assert_eq!(marked(&app), "hel|lo");

    type_str(&mut app, "(");
    assert_eq!(
        marked(&app),
        "hel(|lo",
        "a pair here would cut the word in half"
    );
    assert_eq!(text(&app), "hel(lo");
}

#[test]
fn a_quote_typed_inside_a_word_stays_one_character() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "hello");
    plain(&mut app, KeyCode::Left);
    plain(&mut app, KeyCode::Left);

    type_str(&mut app, "\"");
    assert_eq!(text(&app), "hel\"lo");
}

#[test]
fn a_bracket_typed_at_the_end_of_a_word_still_pairs() {
    // The rule is about the *middle* of a word: the end of one is not the
    // middle, and `hello()` is what typing `(` after `hello` means.
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "hello");
    type_str(&mut app, "(");
    assert_eq!(marked(&app), "hello(|)");
}

#[test]
fn a_selection_inside_a_word_is_still_surrounded() {
    // A selection is not a caret in a word: it is the thing being wrapped.
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "hello");
    ctrl(&mut app, KeyCode::Home);
    plain(&mut app, KeyCode::Right);
    plain(&mut app, KeyCode::Right);
    shift(&mut app, KeyCode::Right);
    shift(&mut app, KeyCode::Right);
    assert_eq!(selection(&app).as_deref(), Some("ll"));

    type_str(&mut app, "(");
    assert_eq!(marked(&app), "he(ll|)o");
}

// ------------------------------------------------------------ selections

/// The selected text, if any.
fn selection(app: &vibe::app::App) -> Option<String> {
    app.active_doc().map(|d| d.selected_text())
}

#[test]
fn an_opener_typed_over_a_selection_surrounds_it() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "foo");

    ctrl(&mut app, 'a');
    type_str(&mut app, "(");
    assert_eq!(marked(&app), "(foo|)");
    assert_eq!(
        selection(&app).as_deref(),
        Some("foo"),
        "the text that was surrounded is still selected"
    );
}

#[test]
fn a_quote_typed_over_a_selection_surrounds_it() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "foo");

    ctrl(&mut app, 'a');
    type_str(&mut app, "\"");
    assert_eq!(marked(&app), "\"foo|\"");
    assert_eq!(selection(&app).as_deref(), Some("foo"));
}

#[test]
fn a_surrounded_selection_can_be_surrounded_again() {
    // Because the selection survives the first bracket, the second one wraps
    // the same text instead of replacing it: `foo` -> `(foo)` -> `("foo")`.
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "foo");

    ctrl(&mut app, 'a');
    type_str(&mut app, "(");
    type_str(&mut app, "\"");
    assert_eq!(marked(&app), "(\"foo|\")");
    assert_eq!(selection(&app).as_deref(), Some("foo"));
}

#[test]
fn typing_after_a_surround_replaces_the_selection() {
    // The other side of keeping it: the next character is typed over the text,
    // exactly as it would have been before the surround.
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "foo");

    ctrl(&mut app, 'a');
    type_str(&mut app, "(");
    type_str(&mut app, "x");
    assert_eq!(marked(&app), "(x|)");
}

// ------------------------------------------------------------- backspace

#[test]
fn backspace_inside_an_empty_pair_removes_both_halves() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "(");
    assert_eq!(marked(&app), "(|)");

    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "", "the pair goes as one");
}

#[test]
fn backspace_removes_one_character_when_the_pair_is_not_empty() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "(a");
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(marked(&app), "(|)");
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "");
}

#[test]
fn backspace_with_a_selection_deletes_the_selection_not_the_pair() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "(");
    // Select the opener. The caret sits between the two halves, so a pair rule
    // that ignored the selection would swallow both of them.
    shift(&mut app, KeyCode::Left);
    assert_eq!(
        app.active_doc().and_then(|d| d.selected_text().into()),
        Some("(".to_string()),
        "the opener is selected"
    );
    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), ")", "only the selection went");
}

// --------------------------------------------------------------- newline

#[test]
fn enter_between_braces_opens_the_block() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "fn main() ");

    type_str(&mut app, "{");
    assert_eq!(marked(&app), "fn main() {|}");
    plain(&mut app, KeyCode::Enter);
    assert_eq!(marked(&app), "fn main() {\n    |\n}");
}

#[test]
fn enter_inside_a_nested_block_uses_the_current_indentation() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "    if x ");

    type_str(&mut app, "{");
    plain(&mut app, KeyCode::Enter);
    assert_eq!(marked(&app), "    if x {\n        |\n    }");
}

#[test]
fn enter_between_parentheses_opens_them_too() {
    let scratch = Scratch::new();
    let mut app = app_with(&scratch, "f");

    type_str(&mut app, "(");
    plain(&mut app, KeyCode::Enter);
    assert_eq!(marked(&app), "f(\n    |\n)");
}

#[test]
fn enter_does_not_indent_when_auto_indent_is_off() {
    // Opening a pair onto three lines is auto-indent applied to a pair, so the
    // switch that turns indentation off turns this off with it.
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| c.editor.auto_indent = false);

    type_str(&mut app, "{");
    plain(&mut app, KeyCode::Enter);
    assert_eq!(marked(&app), "{\n|}");
}

#[test]
fn enter_between_quotes_does_not_break_the_string() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "\"");
    plain(&mut app, KeyCode::Enter);
    assert_eq!(
        marked(&app),
        "\"\n|\"",
        "a quote is not a block, so Enter is just a newline"
    );
}

// ------------------------------------------------------------- undo, paste

#[test]
fn one_undo_removes_a_pair_as_it_was_typed() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "(");
    assert_eq!(text(&app), "()");
    app.execute(Command::Undo);
    assert_eq!(text(&app), "", "the pair is one edit");
}

#[test]
fn a_pasted_bracket_is_not_completed() {
    // Paste is the raw path: what arrives is what lands.
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    app.handle_event(Event::Paste("(".into()));
    assert_eq!(text(&app), "(");
}

/// `Ctrl+Shift+A`, the way a terminal delivers it.
fn toggle(app: &mut vibe::app::App) {
    press(
        app,
        KeyCode::Char('a'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
}

#[test]
fn the_toggle_turns_the_rules_off_and_back_on() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    assert!(app.config.editor.auto_pairs, "on by default");

    toggle(&mut app);
    assert!(!app.config.editor.auto_pairs);
    assert!(
        screen_has(&mut app, 80, 24, "Auto-close pairs off"),
        "the change is announced"
    );
    type_str(&mut app, "(");
    assert_eq!(text(&app), "(", "no pair while it is off");

    toggle(&mut app);
    assert!(app.config.editor.auto_pairs);
    assert!(screen_has(&mut app, 80, 24, "Auto-close pairs on"));

    // Clear the buffer and type the same character again.
    ctrl(&mut app, 'a');
    plain(&mut app, KeyCode::Delete);
    type_str(&mut app, "[");
    assert_eq!(marked(&app), "[|]", "the pair is back");
}

#[test]
fn the_toggle_is_a_command_like_any_other() {
    assert!(Command::ALL.contains(&Command::ToggleAutoPairs));
    assert_eq!(Command::ToggleAutoPairs.id(), "edit.toggleAutoPairs");
    assert!(Command::ToggleAutoPairs
        .default_keys()
        .contains(&"ctrl+shift+a"));
}

#[test]
fn the_setting_turns_the_whole_behaviour_off() {
    let scratch = Scratch::new();
    let mut app = editor_with(scratch.path(), |c| c.editor.auto_pairs = false);

    type_str(&mut app, "(");
    assert_eq!(marked(&app), "(|");
    type_str(&mut app, ")");
    assert_eq!(text(&app), "()", "no stepping over either");

    plain(&mut app, KeyCode::Backspace);
    assert_eq!(text(&app), "(", "and no pair to delete");
}

// ------------------------------------------------------------ multi-cursor

#[test]
fn every_caret_gets_its_own_pair() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    type_str(&mut app, "a\nb");
    ctrl(&mut app, KeyCode::Home);
    press(
        &mut app,
        KeyCode::Down,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );

    let count = app.active_doc().map(|d| d.caret_count()).unwrap_or(0);
    assert_eq!(count, 2, "two carets, one per line");

    type_str(&mut app, "(");
    assert_eq!(
        text(&app),
        "()a\n()b",
        "a pair at each caret, with each caret between its own halves"
    );
}

#[test]
fn a_multi_cursor_typing_over_a_closer_moves_past_it_at_every_caret() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());

    // Two carets, one at the start of each line, and a pair at each.
    type_str(&mut app, "a\nb");
    ctrl(&mut app, KeyCode::Home);
    press(
        &mut app,
        KeyCode::Down,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    type_str(&mut app, "(");
    assert_eq!(text(&app), "()a\n()b");

    type_str(&mut app, ")");
    assert_eq!(text(&app), "()a\n()b", "no doubled closers");
    let columns: Vec<usize> = app
        .active_doc()
        .expect("a buffer")
        .carets()
        .iter()
        .map(|c| c.pos)
        .collect();
    assert_eq!(columns, vec![2, 6], "each caret moved past its own closer");
}
