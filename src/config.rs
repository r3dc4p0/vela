//! Configuration: defaults, TOML loading, validation and diagnostics.
//!
//! Design rules:
//!
//! * every field has a default and is `#[serde(default)]`, so a partial or
//!   newer/older config file still loads;
//! * loading never fails the application — problems are collected as warnings
//!   and shown in the message area, and the affected field falls back to its
//!   default;
//! * values are clamped to sane ranges during validation rather than being
//!   trusted, because a `tab_width = 0` or `scroll_off = 10000` would otherwise
//!   produce a broken or slow UI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::platform;

/// Root configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub editor: EditorSettings,
    pub ui: UiSettings,
    pub files: FileSettings,
    pub terminal: TerminalSettings,
    /// `chord = "command.id"` overrides, e.g. `"ctrl+s" = "file.save"`.
    pub keys: BTreeMap<String, String>,
    /// `key = "#rrggbb"` theme overrides.
    pub colors: BTreeMap<String, String>,
}

/// Editing behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// Columns a tab advances to.
    pub tab_width: usize,
    /// Insert spaces instead of a tab character.
    pub use_spaces: bool,
    /// Copy the previous line's indentation on Enter.
    pub auto_indent: bool,
    /// Enable syntax highlighting.
    pub syntax_highlighting: bool,
    /// Keep this many lines above/below the cursor when scrolling.
    pub scroll_off: usize,
    /// Show line numbers relative to the cursor.
    pub relative_numbers: bool,
    /// Show the line number gutter at all.
    pub line_numbers: bool,
    /// Render spaces and tabs as visible glyphs.
    pub show_whitespace: bool,
    /// Draw vertical indent guides.
    pub show_indent_guides: bool,
    /// Highlight the line containing the cursor.
    pub highlight_cursor_line: bool,
    /// Highlight the matching bracket next to the cursor.
    pub match_brackets: bool,
    /// Undo history entry cap.
    pub max_undo_entries: usize,
    /// Undo history memory cap in megabytes.
    pub max_undo_mb: usize,
    /// Confirm before quitting with unsaved buffers.
    pub confirm_unsaved: bool,
    /// Restore the previous session's buffers on startup.
    pub restore_session: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            use_spaces: true,
            auto_indent: true,
            syntax_highlighting: true,
            scroll_off: 3,
            relative_numbers: false,
            line_numbers: true,
            show_whitespace: false,
            show_indent_guides: true,
            highlight_cursor_line: true,
            match_brackets: true,
            max_undo_entries: 5_000,
            max_undo_mb: 64,
            confirm_unsaved: true,
            restore_session: false,
        }
    }
}

/// Chrome and interaction settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Theme name: `vela-dark` or `vela-light`.
    pub theme: String,
    /// Show the contextual shortcut hints bar.
    pub show_hints: bool,
    /// Show the buffer tab strip when more than one buffer is open.
    pub show_tabs: bool,
    /// Enable mouse reporting.
    pub mouse: bool,
    /// How long messages stay visible, in milliseconds.
    pub message_timeout_ms: u64,
    /// Show the top bar.
    pub show_topbar: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: "vela-dark".to_string(),
            show_hints: true,
            show_tabs: true,
            mouse: true,
            message_timeout_ms: 4_000,
            show_topbar: true,
        }
    }
}

/// File handling settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSettings {
    /// Strip trailing whitespace from every line on save.
    pub trim_trailing_whitespace: bool,
    /// Append a final newline on save when the file does not end with one.
    pub ensure_final_newline: bool,
    /// Warn when the file changed on disk before overwriting it.
    pub warn_external_change: bool,
    /// Number of recent files to remember.
    pub recent_files: usize,
    /// Default directory for the file finder, relative to the file or cwd.
    pub find_root: Option<String>,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            trim_trailing_whitespace: false,
            ensure_final_newline: false,
            warn_external_change: true,
            recent_files: 20,
            find_root: None,
        }
    }
}

/// Terminal capability settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    /// Progressive keyboard protocol: `auto`, `on` or `off`.
    ///
    /// `auto` enables it only for terminals known to implement the protocol
    /// instead of probing, because probing blocks for up to two seconds on
    /// terminals that never answer.
    pub keyboard_enhancement: String,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            keyboard_enhancement: "auto".to_string(),
        }
    }
}

/// How the keyboard protocol should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enhancement {
    Auto,
    On,
    Off,
}

impl Enhancement {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Enhancement::Auto),
            "on" | "true" | "yes" | "1" => Some(Enhancement::On),
            "off" | "false" | "no" | "0" => Some(Enhancement::Off),
            _ => None,
        }
    }
}

/// Result of loading a configuration file.
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<String>,
    pub path: PathBuf,
}

impl Config {
    /// Load from `path`, or return defaults when the file does not exist.
    pub fn load_or_default(path: Option<&Path>) -> LoadedConfig {
        let path = path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(platform::config_path);

        if !path.exists() {
            return LoadedConfig {
                config: Config::default(),
                warnings: Vec::new(),
                path,
            };
        }

        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok((config, mut warnings)) => {
                    warnings.extend(config.validate());
                    LoadedConfig {
                        config,
                        warnings,
                        path,
                    }
                }
                Err(e) => LoadedConfig {
                    config: Config::default(),
                    warnings: vec![format!("{}: {e}", path.display())],
                    path,
                },
            },
            Err(e) => LoadedConfig {
                config: Config::default(),
                warnings: vec![format!("cannot read {}: {e}", path.display())],
                path,
            },
        }
    }

    /// Parse and validate TOML.
    pub fn from_toml(text: &str) -> Result<(Config, Vec<String>)> {
        let config: Config =
            toml::from_str(text).map_err(|e| Error::Config(e.message().to_string()))?;
        Ok((config, Vec::new()))
    }

    /// Serialize back to TOML (used by tests and `--print-config`).
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))
    }

    /// Clamp out-of-range values, returning human readable warnings.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let e = &self.editor;
        if e.tab_width == 0 || e.tab_width > 16 {
            warnings.push(format!(
                "editor.tab_width={} is out of range 1..=16; using 4",
                e.tab_width
            ));
        }
        if e.scroll_off > 50 {
            warnings.push(format!(
                "editor.scroll_off={} is too large; using 50",
                e.scroll_off
            ));
        }
        if e.max_undo_mb == 0 {
            warnings.push("editor.max_undo_mb=0 would disable undo; using 64".to_string());
        }
        if self.ui.message_timeout_ms > 60_000 {
            warnings.push("ui.message_timeout_ms capped at 60000".to_string());
        }
        if self.files.recent_files > 500 {
            warnings.push("files.recent_files capped at 500".to_string());
        }
        warnings
    }

    /// Apply clamps. Called after `validate`.
    pub fn normalize(&mut self) {
        self.editor.tab_width = if self.editor.tab_width == 0 {
            4
        } else {
            self.editor.tab_width.clamp(1, 16)
        };
        self.editor.scroll_off = self.editor.scroll_off.min(50);
        self.editor.max_undo_entries = self.editor.max_undo_entries.clamp(10, 1_000_000);
        self.editor.max_undo_mb = self.editor.max_undo_mb.clamp(1, 4096);
        self.ui.message_timeout_ms = self.ui.message_timeout_ms.min(60_000);
        self.files.recent_files = self.files.recent_files.min(500);
    }

    /// Effective tab width.
    pub fn tab_width(&self) -> usize {
        self.editor.tab_width.clamp(1, 16)
    }

    /// The string inserted for one indent level.
    pub fn indent_unit(&self) -> String {
        if self.editor.use_spaces {
            " ".repeat(self.tab_width())
        } else {
            "\t".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.editor.tab_width, 4);
        assert!(c.editor.syntax_highlighting);
        assert_eq!(c.ui.theme, "vela-dark");
        assert!(c.validate().is_empty());
    }

    #[test]
    fn partial_toml_falls_back_to_defaults() {
        let (c, _) = Config::from_toml("[editor]\ntab_width = 2\n").unwrap();
        assert_eq!(c.editor.tab_width, 2);
        assert_eq!(c.editor.scroll_off, 3, "unspecified field keeps default");
        assert_eq!(c.ui.theme, "vela-dark");
    }

    #[test]
    fn malformed_toml_is_an_error() {
        assert!(Config::from_toml("this is not toml [[[").is_err());
    }

    #[test]
    fn validation_warns_and_normalises() {
        let (mut c, _) = Config::from_toml("[editor]\ntab_width = 0\nscroll_off = 900\n").unwrap();
        let warnings = c.validate();
        assert_eq!(warnings.len(), 2);
        c.normalize();
        assert_eq!(c.tab_width(), 4);
        assert_eq!(c.editor.scroll_off, 50);
    }

    #[test]
    fn roundtrips_through_toml() {
        let c = Config::default();
        let text = c.to_toml().unwrap();
        let (c2, _) = Config::from_toml(&text).unwrap();
        assert_eq!(c2.editor.tab_width, c.editor.tab_width);
        assert_eq!(c2.ui.theme, c.ui.theme);
    }

    #[test]
    fn indent_unit_respects_settings() {
        let mut c = Config::default();
        c.editor.tab_width = 2;
        assert_eq!(c.indent_unit(), "  ");
        c.editor.use_spaces = false;
        assert_eq!(c.indent_unit(), "\t");
    }

    #[test]
    fn key_overrides_are_parsed() {
        let (c, _) = Config::from_toml("[keys]\n\"ctrl+s\" = \"file.save\"\n").unwrap();
        assert_eq!(c.keys.get("ctrl+s").map(String::as_str), Some("file.save"));
    }
}
