//! Platform paths and capability probes.
//!
//! Everything that is OS specific but not terminal specific lives here, so the
//! rest of the code base never calls `dirs` or `std::env` directly.

use std::path::PathBuf;

use crate::APP_NAME;

/// The name this editor had before it was renamed.
///
/// Read, never reported. It is here so that a configuration file, a recent-files
/// list, a session — and above all an unsaved recovery snapshot — written under
/// the old name is not abandoned on the first start after the rename.
const LEGACY_APP_NAME: &str = "vela";

/// Move the old configuration and data directories into place, once.
///
/// Called from `main` and nowhere else. A test builds an `App` that must never
/// touch the user's profile, and this is the one thing in here that would.
pub fn adopt_legacy_dirs() {
    for root in [dirs::config_dir(), dirs::data_dir()].into_iter().flatten() {
        adopt_in(&root);
    }
}

/// The same move, under one root, so it can be tested without a profile.
///
/// Entry by entry rather than as one directory rename, because the new
/// directory can already exist by the time this runs — a log file is enough —
/// and a single `rename` would then either fail or, worse, be skipped for good.
/// An entry that is already present under the new name is left alone: whatever
/// is there is newer than what the old name has.
fn adopt_in(root: &std::path::Path) {
    let old = root.join(LEGACY_APP_NAME);
    let new = root.join(APP_NAME);
    if !old.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&old) else {
        return;
    };
    let _ = std::fs::create_dir_all(&new);
    for entry in entries.flatten() {
        let target = new.join(entry.file_name());
        if !target.exists() {
            let _ = std::fs::rename(entry.path(), target);
        } else if target.is_dir() && entry.path().is_dir() {
            // A directory on both sides. `logs` is the one that happens: the
            // logger can create it before this runs. Merge it, or the copy
            // under the old name is stranded there for good.
            if let Ok(inner) = std::fs::read_dir(entry.path()) {
                for file in inner.flatten() {
                    let to = target.join(file.file_name());
                    if !to.exists() {
                        let _ = std::fs::rename(file.path(), to);
                    }
                }
            }
            let _ = std::fs::remove_dir(entry.path());
        }
    }
    // Succeeds only when everything moved; anything left behind keeps the
    // directory, which is the honest outcome.
    let _ = std::fs::remove_dir(&old);
}

/// Directory holding configuration (`~/.config/vibe` on Unix).
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
}

/// Directory holding logs and session state (`~/.local/share/vibe` on Unix).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_directory_is_adopted_once() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(LEGACY_APP_NAME);
        std::fs::create_dir_all(old.join("recovery")).unwrap();
        std::fs::write(old.join("session"), "one file\n").unwrap();

        adopt_in(dir.path());

        assert!(!old.exists(), "the old directory is gone");
        let adopted = dir.path().join(APP_NAME);
        assert_eq!(
            std::fs::read_to_string(adopted.join("session")).unwrap(),
            "one file\n",
            "with its contents"
        );
    }

    #[test]
    fn an_existing_directory_is_never_overwritten() {
        // The rename must not cost somebody the session they already have
        // under the new name.
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(LEGACY_APP_NAME);
        let new = dir.path().join(APP_NAME);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("session"), "old").unwrap();
        std::fs::write(new.join("session"), "new").unwrap();

        adopt_in(dir.path());

        assert_eq!(std::fs::read_to_string(new.join("session")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old.join("session")).unwrap(), "old");
    }

    #[test]
    fn the_new_directory_may_already_exist() {
        // The logger creates `logs` before this runs, and a `rename` of the
        // whole directory would then be skipped for good. The entries still
        // move, and the one that is already there is left alone.
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(LEGACY_APP_NAME);
        let new = dir.path().join(APP_NAME);
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::create_dir_all(new.join("logs")).unwrap();
        std::fs::write(old.join("recent"), "/tmp/a\n").unwrap();
        std::fs::write(old.join("logs").join("old.log"), "old").unwrap();
        std::fs::write(new.join("logs").join("new.log"), "new").unwrap();

        adopt_in(dir.path());

        assert_eq!(
            std::fs::read_to_string(new.join("recent")).unwrap(),
            "/tmp/a\n"
        );
        assert!(new.join("logs").join("new.log").exists(), "not replaced");
        assert!(
            new.join("logs").join("old.log").exists(),
            "the old logs were merged in, not stranded"
        );
        assert!(!old.join("recent").exists(), "moved, not copied");
        assert!(!old.exists(), "and the old directory is gone");
    }

    #[test]
    fn nothing_happens_without_an_old_directory() {
        let dir = tempfile::tempdir().unwrap();
        adopt_in(dir.path());
        assert!(!dir.path().join(APP_NAME).exists());
    }
}
