//! Diagnostics.
//!
//! Logs go to a file, never to stdout/stderr: writing to the terminal while the
//! alternate screen is active would corrupt the UI. The log file is rotated per
//! process run so a crash report is never mixed with unrelated sessions, and
//! `--verbose` raises the level to `debug` without a recompile.

use std::path::{Path, PathBuf};

use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::time::uptime;
use tracing_subscriber::EnvFilter;

/// Keeps the background log writer alive for the process lifetime.
pub struct LogGuard {
    _worker: Option<WorkerGuard>,
    pub path: Option<PathBuf>,
}

impl LogGuard {
    /// A no-op guard, used when logging is disabled (tests, `--no-log`).
    pub fn disabled() -> Self {
        Self {
            _worker: None,
            path: None,
        }
    }
}

/// Initialise file logging.
///
/// Returns a guard that must be kept alive; dropping it flushes the writer.
pub fn init(dir: &Path, verbose: bool, quiet: bool) -> LogGuard {
    if quiet {
        return LogGuard::disabled();
    }
    if std::fs::create_dir_all(dir).is_err() {
        return LogGuard::disabled();
    }

    // One file per run keeps crash reports self-contained.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file = dir.join(format!("vela-{}-{stamp}.log", std::process::id()));

    let appender = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        Ok(f) => f,
        Err(_) => return LogGuard::disabled(),
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let level = if verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();

    let ok = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_timer(uptime())
        .with_thread_ids(false)
        .try_init()
        .is_ok();

    if !ok {
        return LogGuard::disabled();
    }

    tracing::info!(
        version = crate::VERSION,
        os = crate::platform::os_name(),
        "vela starting"
    );

    LogGuard {
        _worker: Some(guard),
        path: Some(file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_guard_has_no_path() {
        let g = LogGuard::disabled();
        assert!(g.path.is_none());
    }

    #[test]
    fn writes_a_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("logs");
        let guard = init(&sub, true, false);
        if let Some(path) = guard.path.clone() {
            tracing::info!("hello from the test");
            drop(guard);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(text.contains("hello from the test"), "log: {text}");
        }
    }
}
