//! Shared helpers for the integration tests.
//!
//! Every test drives the real `App` through the real input translation layer,
//! so the assertions exercise the same code path a terminal keystroke takes.
//! `App::headless` keeps the tests away from the user's profile directory.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use vela::app::App;
use vela::config::Config;

/// Build an editor rooted at `dir` that never touches user state.
pub fn editor(dir: &Path) -> App {
    App::headless(dir.to_path_buf(), Config::default())
}

/// Build an editor with a customised configuration.
pub fn editor_with(dir: &Path, f: impl FnOnce(&mut Config)) -> App {
    let mut cfg = Config::default();
    f(&mut cfg);
    App::headless(dir.to_path_buf(), cfg)
}

pub fn chord(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

pub fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    app.handle_event(Event::Key(chord(code, mods)));
}

/// Accepts both `'c'` and `KeyCode::End` so tests read naturally.
pub trait KeyArg {
    fn key_code(self) -> KeyCode;
}

impl KeyArg for char {
    fn key_code(self) -> KeyCode {
        KeyCode::Char(self)
    }
}

impl KeyArg for KeyCode {
    fn key_code(self) -> KeyCode {
        self
    }
}

pub fn ctrl(app: &mut App, k: impl KeyArg) {
    press(app, k.key_code(), KeyModifiers::CONTROL);
}

pub fn shift(app: &mut App, code: KeyCode) {
    press(app, code, KeyModifiers::SHIFT);
}

pub fn plain(app: &mut App, code: KeyCode) {
    press(app, code, KeyModifiers::NONE);
}

pub fn alt(app: &mut App, code: KeyCode) {
    press(app, code, KeyModifiers::ALT);
}

/// Type a literal string, one character at a time (newlines become Enter).
pub fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        if ch == '\n' {
            plain(app, KeyCode::Enter);
        } else {
            press(app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }
}

/// The active buffer as a `String`.
pub fn text(app: &App) -> String {
    app.active_doc().map(|d| d.text()).unwrap_or_default()
}

/// Cursor as a flat char index.
pub fn cursor(app: &App) -> usize {
    app.active_doc().map(|d| d.cursor()).unwrap_or(0)
}

/// Cursor as 1-based (line, column).
pub fn cursor_1(app: &App) -> (usize, usize) {
    app.active_doc()
        .map(|d| d.cursor_line_col_1())
        .unwrap_or((1, 1))
}

pub fn modified(app: &App) -> bool {
    app.active_doc().map(|d| d.is_modified()).unwrap_or(false)
}

/// Render the UI headlessly and return the screen as text lines.
pub fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    vela::ui::render_to_lines(app, width, height)
}

/// True when any rendered row contains `needle`.
pub fn screen_has(app: &mut App, width: u16, height: u16, needle: &str) -> bool {
    screen(app, width, height)
        .iter()
        .any(|row| row.contains(needle))
}

/// Open a file that already exists on disk.
pub fn open(app: &mut App, path: &Path) {
    app.open_path(path.to_path_buf());
}

/// Save the active buffer under a new name by driving the real Save As prompt.
pub fn save_as(app: &mut App, path: &Path) {
    app.save_active(true);
    // The prompt pre-fills the current directory; clear it first.
    ctrl(app, 'u');
    for ch in path.to_string_lossy().chars() {
        press(app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    plain(app, KeyCode::Enter);
}

/// A scratch directory that is removed when the guard is dropped.
pub struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn file(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, body).expect("write fixture");
        path
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}
