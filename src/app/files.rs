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

use tracing::{info, warn};

use super::{App, Job, ASYNC_OPEN_THRESHOLD};
use crate::buffer::Document;
use crate::commands::Command;
use crate::editing;
use crate::filesystem::{self, LoadedFile};
use crate::platform;
use crate::ui::{Confirm, ConfirmAction, Level, Overlay, PathPrompt};

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

impl App {
    // ------------------------------------------------------------- opening

    /// Open `path`, reusing an existing buffer when the file is already open.
    ///
    /// Large files are loaded on a worker thread; the UI keeps running and the
    /// result is merged in [`App::poll_jobs`].
    pub fn open_path(&mut self, path: PathBuf) {
        let path = self.resolve(&path);

        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.path().map(|p| same_file(p, &path)).unwrap_or(false))
        {
            self.active = idx;
            self.info(format!("{} is already open", path.display()));
            return;
        }

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > ASYNC_OPEN_THRESHOLD {
            self.open_generation += 1;
            let generation = self.open_generation;
            let tx = self.tx.clone();
            let worker_path = path.clone();
            std::thread::spawn(move || {
                let result = filesystem::load(&worker_path);
                let _ = tx.send(Job::Opened {
                    generation,
                    path: worker_path,
                    result: Box::new(result),
                });
            });
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
                self.add_loaded(path, loaded);
                self.success(format!("Opened {name}"));
            }
            // `vela notes.md` on a file that does not exist yet: open an empty
            // buffer bound to that path so the first save creates the file.
            Err(crate::Error::Open { ref source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                let mut doc = Document::from_text("", Some(path.clone()));
                doc.set_eol(crate::filesystem::Eol::Lf);
                doc.mark_saved();
                let replace_blank = self.buffers.len() == 1
                    && self.buffers[0].path().is_none()
                    && !self.buffers[0].is_modified()
                    && self.buffers[0].len_chars() == 0;
                if replace_blank {
                    self.buffers.clear();
                }
                self.buffers.push(doc);
                self.active = self.buffers.len() - 1;
                self.search.clear();
                self.info(format!("New file {}", path.display()));
            }
            Err(e) => self.error(e.user_message()),
        }
    }

    /// Insert a freshly loaded file as a new buffer.
    pub(crate) fn add_loaded(&mut self, path: PathBuf, loaded: LoadedFile) {
        let mut doc = Document::from_text(&loaded.text, Some(path.clone()));
        doc.set_eol(loaded.eol);
        doc.set_encoding(loaded.encoding, loaded.bom);
        doc.set_disk_meta(loaded.mtime, Some(loaded.len));
        doc.mark_saved();

        // Replace a pristine untitled buffer rather than leaving a blank tab.
        let replace_blank = self.buffers.len() == 1
            && self.buffers[0].path().is_none()
            && !self.buffers[0].is_modified()
            && self.buffers[0].len_chars() == 0;
        if replace_blank {
            self.buffers.clear();
        }

        self.buffers.push(doc);
        self.active = self.buffers.len() - 1;
        self.add_recent(&path);
        self.search.clear();

        if loaded.mixed_eol {
            self.warn(format!(
                "{} had mixed line endings; normalised to {}",
                path.display(),
                loaded.eol.label()
            ));
        }
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
        self.active = self.buffers.len() - 1;
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

        if self.buffers.is_empty() {
            if self.quit_pending {
                self.running = false;
            } else {
                self.buffers.push(Document::untitled());
                self.active = 0;
            }
        } else if self.active > idx {
            self.active -= 1;
        } else if self.active >= self.buffers.len() {
            self.active = self.buffers.len() - 1;
        }
        self.search.clear();
        self.brackets_cache_reset();
    }

    /// Switch to the buffer at `idx`, used by clicking its tab.
    pub fn focus_buffer(&mut self, idx: usize) {
        if idx >= self.buffers.len() || idx == self.active {
            return;
        }
        self.active = idx;
        self.search.clear();
        self.brackets_cache_reset();
    }

    pub fn cycle_buffer(&mut self, delta: isize) {
        if self.buffers.len() < 2 {
            return;
        }
        let len = self.buffers.len() as isize;
        let mut next = self.active as isize + delta;
        if next < 0 {
            next += len;
        }
        if next >= len {
            next -= len;
        }
        self.active = next as usize;
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
        if !self.persist {
            return;
        }
        let path = platform::recent_files_path();
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
        if !self.persist {
            return;
        }
        let path = platform::recent_files_path();
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
        if !self.persist {
            return;
        }
        let dir = platform::data_dir();
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
        if !self.persist {
            return false;
        }
        let path = platform::data_dir().join("session");
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
