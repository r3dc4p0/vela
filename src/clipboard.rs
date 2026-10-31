//! Clipboard abstraction with an explicit fallback.
//!
//! Four layers, tried in the order the configuration asks for:
//!
//! 1. the OS pasteboard via `arboard` (macOS, X11, Wayland);
//! 2. **OSC 52**, which asks the *terminal* to do the copy — the only route
//!    that reaches the user's real clipboard over SSH, inside tmux, or in a
//!    container, where the process has no pasteboard of its own;
//! 3. an internal clipboard owned by the editor, always available;
//! 4. never touching anything outside the process, for anyone who wants that.
//!
//! A copy always populates the internal clipboard as well, so a paste still
//! works after the terminal loses clipboard access. Failures are reported once
//! instead of silently dropping the text, and never abort the edit.
//!
//! `arboard` succeeding is not proof that the user has the text: in an SSH
//! session it may have written the *remote* X11 clipboard, which nobody will
//! paste from. That is what `clipboard = "terminal"` is for.

use tracing::{debug, warn};

/// Where a copy should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipboardMode {
    /// The pasteboard if there is one, the terminal if there is not.
    #[default]
    Auto,
    /// The OS pasteboard only.
    System,
    /// The terminal only, through OSC 52.
    Terminal,
    /// Stay inside the editor.
    Internal,
}

impl ClipboardMode {
    /// Parse the configuration value, case-insensitively.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(ClipboardMode::Auto),
            "system" | "os" => Some(ClipboardMode::System),
            "terminal" | "osc52" => Some(ClipboardMode::Terminal),
            "internal" | "none" | "off" => Some(ClipboardMode::Internal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ClipboardMode::Auto => "auto",
            ClipboardMode::System => "system",
            ClipboardMode::Terminal => "terminal",
            ClipboardMode::Internal => "internal",
        }
    }
}

/// Outcome of a clipboard write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardWrite {
    /// Written to the OS pasteboard.
    System,
    /// Queued as an OSC 52 sequence for the terminal to act on.
    Terminal,
    /// Neither: stored internally only.
    InternalFallback,
}

impl ClipboardWrite {
    pub fn is_fallback(self) -> bool {
        self == ClipboardWrite::InternalFallback
    }
}

/// Largest selection sent to the terminal.
///
/// OSC 52 has no agreed limit and the terminals that implement it disagree:
/// xterm truncates around 100 KiB, others silently ignore an oversized
/// sequence, and a few stall while they take it. Beyond this the copy stays
/// internal and says so, which is honest — a paste that quietly does nothing
/// is not.
pub const OSC52_LIMIT: usize = 100 * 1024;

/// Clipboard facade.
pub struct Clipboard {
    mode: ClipboardMode,
    system: Option<arboard::Clipboard>,
    internal: String,
    /// The OSC 52 sequence waiting to be written to the terminal.
    pending: Option<String>,
    /// Set once we have told the user the clipboard is unavailable.
    warned: bool,
    system_error: Option<String>,
    /// The last copy was too big for the terminal.
    too_large: bool,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard")
            .field("mode", &self.mode)
            .field("system_available", &self.system.is_some())
            .field("internal_len", &self.internal.len())
            .field("pending", &self.pending.is_some())
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
        Self::with_mode(ClipboardMode::Auto)
    }

    /// The same, in the mode the configuration chose.
    pub fn with_mode(mode: ClipboardMode) -> Self {
        let (system, system_error) = match arboard::Clipboard::new() {
            Ok(c) => (Some(c), None),
            Err(e) => {
                debug!("system clipboard unavailable: {e}");
                (None, Some(e.to_string()))
            }
        };
        Self {
            mode,
            system,
            internal: String::new(),
            pending: None,
            warned: false,
            system_error,
            too_large: false,
        }
    }

    /// True when a system clipboard backend was initialised.
    pub fn system_available(&self) -> bool {
        self.system.is_some()
    }

    pub fn mode(&self) -> ClipboardMode {
        self.mode
    }

    /// Change where copies go.
    ///
    /// Does not probe the pasteboard: the mode says what to *try*, and a mode
    /// chosen later is no reason to open the platform clipboard again.
    pub fn set_mode(&mut self, mode: ClipboardMode) {
        self.mode = mode;
    }

    /// A clipboard that never touches anything outside the process.
    ///
    /// Used by headless test applications: the platform pasteboard is not
    /// thread-safe, so N test threads each opening it would be a data race for
    /// no benefit. Copy/paste semantics are identical; only the system and
    /// terminal layers are skipped.
    pub fn internal_only() -> Self {
        Self {
            mode: ClipboardMode::Internal,
            system: None,
            internal: String::new(),
            pending: None,
            warned: true,
            system_error: None,
            too_large: false,
        }
    }

    /// Read the clipboard, preferring the system one and falling back to the
    /// internal buffer.
    ///
    /// There is no OSC 52 read: asking the terminal for its clipboard is a
    /// round-trip that blocks the event loop until the answer arrives, and
    /// `Ctrl+V` already reaches the editor as a bracketed paste.
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
        self.pending = None;
        self.too_large = false;

        match self.mode {
            ClipboardMode::Internal => ClipboardWrite::InternalFallback,
            ClipboardMode::Terminal => self.queue_for_terminal(text),
            ClipboardMode::System => self.write_system(text),
            // The pasteboard first, because it needs no help from the terminal;
            // OSC 52 when there is no pasteboard, or writing to it failed.
            ClipboardMode::Auto => match self.write_system(text) {
                ClipboardWrite::System => ClipboardWrite::System,
                _ => self.queue_for_terminal(text),
            },
        }
    }

    /// The OSC 52 sequence to hand to the terminal, if a copy produced one.
    ///
    /// Taken rather than borrowed so it is written exactly once: the event loop
    /// flushes it after the keystroke, and a sequence left in place would be
    /// written again on the next frame.
    pub fn take_terminal_sequence(&mut self) -> Option<String> {
        self.pending.take()
    }

    fn write_system(&mut self, text: &str) -> ClipboardWrite {
        let Some(sys) = self.system.as_mut() else {
            return ClipboardWrite::InternalFallback;
        };
        match sys.set_text(text.to_string()) {
            Ok(()) => ClipboardWrite::System,
            Err(e) => {
                warn!("clipboard write failed: {e}");
                self.system = None;
                self.system_error = Some(e.to_string());
                ClipboardWrite::InternalFallback
            }
        }
    }

    fn queue_for_terminal(&mut self, text: &str) -> ClipboardWrite {
        match osc52_sequence(text) {
            Some(sequence) => {
                self.pending = Some(sequence);
                ClipboardWrite::Terminal
            }
            None => {
                self.too_large = true;
                ClipboardWrite::InternalFallback
            }
        }
    }

    /// Internal buffer contents (used by tests).
    pub fn internal(&self) -> &str {
        &self.internal
    }

    /// Returns true exactly once, so the UI can warn about the fallback without
    /// repeating itself on every copy.
    pub fn take_fallback_notice(&mut self) -> Option<String> {
        if self.warned {
            return None;
        }
        self.warned = true;
        if self.too_large {
            return Some(format!(
                "Selection too large for the terminal clipboard ({} KiB); kept inside the editor",
                OSC52_LIMIT / 1024
            ));
        }
        Some(match &self.system_error {
            Some(err) => format!("System clipboard unavailable ({err}); using internal clipboard"),
            None => "System clipboard unavailable; using internal clipboard".to_string(),
        })
    }
}

/// The escape sequence that asks the terminal to put `text` on the clipboard.
///
/// `OSC 52 ; c ; <base64> ST`, where `c` is the clipboard selection. `ST` rather
/// than `BEL` because it is the form the standard asks for and every terminal
/// that implements OSC 52 accepts it.
pub fn osc52_sequence(text: &str) -> Option<String> {
    if text.is_empty() || text.len() > OSC52_LIMIT {
        return None;
    }
    Some(format!("\x1b]52;c;{}\x1b\\", base64(text.as_bytes())))
}

/// Base64, standard alphabet with padding.
///
/// Twenty lines rather than a dependency: this is the only place the editor
/// needs it, and a clipboard is not the place to take on a crate.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipboard with no pasteboard, so the layers below it are the ones
    /// under test.
    fn without_system(mode: ClipboardMode) -> Clipboard {
        Clipboard {
            mode,
            system: None,
            internal: String::new(),
            pending: None,
            warned: false,
            system_error: None,
            too_large: false,
        }
    }

    #[test]
    fn internal_buffer_roundtrips() {
        let mut c = without_system(ClipboardMode::Internal);
        assert_eq!(c.get(), None);
        let outcome = c.set("hello");
        assert_eq!(outcome, ClipboardWrite::InternalFallback);
        assert!(outcome.is_fallback());
        assert_eq!(c.get(), Some("hello".to_string()));
        assert_eq!(c.internal(), "hello");
        assert!(
            c.take_terminal_sequence().is_none(),
            "nothing was asked of the terminal"
        );
    }

    #[test]
    fn the_terminal_mode_queues_one_sequence() {
        let mut c = without_system(ClipboardMode::Terminal);
        assert_eq!(c.set("hello"), ClipboardWrite::Terminal);
        let sequence = c.take_terminal_sequence().expect("a sequence");
        assert_eq!(sequence, "\x1b]52;c;aGVsbG8=\x1b\\");
        assert!(
            c.take_terminal_sequence().is_none(),
            "and only once: a second frame must not send it again"
        );
        // The internal copy is kept either way, so an in-editor paste works.
        assert_eq!(c.get(), Some("hello".to_string()));
    }

    #[test]
    fn the_system_mode_never_asks_the_terminal() {
        let mut c = without_system(ClipboardMode::System);
        // No pasteboard in this fixture, so the copy falls back — and does not
        // reach for the terminal, because the mode said not to.
        assert_eq!(c.set("hello"), ClipboardWrite::InternalFallback);
        assert!(c.take_terminal_sequence().is_none());
    }

    #[test]
    fn auto_falls_back_to_the_terminal() {
        let mut c = without_system(ClipboardMode::Auto);
        assert_eq!(c.set("hi"), ClipboardWrite::Terminal);
        assert_eq!(
            c.take_terminal_sequence().as_deref(),
            Some("\x1b]52;c;aGk=\x1b\\")
        );
    }

    #[test]
    fn base64_matches_the_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // A multi-byte character has to survive as its UTF-8 bytes.
        assert_eq!(base64("é".as_bytes()), "w6k=");
    }

    #[test]
    fn a_large_selection_is_not_sent_to_the_terminal() {
        let mut c = without_system(ClipboardMode::Terminal);
        let big = "x".repeat(OSC52_LIMIT + 1);
        assert_eq!(c.set(&big), ClipboardWrite::InternalFallback);
        assert!(c.take_terminal_sequence().is_none());
        let notice = c.take_fallback_notice().expect("it says why");
        assert!(notice.contains("too large"), "{notice}");

        // One byte under the limit still goes.
        let ok = "x".repeat(OSC52_LIMIT);
        assert_eq!(c.set(&ok), ClipboardWrite::Terminal);
        assert!(c.take_terminal_sequence().is_some());
    }

    #[test]
    fn an_empty_selection_is_not_sent() {
        assert!(osc52_sequence("").is_none());
    }

    #[test]
    fn the_mode_parses_from_configuration() {
        assert_eq!(ClipboardMode::parse("auto"), Some(ClipboardMode::Auto));
        assert_eq!(ClipboardMode::parse(""), Some(ClipboardMode::Auto));
        assert_eq!(ClipboardMode::parse("SYSTEM"), Some(ClipboardMode::System));
        assert_eq!(
            ClipboardMode::parse(" terminal "),
            Some(ClipboardMode::Terminal)
        );
        assert_eq!(ClipboardMode::parse("osc52"), Some(ClipboardMode::Terminal));
        assert_eq!(
            ClipboardMode::parse("internal"),
            Some(ClipboardMode::Internal)
        );
        assert_eq!(ClipboardMode::parse("pasteboard"), None);
    }

    #[test]
    fn fallback_notice_is_emitted_once() {
        let mut c = Clipboard {
            mode: ClipboardMode::Internal,
            system: None,
            internal: String::new(),
            pending: None,
            warned: false,
            system_error: Some("no display".into()),
            too_large: false,
        };
        assert!(c.take_fallback_notice().is_some());
        assert!(c.take_fallback_notice().is_none());
    }

    #[test]
    fn empty_internal_buffer_reads_as_none() {
        let mut c = without_system(ClipboardMode::Internal);
        c.set("");
        assert_eq!(c.get(), None);
    }
}
