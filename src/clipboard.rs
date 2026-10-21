//! Clipboard abstraction with an explicit fallback.
//!
//! Three layers, tried in order:
//!
//! 1. the OS clipboard via `arboard` (works on macOS, X11 and Wayland, and over
//!    SSH only if a clipboard bridge such as `xclip` forwarding exists);
//! 2. an internal clipboard owned by the editor, always available;
//! 3. — not implemented — OSC 52 terminal passthrough, see the roadmap.
//!
//! A copy always populates *both* the system and internal clipboard, so a paste
//! still works after the terminal loses clipboard access (tmux, SSH, headless
//! CI). Failures are reported once to the user instead of silently dropping the
//! text, and never abort the edit.

use tracing::{debug, warn};

/// Outcome of a clipboard write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardWrite {
    /// Written to the system clipboard.
    System,
    /// System clipboard unavailable; stored internally only.
    InternalFallback,
}

impl ClipboardWrite {
    pub fn is_fallback(self) -> bool {
        self == ClipboardWrite::InternalFallback
    }
}

/// Clipboard facade.
pub struct Clipboard {
    system: Option<arboard::Clipboard>,
    internal: String,
    /// Set once we have told the user the system clipboard is unavailable.
    warned: bool,
    system_error: Option<String>,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard")
            .field("system_available", &self.system.is_some())
            .field("internal_len", &self.internal.len())
            .finish()
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    /// Probe the system clipboard. Failure is expected in headless sessions and
    /// is not an error.
    pub fn new() -> Self {
        let (system, system_error) = match arboard::Clipboard::new() {
            Ok(c) => (Some(c), None),
            Err(e) => {
                debug!("system clipboard unavailable: {e}");
                (None, Some(e.to_string()))
            }
        };
        Self {
            system,
            internal: String::new(),
            warned: false,
            system_error,
        }
    }

    /// True when a system clipboard backend was initialised.
    pub fn system_available(&self) -> bool {
        self.system.is_some()
    }

    /// A clipboard that never touches the OS.
    ///
    /// Used by headless test applications: the platform pasteboard is not
    /// thread-safe, so N test threads each opening it would be a data race for
    /// no benefit. Copy/paste semantics are identical, only the system layer is
    /// skipped.
    pub fn internal_only() -> Self {
        Self {
            system: None,
            internal: String::new(),
            warned: true,
            system_error: None,
        }
    }

    /// Read the clipboard, preferring the system one and falling back to the
    /// internal buffer.
    pub fn get(&mut self) -> Option<String> {
        if let Some(sys) = self.system.as_mut() {
            match sys.get_text() {
                Ok(text) if !text.is_empty() => return Some(text),
                Ok(_) => {}
                Err(e) => {
                    warn!("clipboard read failed: {e}");
                }
            }
        }
        if self.internal.is_empty() {
            None
        } else {
            Some(self.internal.clone())
        }
    }

    /// Write `text`, always keeping an internal copy.
    pub fn set(&mut self, text: &str) -> ClipboardWrite {
        self.internal = text.to_string();
        if let Some(sys) = self.system.as_mut() {
            match sys.set_text(text.to_string()) {
                Ok(()) => return ClipboardWrite::System,
                Err(e) => {
                    warn!("clipboard write failed: {e}");
                    self.system = None;
                    self.system_error = Some(e.to_string());
                }
            }
        }
        ClipboardWrite::InternalFallback
    }

    /// Internal buffer contents (used by tests and the `--clipboard-status`).
    pub fn internal(&self) -> &str {
        &self.internal
    }

    /// Returns true exactly once, so the UI can warn about the fallback without
    /// repeating itself on every copy.
    pub fn take_fallback_notice(&mut self) -> Option<String> {
        if self.system.is_some() || self.warned {
            return None;
        }
        self.warned = true;
        Some(match &self.system_error {
            Some(err) => format!("System clipboard unavailable ({err}); using internal clipboard"),
            None => "System clipboard unavailable; using internal clipboard".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_buffer_roundtrips() {
        let mut c = Clipboard {
            system: None,
            internal: String::new(),
            warned: false,
            system_error: None,
        };
        assert_eq!(c.get(), None);
        let outcome = c.set("hello");
        assert_eq!(outcome, ClipboardWrite::InternalFallback);
        assert!(outcome.is_fallback());
        assert_eq!(c.get(), Some("hello".to_string()));
        assert_eq!(c.internal(), "hello");
    }

    #[test]
    fn fallback_notice_is_emitted_once() {
        let mut c = Clipboard {
            system: None,
            internal: String::new(),
            warned: false,
            system_error: Some("no display".into()),
        };
        assert!(c.take_fallback_notice().is_some());
        assert!(c.take_fallback_notice().is_none());
    }

    #[test]
    fn empty_internal_buffer_reads_as_none() {
        let mut c = Clipboard {
            system: None,
            internal: String::new(),
            warned: false,
            system_error: None,
        };
        c.set("");
        assert_eq!(c.get(), None);
    }
}
