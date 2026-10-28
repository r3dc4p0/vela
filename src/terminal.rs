//! Terminal lifecycle: setup, teardown and capability detection.
//!
//! # Guaranteed restoration
//!
//! Raw mode, the alternate screen, mouse reporting, bracketed paste and the
//! Kitty keyboard protocol all leave the terminal in a state the shell cannot
//! recover from. Three mechanisms make sure they are undone:
//!
//! 1. [`TerminalGuard`]'s `Drop`, for normal exits and recoverable errors;
//! 2. a panic hook, because release builds abort on panic and never unwind;
//! 3. a `SIGTERM`/`SIGINT`/`SIGHUP` handler, for `kill` and closed windows.
//!
//! # Keyboard protocol detection
//!
//! crossterm's `supports_keyboard_enhancement()` writes a query and waits up to
//! **two seconds** for an answer. That would dominate startup time on terminals
//! that ignore the query, so Vibe does not probe by default. Instead `auto` mode
//! consults `$TERM` / `$TERM_PROGRAM` against a list of terminals known to
//! implement the protocol, and users can force it on or off with
//! `[terminal] keyboard_enhancement = "on"` or the `VIBE_ENHANCED_KEYS`
//! environment variable.

use std::io::{self, Stdout, Write};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tracing::{debug, info};

use crate::config::Enhancement;

/// The concrete backend the application draws into.
pub type VibeBackend = CrosstermBackend<Stdout>;
/// The concrete terminal type.
pub type VibeTerminal = Terminal<VibeBackend>;

/// True when the terminal is believed to speak the Kitty keyboard protocol.
///
/// Detection is name based on purpose: it is instantaneous and never blocks.
pub fn terminal_supports_enhancement() -> bool {
    let term = std::env::var("TERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();

    if std::env::var_os("KITTY_WINDOW_ID").is_some() || std::env::var_os("WEZTERM_PANE").is_some() {
        return true;
    }
    if term.contains("kitty") || term.contains("wezterm") || term.contains("ghostty") {
        return true;
    }
    matches!(
        program.as_str(),
        "iterm.app" | "wezterm" | "ghostty" | "vscode" | "kitty" | "rio" | "tabby" | "warpterminal"
    )
}

/// Decide whether to push the enhancement flags.
pub fn should_enable_enhancement(mode: Enhancement) -> bool {
    // An explicit environment override always wins, so users can fix a
    // mis-detected terminal without editing a config file.
    if let Ok(value) = std::env::var("VIBE_ENHANCED_KEYS") {
        if let Some(m) = Enhancement::parse(&value) {
            return match m {
                Enhancement::On => true,
                Enhancement::Off => false,
                Enhancement::Auto => terminal_supports_enhancement(),
            };
        }
    }
    match mode {
        Enhancement::On => true,
        Enhancement::Off => false,
        Enhancement::Auto => terminal_supports_enhancement(),
    }
}

/// Owns the terminal for the lifetime of the application.
pub struct TerminalGuard {
    enhanced: bool,
    active: bool,
}

impl TerminalGuard {
    /// Enter raw mode, the alternate screen and the requested input modes.
    pub fn enter(mouse: bool, enhancement: Enhancement) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        execute!(stdout, EnableBracketedPaste)?;
        if mouse {
            execute!(stdout, EnableMouseCapture)?;
        }

        let enhanced = should_enable_enhancement(enhancement);
        if enhanced {
            // If the terminal rejects the sequence we simply lose modifier
            // fidelity; that is a documented degradation, not an error.
            let pushed = execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
            if pushed.is_err() {
                debug!("keyboard enhancement rejected by terminal");
            }
        }

        let guard = Self {
            enhanced,
            active: true,
        };

        // Restore on panic even with `panic = "abort"`.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));

        info!(
            enhanced_keys = enhanced,
            mouse,
            term = %crate::platform::term_name(),
            "terminal initialised"
        );
        Ok(guard)
    }

    /// Build a ratatui terminal on stdout.
    pub fn terminal(&self) -> io::Result<VibeTerminal> {
        let backend = CrosstermBackend::new(io::stdout());
        Terminal::new(backend)
    }

    /// True when the enhanced keyboard protocol is active.
    pub fn enhanced_keys(&self) -> bool {
        self.enhanced
    }

    /// Undo everything. Safe to call more than once.
    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        restore_terminal();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Best-effort terminal restoration, callable from signal and panic handlers.
pub fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture);
    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    let _ = execute!(stdout, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = stdout.flush();
}

/// Install handlers for the signals that can kill us while in raw mode.
pub fn install_signal_handlers() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    for signal in [SIGTERM, SIGINT, SIGHUP, SIGQUIT] {
        // `low_level::register` is async-signal-safe enough for `write`/`ioctl`
        // work, which is all `restore_terminal` does.
        let _ = unsafe {
            signal_hook::low_level::register(signal, || {
                restore_terminal();
                std::process::exit(130);
            })
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enhancement_modes_parse() {
        assert_eq!(Enhancement::parse("auto"), Some(Enhancement::Auto));
        assert_eq!(Enhancement::parse("ON"), Some(Enhancement::On));
        assert_eq!(Enhancement::parse("off"), Some(Enhancement::Off));
        assert_eq!(Enhancement::parse("maybe"), None);
    }

    #[test]
    fn explicit_modes_do_not_depend_on_the_terminal() {
        assert!(should_enable_enhancement(Enhancement::On));
        assert!(!should_enable_enhancement(Enhancement::Off));
    }

    #[test]
    fn auto_detection_never_panics() {
        let _ = terminal_supports_enhancement();
    }
}
