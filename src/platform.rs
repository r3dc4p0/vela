//! Platform paths and capability probes.
//!
//! Everything that is OS specific but not terminal specific lives here, so the
//! rest of the code base never calls `dirs` or `std::env` directly.

use std::path::PathBuf;

use crate::APP_NAME;

/// Directory holding configuration (`~/.config/vela` on Unix).
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
}

/// Directory holding logs and session state (`~/.local/share/vela` on Unix).
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
}

/// Default configuration file path.
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Default log directory.
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Where the recent-files list is persisted.
pub fn recent_files_path() -> PathBuf {
    data_dir().join("recent")
}

/// Where an unsaved recovery snapshot is written.
pub fn recovery_dir() -> PathBuf {
    data_dir().join("recovery")
}

/// Human readable platform name, shown in `--version` and the help screen.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "unknown"
    }
}

/// Value of `$TERM`, used to explain degraded key support.
pub fn term_name() -> String {
    std::env::var("TERM").unwrap_or_else(|_| "unknown".to_string())
}

/// `$TERM_PROGRAM` (iTerm.app, Apple_Terminal, vscode, WezTerm, ...).
pub fn term_program() -> Option<String> {
    std::env::var("TERM_PROGRAM").ok()
}

/// True inside SSH sessions, where the system clipboard is usually absent.
pub fn is_remote_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// Current working directory, falling back to `.`.
pub fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
