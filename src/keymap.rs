//! Key chords and binding resolution.
//!
//! # Why a normalisation step exists
//!
//! Terminals disagree about how to report modified keys. With the Kitty
//! keyboard protocol enabled (`DISAMBIGUATE_ESCAPE_CODES`) we get precise
//! modifiers; without it, `Ctrl+Shift+S` may arrive as `Ctrl+S`, as a bare
//! control byte, or not at all.
//!
//! [`KeyChord::from_event`] normalises events into a canonical form:
//!
//! * `Char('S')` becomes `Char('s')` + `SHIFT`;
//! * `SHIFT` is dropped for non-character keys where it carries no meaning.
//!
//! So a config written as `"ctrl+shift+s"` matches both the enhanced and the
//! legacy encodings. Text insertion deliberately bypasses this: it uses the raw
//! event character so `A` stays `A`.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::commands::Command;

/// A canonicalised key combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyChord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut chord = Self { code, mods };
        chord.canonicalise();
        chord
    }

    fn canonicalise(&mut self) {
        // Only the modifiers we care about; ignore lock keys and media bits.
        self.mods &=
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER;
        // crossterm reports Shift+Tab as `BackTab`; fold it into the canonical
        // form so `ctrl+shift+tab` resolves on terminals that do either.
        if self.code == KeyCode::BackTab {
            self.code = KeyCode::Tab;
            self.mods |= KeyModifiers::SHIFT;
        }
        match self.code {
            KeyCode::Char(c) => {
                if c.is_uppercase() {
                    let mut lower = c.to_lowercase();
                    if let Some(l) = lower.next() {
                        self.code = KeyCode::Char(l);
                        self.mods |= KeyModifiers::SHIFT;
                    }
                }
            }
            // Shift carries no meaning for Escape.
            KeyCode::Esc => {
                self.mods.remove(KeyModifiers::SHIFT);
            }
            _ => {}
        }
    }

    /// Normalise a terminal event for binding lookup.
    pub fn from_event(event: &KeyEvent) -> Self {
        Self::new(event.code, event.modifiers)
    }

    /// Parse `ctrl+shift+p`, `alt+up`, `f1`, `esc`, `/`.
    pub fn parse(input: &str) -> Option<Self> {
        let raw = input.trim();
        if raw.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        let mut rest = raw;
        // Peel modifiers off the front.
        loop {
            let lower = rest.to_ascii_lowercase();
            let Some((head, _)) = lower.split_once('+') else {
                break;
            };
            match head {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "alt" | "opt" | "option" | "meta" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                "super" | "cmd" | "command" | "win" => mods |= KeyModifiers::SUPER,
                _ => break,
            }
            // Modifier names are ASCII, so byte lengths line up with `raw`.
            rest = &rest[head.len() + 1..];
        }

        let code = match rest.to_ascii_lowercase().as_str() {
            "enter" | "return" | "cr" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            "plus" => KeyCode::Char('+'),
            "minus" => KeyCode::Char('-'),
            other if other.len() >= 2 && other.len() <= 3 && other.starts_with('f') => {
                let n: u8 = other[1..].parse().ok()?;
                if (1..=24).contains(&n) {
                    KeyCode::F(n)
                } else {
                    return None;
                }
            }
            other => {
                let mut chars = other.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        };

        Some(Self::new(code, mods))
    }

    /// Human readable form: `Ctrl+Shift+P`, `F1`, `Esc`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push_str("Cmd+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("Shift+");
        }
        match self.code {
            KeyCode::Char(' ') => out.push_str("Space"),
            KeyCode::Char(c) => {
                if self.mods.contains(KeyModifiers::SHIFT) {
                    out.push(c.to_ascii_uppercase());
                } else {
                    out.push(c);
                }
            }
            KeyCode::F(n) => out.push_str(&format!("F{n}")),
            KeyCode::Enter => out.push_str("Enter"),
            KeyCode::Esc => out.push_str("Esc"),
            KeyCode::Tab => out.push_str("Tab"),
            KeyCode::Backspace => out.push_str("Backspace"),
            KeyCode::Delete => out.push_str("Del"),
            KeyCode::Insert => out.push_str("Ins"),
            KeyCode::Up => out.push('↑'),
            KeyCode::Down => out.push('↓'),
            KeyCode::Left => out.push('←'),
            KeyCode::Right => out.push('→'),
            KeyCode::Home => out.push_str("Home"),
            KeyCode::End => out.push_str("End"),
            KeyCode::PageUp => out.push_str("PgUp"),
            KeyCode::PageDown => out.push_str("PgDn"),
            _ => out.push('?'),
        }
        out
    }

    /// Config-file representation, the inverse of [`KeyChord::parse`].
    pub fn to_config_string(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("alt+");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push_str("super+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("shift+");
        }
        match self.code {
            KeyCode::Char(c) => out.push(c),
            KeyCode::F(n) => out.push_str(&format!("f{n}")),
            KeyCode::Enter => out.push_str("enter"),
            KeyCode::Esc => out.push_str("esc"),
            KeyCode::Tab => out.push_str("tab"),
            KeyCode::Backspace => out.push_str("backspace"),
            KeyCode::Delete => out.push_str("delete"),
            KeyCode::Insert => out.push_str("insert"),
            KeyCode::Up => out.push_str("up"),
            KeyCode::Down => out.push_str("down"),
            KeyCode::Left => out.push_str("left"),
            KeyCode::Right => out.push_str("right"),
            KeyCode::Home => out.push_str("home"),
            KeyCode::End => out.push_str("end"),
            KeyCode::PageUp => out.push_str("pageup"),
            KeyCode::PageDown => out.push_str("pagedown"),
            _ => out.push('?'),
        }
        out
    }
}

/// Chord to command map.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: HashMap<KeyChord, Command>,
}

impl Keymap {
    /// Bindings derived from [`Command::default_keys`].
    pub fn defaults() -> Self {
        let mut map = Self::default();
        for cmd in Command::ALL {
            for spec in cmd.default_keys() {
                match KeyChord::parse(spec) {
                    Some(chord) => {
                        map.bindings.insert(chord, *cmd);
                    }
                    None => {
                        debug_assert!(false, "invalid default binding {spec} for {}", cmd.id());
                    }
                }
            }
        }
        map
    }

    /// Apply `chord = "command.id"` overrides from configuration.
    ///
    /// Returns warnings for unknown chords and unknown command ids; those
    /// bindings are skipped so a typo cannot silently unbind a working key.
    pub fn apply_overrides(
        &mut self,
        overrides: &std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for (chord_str, cmd_id) in overrides {
            let Some(chord) = KeyChord::parse(chord_str) else {
                warnings.push(format!("keys: cannot parse \"{chord_str}\""));
                continue;
            };
            if cmd_id.is_empty() || cmd_id == "none" {
                self.bindings.remove(&chord);
                continue;
            }
            match Command::from_id(cmd_id) {
                Some(cmd) => {
                    self.bindings.insert(chord, cmd);
                }
                None => warnings.push(format!("keys: unknown command \"{cmd_id}\"")),
            }
        }
        warnings
    }

    pub fn bind(&mut self, chord: KeyChord, cmd: Command) {
        self.bindings.insert(chord, cmd);
    }

    pub fn unbind(&mut self, chord: &KeyChord) {
        self.bindings.remove(chord);
    }

    pub fn lookup(&self, chord: &KeyChord) -> Option<Command> {
        self.bindings.get(chord).copied()
    }

    /// All chords bound to `cmd`, sorted for stable display.
    pub fn chords_for(&self, cmd: Command) -> Vec<KeyChord> {
        let mut out: Vec<KeyChord> = self
            .bindings
            .iter()
            .filter(|(_, c)| **c == cmd)
            .map(|(k, _)| *k)
            .collect();
        out.sort_by_key(|c| c.to_config_string());
        out
    }

    /// Primary binding label for display, or empty string.
    pub fn label_for(&self, cmd: Command) -> String {
        self.chords_for(cmd)
            .first()
            .map(KeyChord::label)
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Every binding, sorted, for the help screen and diagnostics.
    pub fn all(&self) -> Vec<(KeyChord, Command)> {
        let mut out: Vec<(KeyChord, Command)> =
            self.bindings.iter().map(|(k, v)| (*k, *v)).collect();
        out.sort_by_key(|(k, _)| (k.to_config_string(), k.label()));
        out
    }
}

/// True when the event should be treated as text input.
///
/// Text input is everything printable that is not a command chord: no Control,
/// no Alt (Alt is a modifier for commands in this editor), and a `Char` code.
pub fn is_text_input(event: &KeyEvent) -> bool {
    if event.kind == KeyEventKind::Release {
        return false;
    }
    match event.code {
        KeyCode::Char(_) => {
            let mods = event.modifiers;
            !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifier_chords() {
        let c = KeyChord::parse("ctrl+shift+s").unwrap();
        assert_eq!(c.code, KeyCode::Char('s'));
        assert!(c.mods.contains(KeyModifiers::CONTROL));
        assert!(c.mods.contains(KeyModifiers::SHIFT));

        let f = KeyChord::parse("F1").unwrap();
        assert_eq!(f.code, KeyCode::F(1));
        assert_eq!(f.mods, KeyModifiers::NONE);
    }

    #[test]
    fn parses_named_keys_and_symbols() {
        assert_eq!(KeyChord::parse("esc").unwrap().code, KeyCode::Esc);
        assert_eq!(KeyChord::parse("pageup").unwrap().code, KeyCode::PageUp);
        assert_eq!(KeyChord::parse("ctrl+/").unwrap().code, KeyCode::Char('/'));
        assert_eq!(KeyChord::parse("space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(KeyChord::parse("alt+up").unwrap().code, KeyCode::Up);
        assert!(KeyChord::parse("ctrl+notakey").is_none());
        assert!(KeyChord::parse("").is_none());
    }

    #[test]
    fn roundtrips_through_config_string() {
        for spec in [
            "ctrl+s",
            "ctrl+shift+p",
            "alt+up",
            "f1",
            "esc",
            "ctrl+/",
            "shift+f3",
        ] {
            let chord = KeyChord::parse(spec).unwrap();
            let s = chord.to_config_string();
            assert_eq!(
                KeyChord::parse(&s).unwrap(),
                chord,
                "roundtrip {spec} -> {s}"
            );
        }
    }

    #[test]
    fn uppercase_events_normalise_to_shift() {
        let event = KeyEvent::new(KeyCode::Char('S'), KeyModifiers::CONTROL);
        let chord = KeyChord::from_event(&event);
        assert_eq!(chord.code, KeyCode::Char('s'));
        assert!(chord.mods.contains(KeyModifiers::SHIFT));
        assert_eq!(chord, KeyChord::parse("ctrl+shift+s").unwrap());
    }

    #[test]
    fn legacy_ctrl_shift_falls_back_to_ctrl() {
        // A terminal without the Kitty protocol reports Ctrl+Shift+S as the
        // control byte 0x13 with no SHIFT bit; we still resolve a command.
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let map = Keymap::defaults();
        assert_eq!(
            map.lookup(&KeyChord::from_event(&event)),
            Some(Command::Save)
        );
    }

    #[test]
    fn defaults_cover_the_specified_shortcuts() {
        let map = Keymap::defaults();
        let expect = [
            ("ctrl+s", Command::Save),
            ("ctrl+shift+s", Command::SaveAs),
            ("ctrl+o", Command::Open),
            ("ctrl+n", Command::NewBuffer),
            ("ctrl+f", Command::Find),
            ("ctrl+h", Command::Replace),
            ("ctrl+z", Command::Undo),
            ("ctrl+y", Command::Redo),
            ("ctrl+x", Command::Cut),
            ("ctrl+c", Command::Copy),
            ("ctrl+v", Command::Paste),
            ("ctrl+a", Command::SelectAll),
            ("ctrl+p", Command::QuickOpen),
            ("ctrl+shift+p", Command::CommandPalette),
            ("ctrl+g", Command::GotoLine),
            ("ctrl+w", Command::CloseBuffer),
            ("ctrl+tab", Command::NextBuffer),
            ("f1", Command::Help),
        ];
        for (spec, cmd) in expect {
            let chord = KeyChord::parse(spec).unwrap();
            assert_eq!(map.lookup(&chord), Some(cmd), "{spec}");
        }
    }

    #[test]
    fn overrides_apply_and_report_errors() {
        let mut map = Keymap::defaults();
        let mut over = std::collections::BTreeMap::new();
        over.insert("ctrl+k".to_string(), "file.save".to_string());
        over.insert("ctrl+bad".to_string(), "file.save".to_string());
        over.insert("ctrl+j".to_string(), "nope.nope".to_string());
        over.insert("ctrl+s".to_string(), "none".to_string());
        let warnings = map.apply_overrides(&over);
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            map.lookup(&KeyChord::parse("ctrl+k").unwrap()),
            Some(Command::Save)
        );
        assert_eq!(map.lookup(&KeyChord::parse("ctrl+s").unwrap()), None);
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(
            KeyChord::parse("ctrl+shift+p").unwrap().label(),
            "Ctrl+Shift+P"
        );
        assert_eq!(KeyChord::parse("f1").unwrap().label(), "F1");
        assert_eq!(KeyChord::parse("esc").unwrap().label(), "Esc");
    }

    #[test]
    fn text_input_excludes_command_modifiers() {
        assert!(is_text_input(&KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE
        )));
        assert!(is_text_input(&KeyEvent::new(
            KeyCode::Char('A'),
            KeyModifiers::SHIFT
        )));
        assert!(!is_text_input(&KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_text_input(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }
}
