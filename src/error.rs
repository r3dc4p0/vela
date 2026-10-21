//! Typed errors with actionable messages.
//!
//! The editor never panics on user input; every recoverable failure is surfaced
//! as a [`Error`] and reported in the status/message area.

use std::path::PathBuf;

use thiserror::Error;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong during normal operation.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot save {path}: {source}")]
    Save {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not a text file (binary data detected)")]
    Binary { path: PathBuf },

    #[error("{path} uses an unsupported encoding ({encoding}); Vela edits UTF-8 and UTF-16 (BOM) files only")]
    UnsupportedEncoding { path: PathBuf, encoding: String },

    #[error("{0} is a directory")]
    IsDirectory(PathBuf),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("invalid regular expression: {0}")]
    Regex(String),

    #[error("no file name given")]
    NoFileName,

    #[error("buffer is empty")]
    EmptyBuffer,

    #[error("{0}")]
    Message(String),
}

impl Error {
    /// Convenience constructor for `Error::Open`.
    pub fn open(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Open {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor for `Error::Save`.
    pub fn save(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Save {
            path: path.into(),
            source,
        }
    }

    /// Short, user facing text (used in the status bar).
    pub fn user_message(&self) -> String {
        self.to_string()
    }
}
