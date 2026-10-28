//! File lifecycle: opening, saving, closing, recents and session restore.
//!
//! Save policy (see also [`crate::filesystem`]):
//!
//! * normalisations configured by the user (trim trailing whitespace, ensure a
//!   final newline) are applied **to the buffer** first, so the in-memory text
//!   and the file on disk never diverge and the change is undoable;
//! * a buffer is only marked clean after `filesystem::save` reports success;
//! * an external modification is detected before overwriting and confirmed with
//!   the user instead of silently clobbering another process's work.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::{App, Job, ASYNC_OPEN_THRESHOLD};
use crate::buffer::Document;
use crate::commands::Command;
use crate::editing;
use crate::filesystem::{self, LoadedFile};
use crate::indent;
use crate::ui::{Confirm, ConfirmAction, Level, Overlay, PathPrompt};

/// How long an open may take before the loader panel appears.
///
/// A five-megabyte file opens in about ten milliseconds, and a panel that
/// flashes for a single frame is worse than no panel at all — it reads as a
/// glitch. The status bar still says "Opening …" straight away, so the delay
/// hides nothing; it only keeps the *panel* off the screen for work that is
/// over before it could be read.
pub const LOADER_DELAY: Duration = Duration::from_millis(120);

/// Directories skipped by the file finder.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
    ".idea",
    ".DS_Store",
];

/// Maximum number of files the finder indexes.
const INDEX_LIMIT: usize = 50_000;

/// An open that is still in flight.
///
/// Shared with the worker thread through an `Arc`: the worker writes the byte
/// counter, the UI reads it while painting. That is the whole point of the
/// type — the number on screen is the number of bytes actually on disk, not an
/// estimate, which is why the bar can claim a percentage at all.
#[derive(Debug)]
pub struct Loading {
    pub path: PathBuf,
    /// Size from the file's metadata, or 0 when it could not be read.
    pub total: u64,
    started: Instant,
    done: AtomicU64,
    preparing: AtomicBool,
}

impl Loading {
    pub fn new(path: PathBuf, total: u64) -> Self {
        Self {
            path,
            total,
            started: Instant::now(),
            done: AtomicU64::new(0),
            preparing: AtomicBool::new(false),
        }
    }

    /// File name without its directory, for the panel.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// True once the file is read and the buffer is being built.
    pub fn preparing(&self) -> bool {
        self.preparing.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Read fraction in `0.0..=1.0`, or `None` when the size is unknown.
    pub fn fraction(&self) -> Option<f32> {
        if self.total == 0 {
            return None;
        }
        Some((self.done() as f64 / self.total as f64).clamp(0.0, 1.0) as f32)
    }

    /// Called by the worker, once per chunk read.
    pub fn set_done(&self, bytes: u64) {
        self.done.store(bytes, Ordering::Relaxed);
    }

    /// Called by the worker when reading is over and the buffer is being built.
    pub fn start_preparing(&self) {
        self.preparing.store(true, Ordering::Relaxed);
    }
}

/// A file that finished loading, with its buffer already built.
///
/// The rope is constructed on the worker rather than on the UI thread: for a
/// two-gigabyte file that is 0.7 s, and doing it on the UI thread would freeze
/// the very frame the loader exists to cover.
pub struct OpenedBuffer {
    pub document: Document,
    pub mixed_eol: bool,
}

/// Turn a freshly loaded file into a document, ready to install.
fn build_document(path: &Path, loaded: LoadedFile, indent: indent::Options) -> OpenedBuffer {
    let mut doc = Document::from_text(&loaded.text, Some(path.to_path_buf()));
    // The file's own indentation, read before the text is dropped: this is the
    // last place it is cheap to look at.
    doc.set_indent_style(indent::detect(&loaded.text, indent));
    doc.set_eol(loaded.eol);
    doc.set_encoding(loaded.encoding, loaded.bom);
    doc.set_disk_meta(loaded.mtime, Some(loaded.len));
    doc.mark_saved();
    OpenedBuffer {
        document: doc,
        mixed_eol: loaded.mixed_eol,
    }
}

impl App {
    // ------------------------------------------------------------- opening

    /// Open `path`, reusing an existing buffer when the file is already open.
    ///
    /// Large files are loaded on a worker thread; the UI keeps running and the
    /// result is merged in [`App::poll_jobs`].
    pub fn open_path(&mut self, path: PathBuf) {
        let path = self.resolve(&path);
        let indent = self.indent_options();

        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.path().map(|p| same_file(p, &path)).unwrap_or(false))
        {
            self.set_active_buffer(idx);
            self.info(format!("{} is already open", path.display()));
            return;
        }

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > ASYNC_OPEN_THRESHOLD {
            self.open_generation += 1;
            let generation = self.open_generation;
            let tx = self.tx.clone();
            let worker_path = path.clone();
            let loading = Arc::new(Loading::new(path.clone(), size));
            let progress = Arc::clone(&loading);
            std::thread::spawn(move || {
                let result =
                    filesystem::load_with(&worker_path, |n| progress.set_done(n)).map(|loaded| {
                        progress.start_preparing();
                        build_document(&worker_path, loaded, indent)
                    });
                let _ = tx.send(Job::Opened {
                    generation,
                    path: worker_path,
                    result: Box::new(result),
                });
            });
            self.loading = Some(loading);
            self.info(format!(
                "Opening {} ({:.1} MiB)…",
                path.display(),
                size as f64 / (1024.0 * 1024.0)
            ));
            return;
        }

        match filesystem::load(&path) {
            Ok(loaded) => {
                let name = path.display().to_string();
                let opened = build_document(&path, loaded, indent);
                self.install(path, opened);
                self.success(format!("Opened {name}"));
            }
            // `vibe notes.md` on a file that does not exist yet: open an empty
            // buffer bound to that path so the first save creates the file.
            Err(crate::Error::Open { ref source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                let mut doc = Document::from_text("", Some(path.clone()));
                doc.set_eol(crate::filesystem::Eol::Lf);
                doc.mark_saved();
                self.adopt(doc);
                self.search.clear();
                self.info(format!("New file {}", path.display()));
            }
            Err(e) => self.error(e.user_message()),
        }
    }

    /// Put an already-built buffer in place.
    ///
    /// Split from [`build_document`] so the worker can hand over a finished
    /// document: everything expensive has happened by the time this runs, and
    /// all that is left is a `Vec` push.
    pub(crate) fn install(&mut self, path: PathBuf, opened: OpenedBuffer) {
        self.adopt(opened.document);
        self.add_recent(&path);
        self.search.clear();

        if opened.mixed_eol {
            self.warn(format!(
                "{} had mixed line endings; normalised to {}",
                path.display(),
                self.buffers[self.active()].eol().label()
            ));
        }
    }

    /// Put a freshly opened document into the buffer list.
    ///
    /// A pristine untitled buffer is replaced rather than left behind as an
    /// empty tab — but only when a single pane is showing it. Two panes on the
    /// blank buffer are two views of one document, and swapping it for the file
    /// would rewrite the other view behind the user's back: the split they just
    /// made would silently collapse. In that case the file is appended and
    /// loaded into the pane with the focus, which leaves the other pane showing
    /// the scratch buffer it had.
    /// Returns the index the document ended up at.
    pub(crate) fn adopt(&mut self, doc: Document) -> usize {
        match self.replaceable_blank() {
            Some(idx) => {
                let old = self.buffers[idx].id();
                self.syntax_cache.remove(&old);
                self.brackets_cache_reset();
                self.buffers[idx] = doc;
                // The viewport described the blank buffer, not the file.
                for pane in &mut self.panes {
                    if pane.buffer == idx {
                        pane.top = 0;
                        pane.row = 0;
                        pane.left = 0;
                        pane.detached = false;
                    }
                }
                self.install_active_view();
                idx
            }
            None => {
                self.buffers.push(doc);
                self.ensure_panes();
                self.set_active_buffer(self.buffers.len() - 1);
                self.buffers.len() - 1
            }
        }
    }

    /// The pristine untitled buffer a new file may take the place of.
    ///
    /// It has to be the buffer the file would be loaded into — the one in the
    /// pane with the focus — and it has to be a blank scratch that no other
    /// pane is looking at. Two views of one blank buffer cannot be replaced:
    /// the file would appear in both, rewriting the split the user just made.
    fn replaceable_blank(&self) -> Option<usize> {
        let idx = self.panes.get(self.active_pane)?.buffer;
        let doc = self.buffers.get(idx)?;
        if doc.path().is_some() || doc.is_modified() || doc.len_chars() != 0 {
            return None;
        }
        let viewers = self.panes.iter().filter(|p| p.buffer == idx).count();
        (viewers <= 1).then_some(idx)
    }

    /// Turn a possibly relative path into an absolute one.
    pub fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    pub fn new_buffer(&mut self) {
        self.buffers.push(Document::untitled());
        self.set_active_buffer(self.buffers.len() - 1);
        self.search.clear();
        self.info("New buffer");
    }

    // ------------------------------------------------------------- closing

    pub fn close_active(&mut self) {
        if let Some(idx) = self.active_index() {
            self.close_buffer_at(idx);
        }
    }

    pub(crate) fn close_buffer_at(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        if self.buffers[idx].is_modified() && self.config.editor.confirm_unsaved {
            let name = self.buffers[idx].display_name();
            self.ui.overlay = Overlay::Confirm(Confirm {
                prompt: format!("{name} has unsaved changes."),
                items: vec![
                    "Save and close".to_string(),
                    "Discard changes".to_string(),
                    "Cancel".to_string(),
                ],
                selected: 0,
                action: ConfirmAction::CloseBuffer(idx),
            });
            return;
        }
        self.remove_buffer(idx);
    }

    pub(crate) fn remove_buffer(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        let id = self.buffers[idx].id();
        self.syntax_cache.remove(&id);
        self.buffers.remove(idx);

        self.after_buffer_removed(idx);
        if self.buffers.is_empty() {
            if self.quit_pending {
                self.running = false;
            } else {
                self.buffers.push(Document::untitled());
                self.panes.clear();
                self.ensure_panes();
            }
        }
        self.search.clear();
        self.brackets_cache_reset();
    }

    /// Switch to the buffer at `idx`, used by clicking its tab.
    pub fn focus_buffer(&mut self, idx: usize) {
        if idx >= self.buffers.len() || idx == self.active() {
            return;
        }
        self.set_active_buffer(idx);
        self.search.clear();
        self.brackets_cache_reset();
    }

    pub fn cycle_buffer(&mut self, delta: isize) {
        if self.buffers.len() < 2 {
            return;
        }
        let len = self.buffers.len() as isize;
        let mut next = self.active() as isize + delta;
        if next < 0 {
            next += len;
        }
        if next >= len {
            next -= len;
        }
        self.set_active_buffer(next as usize);
        self.search.clear();
        self.brackets_cache_reset();
    }

    // -------------------------------------------------------------- saving

    /// Save the active buffer, prompting for a path when it has none.
    pub fn save_active(&mut self, force_as: bool) {
        let Some(idx) = self.active_index() else {
            return;
        };
        if force_as || self.buffers[idx].path().is_none() {
            self.save_as_prompt();
            return;
        }
        self.write_buffer(idx, false);
    }

    pub fn save_all(&mut self) {
        let indices: Vec<usize> = (0..self.buffers.len()).collect();
        let mut saved = 0usize;
        for idx in indices {
            if self.buffers[idx].path().is_none() {
                if self.buffers[idx].is_modified() {
                    self.warn(format!(
                        "{} has no file name; use Save As",
                        self.buffers[idx].display_name()
                    ));
                }
                continue;
            }
            if self.write_buffer(idx, false) {
                saved += 1;
            }
        }
        if saved > 0 {
            self.success(format!("Saved {saved} buffer(s)"));
        } else {
            self.info("Nothing to save");
        }
    }

    /// Write buffer `idx` to its path.
    ///
    /// Returns `true` only when the bytes are on disk and the buffer was marked
    /// clean.
    pub(crate) fn write_buffer(&mut self, idx: usize, force: bool) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        let Some(path) = self.buffers[idx].path().map(Path::to_path_buf) else {
            return false;
        };

        // Refuse to overwrite another process's changes without confirmation.
        if !force && self.config.files.warn_external_change {
            let (mtime, len) = (self.buffers[idx].disk_mtime(), self.buffers[idx].disk_len());
            if filesystem::changed_on_disk(&path, mtime, len) {
                self.buffers[idx].set_externally_modified(true);
                let name = path.display().to_string();
                self.ui.overlay = Overlay::Confirm(Confirm {
                    prompt: format!("{name} changed on disk since it was opened."),
                    items: vec![
                        "Overwrite".to_string(),
                        "Reload from disk".to_string(),
                        "Cancel".to_string(),
                    ],
                    selected: 2,
                    action: ConfirmAction::OverwriteExternal,
                });
                return false;
            }
        }

        // Apply save-time normalisations to the buffer, not just to the bytes,
        // so memory and disk stay identical and the change can be undone.
        if self.config.files.trim_trailing_whitespace {
            editing::trim_trailing_whitespace(&mut self.buffers[idx]);
        }
        if self.config.files.ensure_final_newline {
            let doc = &mut self.buffers[idx];
            if doc.len_chars() > 0 {
                let last = doc.len_chars();
                let ends_with_newline = doc
                    .storage()
                    .char_at(last.saturating_sub(1))
                    .map(|c| c == '\n')
                    .unwrap_or(false);
                if !ends_with_newline {
                    doc.replace_range(last, last, "\n", crate::buffer::TxKind::Other);
                }
            }
        }

        let doc = &self.buffers[idx];
        let text = doc.text();
        let eol = doc.eol();
        let encoding = doc.encoding();
        let bom = doc.bom();

        match filesystem::save(&path, &text, eol, encoding, bom) {
            Ok(outcome) => {
                let doc = &mut self.buffers[idx];
                doc.mark_saved();
                let meta = filesystem::metadata_of(&path);
                doc.set_disk_meta(meta.map(|m| m.0), meta.map(|m| m.1));
                doc.set_externally_modified(false);
                self.add_recent(&path);
                self.success(format!(
                    "Saved {} ({} bytes)",
                    path.display(),
                    outcome.bytes_written
                ));
                info!(
                    "saved {} bytes to {}",
                    outcome.bytes_written,
                    path.display()
                );
                true
            }
            Err(e) => {
                self.error(e.user_message());
                warn!("save failed: {e}");
                false
            }
        }
    }

    /// Save buffer `idx` to a new path.
    pub(crate) fn save_to_path(&mut self, idx: usize, path: PathBuf) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        let path = self.resolve(&path);
        self.buffers[idx].set_path(Some(path.clone()));
        // A new path means the previous disk metadata is meaningless.
        self.buffers[idx].set_disk_meta(None, None);
        self.write_buffer(idx, true)
    }

    /// Reload the active buffer from disk, discarding in-memory changes.
    pub fn reload_active(&mut self) {
        let Some(idx) = self.active_index() else {
            return;
        };
        let Some(path) = self.buffers[idx].path().map(Path::to_path_buf) else {
            self.warn("Buffer has no file to reload");
            return;
        };
        if self.buffers[idx].is_modified() {
            self.ui.overlay = Overlay::Confirm(Confirm {
                prompt: format!(
                    "Discard changes to {} and reload from disk?",
                    path.display()
                ),
                items: vec!["Reload".to_string(), "Cancel".to_string()],
                selected: 1,
                action: ConfirmAction::Reload,
            });
            return;
        }
        self.reload_buffer(idx, &path);
    }

    pub(crate) fn reload_buffer(&mut self, idx: usize, path: &Path) {
        match filesystem::load(path) {
            Ok(loaded) => {
                let doc = &mut self.buffers[idx];
                doc.reset_contents(&loaded.text);
                doc.set_eol(loaded.eol);
                doc.set_encoding(loaded.encoding, loaded.bom);
                doc.set_disk_meta(loaded.mtime, Some(loaded.len));
                doc.set_externally_modified(false);
                doc.mark_saved();
                self.syntax_cache.remove(&doc.id());
                self.success(format!("Reloaded {}", path.display()));
            }
            Err(e) => self.error(e.user_message()),
        }
    }

    // ------------------------------------------------------------- quitting

    pub fn request_quit(&mut self, all: bool) {
        if self.has_unsaved() && self.config.editor.confirm_unsaved {
            self.quit_pending = true;
            let n = self.unsaved_count();
            let prompt = if all {
                format!("{n} buffer(s) have unsaved changes. Save them all before quitting?")
            } else {
                format!("{n} buffer(s) have unsaved changes.")
            };
            self.ui.overlay = Overlay::Confirm(Confirm {
                prompt,
                items: vec![
                    "Save all and quit".to_string(),
                    "Discard and quit".to_string(),
                    "Cancel".to_string(),
                ],
                selected: 2,
                action: ConfirmAction::Quit,
            });
            return;
        }
        self.running = false;
    }

    /// Save every modified buffer that has a path; returns true when all saved.
    pub(crate) fn save_all_modified(&mut self) -> bool {
        let indices: Vec<usize> = (0..self.buffers.len()).collect();
        let mut ok = true;
        for idx in indices {
            if !self.buffers[idx].is_modified() {
                continue;
            }
            if self.buffers[idx].path().is_none() {
                ok = false;
                continue;
            }
            if !self.write_buffer(idx, true) {
                ok = false;
            }
        }
        ok
    }

    pub(crate) fn quit_now(&mut self) {
        self.running = false;
        self.quit_pending = true;
    }

    // -------------------------------------------------------------- recents

    pub fn load_recent(&mut self) {
        if !self.persistence_on() {
            return;
        }
        let path = self.data_dir.join("recent");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        self.recent = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .take(self.config.files.recent_files)
            .collect();
    }

    pub fn add_recent(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        let limit = self.config.files.recent_files.max(1);
        self.recent.truncate(limit);
    }

    pub(crate) fn save_recent(&mut self) {
        if !self.persistence_on() {
            return;
        }
        let path = self.data_dir.join("recent");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = self
            .recent
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if std::fs::write(&path, body).is_err() {
            warn!("could not persist recent files");
        }
    }

    // -------------------------------------------------------------- session

    /// Persist open buffers and cursor positions.
    pub fn save_session(&mut self) {
        if !self.persistence_on() {
            return;
        }
        let dir = self.data_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let mut out = String::new();
        for doc in &self.buffers {
            let Some(path) = doc.path() else { continue };
            let (line, col) = doc.cursor_line_col();
            out.push_str(&format!("{}\t{}\t{}\n", path.to_string_lossy(), line, col));
        }
        let _ = std::fs::write(dir.join("session"), out);
    }

    /// Restore the previous session. Returns true when at least one buffer was
    /// reopened.
    pub fn restore_session(&mut self) -> bool {
        if !self.persistence_on() {
            return false;
        }
        let path = self.data_dir.join("session");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return false;
        };
        let mut restored = 0usize;
        for line in text.lines() {
            let mut parts = line.split('\t');
            let (Some(p), Some(l), Some(c)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let path = PathBuf::from(p);
            if !path.exists() {
                continue;
            }
            let before = self.buffers.len();
            self.open_path(path);
            if self.buffers.len() > before {
                if let Ok(l) = l.parse::<usize>() {
                    let col = c.parse::<usize>().unwrap_or(0);
                    if let Some(doc) = self.active_doc_mut() {
                        doc.goto_line_col(l, col);
                    }
                }
                restored += 1;
            }
        }
        restored > 0
    }

    // ---------------------------------------------------------- file finder

    /// Build the file index in the background if it is not ready yet.
    pub fn ensure_file_index(&mut self) {
        if self.file_index.is_some() || self.index_loading {
            return;
        }
        self.index_loading = true;
        let root = self.cwd.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let files = walk_files(&root, INDEX_LIMIT);
            let _ = tx.send(Job::Index(files));
        });
    }

    // ------------------------------------------------------------ open prompt

    pub fn open_prompt(&mut self) {
        let initial = format!("{}/", self.cwd.display());
        let mut prompt = PathPrompt::new(initial);
        prompt.completion = complete_path(prompt.input.value(), &self.cwd);
        self.ui.overlay = Overlay::Open(prompt);
    }

    pub fn save_as_prompt(&mut self) {
        let Some(idx) = self.active_index() else {
            return;
        };
        let initial = match self.buffers[idx].path() {
            Some(p) => p.to_string_lossy().to_string(),
            None => format!("{}/", self.cwd.display()),
        };
        let mut prompt = PathPrompt::new(initial);
        prompt.completion = complete_path(prompt.input.value(), &self.cwd);
        self.ui.overlay = Overlay::SaveAs(prompt);
    }
}

/// True when both paths point at the same file (best effort).
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Recursively collect files under `root`, skipping noise directories.
pub fn walk_files(root: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= limit {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out
}

/// Filesystem completion for the path prompts.
pub fn complete_path(input: &str, cwd: &Path) -> Vec<String> {
    let expanded = if let Some(rest) = input.strip_prefix("~/") {
        match dirs::home_dir() {
            Some(home) => format!("{}/{rest}", home.display()),
            None => input.to_string(),
        }
    } else {
        input.to_string()
    };

    let path = PathBuf::from(&expanded);
    let (dir, prefix) = if expanded.ends_with('/') {
        (path.clone(), String::new())
    } else {
        (
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| cwd.to_path_buf()),
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    };
    let dir = if dir.as_os_str().is_empty() {
        cwd.to_path_buf()
    } else if dir.is_absolute() {
        dir
    } else {
        cwd.join(dir)
    };

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with(&prefix) {
                return None;
            }
            if prefix.is_empty() && name.starts_with('.') {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some(if is_dir { format!("{name}/") } else { name })
        })
        .collect();
    out.sort();
    out.truncate(50);
    out
}

/// Resolve a completion selection into a full input value.
pub fn apply_completion(input: &str, choice: &str) -> String {
    if input.ends_with('/') {
        format!("{input}{choice}")
    } else {
        let parent = input.rfind('/').map(|i| &input[..=i]).unwrap_or("");
        format!("{parent}{choice}")
    }
}

/// Dialog actions handled by the confirm overlay.
pub(crate) fn handle_confirm(app: &mut App, action: ConfirmAction, choice: usize) {
    match (action, choice) {
        (ConfirmAction::Quit, 0) => {
            if app.save_all_modified() {
                app.quit_now();
            } else {
                app.ui.overlay = Overlay::None;
                app.warn("Some buffers could not be saved; fix the errors or discard");
            }
        }
        (ConfirmAction::Quit, 1) => app.quit_now(),
        (ConfirmAction::Quit, _) => {
            app.ui.overlay = Overlay::None;
            app.quit_pending = false;
        }
        (ConfirmAction::CloseBuffer(idx), 0) => {
            app.ui.overlay = Overlay::None;
            if app.write_buffer(idx, false) {
                app.remove_buffer(idx);
            }
        }
        (ConfirmAction::CloseBuffer(idx), 1) => {
            app.ui.overlay = Overlay::None;
            app.remove_buffer(idx);
        }
        (ConfirmAction::CloseBuffer(_), _) => app.ui.overlay = Overlay::None,
        (ConfirmAction::Reload, 0) => {
            app.ui.overlay = Overlay::None;
            if let Some(idx) = app.active_index() {
                if let Some(path) = app.buffers[idx].path().map(Path::to_path_buf) {
                    app.reload_buffer(idx, &path);
                }
            }
        }
        (ConfirmAction::Reload, _) => app.ui.overlay = Overlay::None,
        (ConfirmAction::OverwriteExternal, 0) => {
            app.ui.overlay = Overlay::None;
            if let Some(idx) = app.active_index() {
                app.write_buffer(idx, true);
            }
        }
        (ConfirmAction::OverwriteExternal, 1) => {
            app.ui.overlay = Overlay::None;
            if let Some(idx) = app.active_index() {
                if let Some(path) = app.buffers[idx].path().map(Path::to_path_buf) {
                    app.reload_buffer(idx, &path);
                }
            }
        }
        (ConfirmAction::OverwriteExternal, _) => {
            app.ui.overlay = Overlay::None;
            app.info("Save cancelled");
        }
    }
}

/// Helper for tests and diagnostics: list of open paths.
impl App {
    pub fn open_paths(&self) -> Vec<PathBuf> {
        self.buffers
            .iter()
            .filter_map(|d| d.path().map(|p| p.to_path_buf()))
            .collect()
    }

    /// Execute a command by id (used by tests and future scripting).
    pub fn execute_id(&mut self, id: &str) -> bool {
        match Command::from_id(id) {
            Some(cmd) => {
                self.execute(cmd);
                true
            }
            None => false,
        }
    }

    /// Convenience for the status line in tests.
    pub fn last_message(&self) -> Option<&str> {
        self.ui.status.as_ref().map(|m| m.text.as_str())
    }

    /// Message severity of the last message.
    pub fn last_level(&self) -> Option<Level> {
        self.ui.status.as_ref().map(|m| m.level)
    }
}
