//! Input normalisation.
//!
//! Terminal events are translated here into editor [`Action`]s. Keeping this in
//! one place means the rest of the application never inspects a `KeyEvent`, and
//! that terminal quirks (BackTab, Alt+char arriving as a two-byte sequence, a
//! bare Ctrl+letter arriving as a control character) are handled once.
//!
//! Text insertion is separated from command dispatch on purpose: a `Char('A')`
//! must insert an uppercase `A`, while `Ctrl+Shift+A` must resolve a binding.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Motion;
use crate::commands::Command;
use crate::keymap::{is_text_input, KeyChord, Keymap};

/// A normalised editor action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Insert literal text at the cursor.
    InsertText(String),
    /// Insert a line break, honouring auto-indent.
    InsertNewline,
    /// Insert one indent level (or indent the selected block).
    InsertTab,
    /// Remove one indent level from the selected block.
    Dedent,
    Backspace,
    DeleteForward,
    /// Move the cursor; `extend` keeps/grows the selection.
    Move {
        motion: Motion,
        extend: bool,
    },
    /// A registered command.
    Command(Command),
    /// Cancel the current transient state.
    Escape,
    /// Nothing to do.
    Ignored,
}

/// Translate a key event in the normal editing context.
pub fn translate(event: &KeyEvent, keymap: &Keymap) -> Action {
    let code = event.code;
    let mods = event.modifiers;
    let shift = mods.contains(KeyModifiers::SHIFT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);

    // 1. Command chords win, so a user can rebind anything.
    if let Some(cmd) = keymap.lookup(&KeyChord::from_event(event)) {
        return Action::Command(cmd);
    }

    // 2. Navigation, including Ctrl+word motions which are not commands.
    let extend = shift;
    match code {
        KeyCode::Left => {
            let motion = if ctrl { Motion::WordLeft } else { Motion::Left };
            return Action::Move { motion, extend };
        }
        KeyCode::Right => {
            let motion = if ctrl {
                Motion::WordRight
            } else {
                Motion::Right
            };
            return Action::Move { motion, extend };
        }
        KeyCode::Up => {
            return Action::Move {
                motion: Motion::Up,
                extend,
            }
        }
        KeyCode::Down => {
            return Action::Move {
                motion: Motion::Down,
                extend,
            }
        }
        KeyCode::Home => {
            let motion = if ctrl {
                Motion::DocStart
            } else if alt {
                Motion::LineStart
            } else {
                Motion::LineStartSmart
            };
            return Action::Move { motion, extend };
        }
        KeyCode::End => {
            let motion = if ctrl {
                Motion::DocEnd
            } else {
                Motion::LineEnd
            };
            return Action::Move { motion, extend };
        }
        KeyCode::PageUp => {
            return Action::Move {
                motion: Motion::PageUp,
                extend,
            }
        }
        KeyCode::PageDown => {
            return Action::Move {
                motion: Motion::PageDown,
                extend,
            }
        }
        _ => {}
    }

    // 3. Editing keys.
    match code {
        KeyCode::Esc => return Action::Escape,
        KeyCode::Enter => return Action::InsertNewline,
        // Shift+Tab is reported as Tab+SHIFT by terminals with the extended
        // keyboard protocol, and as BackTab by the others.
        KeyCode::Tab if shift => return Action::Dedent,
        KeyCode::Tab => return Action::InsertTab,
        KeyCode::BackTab => return Action::Dedent,
        KeyCode::Backspace => return Action::Backspace,
        KeyCode::Delete => return Action::DeleteForward,
        _ => {}
    }

    // 4. Printable input. AltGr on some layouts shows up as Ctrl+Alt, so treat
    //    that combination as text rather than dropping the character.
    if is_text_input(event) || (ctrl && alt && matches!(code, KeyCode::Char(_))) {
        if let KeyCode::Char(c) = code {
            return Action::InsertText(c.to_string());
        }
    }

    Action::Ignored
}

/// Translate a bracketed-paste payload.
///
/// Pasted text is inserted verbatim — no auto-indent, no newline splitting —
/// because reformatting a paste is surprising and can corrupt code.
pub fn translate_paste(text: &str) -> Action {
    if text.is_empty() {
        return Action::Ignored;
    }
    // Clipboard text from another platform (or from a browser) can carry CRLF
    // or bare CR. The buffer only ever stores LF internally; the document's
    // EOL setting is applied again on save, so normalising here keeps the
    // in-memory text consistent without changing what the user sees.
    if text.contains('\r') {
        Action::InsertText(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Action::InsertText(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn plain_char_is_text() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::Char('x'), KeyModifiers::NONE), &map),
            Action::InsertText("x".into())
        );
        assert_eq!(
            translate(&key(KeyCode::Char('X'), KeyModifiers::SHIFT), &map),
            Action::InsertText("X".into()),
            "shifted characters keep their case"
        );
    }

    #[test]
    fn ctrl_letters_are_commands_not_text() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::Char('s'), KeyModifiers::CONTROL), &map),
            Action::Command(Command::Save)
        );
        assert_eq!(
            translate(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), &map),
            Action::Command(Command::Copy),
            "Ctrl+C must copy, never interrupt"
        );
    }

    #[test]
    fn arrows_and_shift_selection() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::Left, KeyModifiers::NONE), &map),
            Action::Move {
                motion: Motion::Left,
                extend: false
            }
        );
        assert_eq!(
            translate(&key(KeyCode::Left, KeyModifiers::SHIFT), &map),
            Action::Move {
                motion: Motion::Left,
                extend: true
            }
        );
        assert_eq!(
            translate(&key(KeyCode::Right, KeyModifiers::CONTROL), &map),
            Action::Move {
                motion: Motion::WordRight,
                extend: false
            }
        );
    }

    #[test]
    fn tab_and_backtab() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::Tab, KeyModifiers::NONE), &map),
            Action::InsertTab
        );
        assert_eq!(
            translate(&key(KeyCode::BackTab, KeyModifiers::NONE), &map),
            Action::Dedent
        );
        // Some terminals report Shift+Tab as Tab with SHIFT.
        assert_eq!(
            translate(&key(KeyCode::Tab, KeyModifiers::SHIFT), &map),
            Action::Dedent
        );
    }

    #[test]
    fn escape_and_editing_keys() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::Esc, KeyModifiers::NONE), &map),
            Action::Escape
        );
        assert_eq!(
            translate(&key(KeyCode::Backspace, KeyModifiers::NONE), &map),
            Action::Backspace
        );
        assert_eq!(
            translate(&key(KeyCode::Enter, KeyModifiers::NONE), &map),
            Action::InsertNewline
        );
    }

    #[test]
    fn alt_char_is_not_text() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::Char('z'), KeyModifiers::ALT), &map),
            Action::Command(Command::ToggleWhitespace)
        );
        assert_eq!(
            translate(&key(KeyCode::Char('q'), KeyModifiers::ALT), &map),
            Action::Ignored
        );
    }

    #[test]
    fn function_keys_resolve() {
        let map = Keymap::defaults();
        assert_eq!(
            translate(&key(KeyCode::F(1), KeyModifiers::NONE), &map),
            Action::Command(Command::Help)
        );
        assert_eq!(
            translate(&key(KeyCode::F(3), KeyModifiers::NONE), &map),
            Action::Command(Command::FindNext)
        );
        assert_eq!(
            translate(&key(KeyCode::F(3), KeyModifiers::SHIFT), &map),
            Action::Command(Command::FindPrev)
        );
    }
}
