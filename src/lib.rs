//! Vela — a modern, fast terminal text editor.
//!
//! The crate is split into a reusable library (`vela`) and a thin binary
//! (`src/main.rs`). Everything that is not terminal I/O is testable without a
//! TTY, which is why the editor core lives here rather than in `main`.
//!
//! Module map:
//!
//! | module         | responsibility                                             |
//! |----------------|------------------------------------------------------------|
//! | [`buffer`]     | text storage, cursor, selection, undo history              |
//! | [`editing`]    | higher level edit operations built on `buffer` primitives  |
//! | [`search`]     | query model, literal/regex matching, replacement           |
//! | [`filesystem`] | loading, atomic saving, encoding and line endings          |
//! | [`commands`]   | command registry, metadata and dispatch                    |
//! | [`keymap`]     | key chord parsing and binding resolution                   |
//! | [`input`]      | terminal event normalisation                               |
//! | [`ui`]         | layout, painting, overlays                                 |
//! | [`app`]        | application state machine and event loop                   |
//! | [`config`]     | TOML configuration                                         |
//! | [`theme`]      | colour palettes                                            |
//! | [`clipboard`]  | system clipboard with internal fallback                    |
//! | [`syntax`]     | lightweight incremental line lexer                         |
//! | [`terminal`]   | terminal setup/teardown and capability detection           |
//! | [`logging`]    | file based diagnostics                                     |

pub mod app;
pub mod autopair;
pub mod buffer;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod editing;
pub mod error;
pub mod filesystem;
pub mod fuzzy;
pub mod indent;
pub mod input;
pub mod keymap;
pub mod logging;
pub mod platform;
pub mod recovery;
pub mod search;
pub mod syntax;
pub mod terminal;
pub mod theme;
pub mod ui;

pub use error::{Error, Result};

/// Application name used in the UI and in paths.
pub const APP_NAME: &str = "vela";

/// Semantic version, shown in the top bar and `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
