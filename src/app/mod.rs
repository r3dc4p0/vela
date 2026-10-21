//! Application state and the event loop.
//!
//! `App` owns the buffers, the UI state and the configuration, and it is the
//! only place where a user intent turns into a document mutation. The flow is
//! always:
//!
//! ```text
//! terminal event -> input::translate -> Action -> App::execute / edit
//! ```
//!
//! Nothing in here knows about rendering details; `ui::draw` reads this state.

mod files;
mod prompts;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEventKind};
use ratatui::layout::Rect;
use tracing::{debug, error as log_error, info};

use crate::buffer::{Document, Motion, MotionCtx};
use crate::clipboard::Clipboard;
use crate::commands::Command;
use crate::config::Config;
use crate::editing;
use crate::filesystem::LoadedFile;
use crate::input::{self, Action};
use crate::keymap::Keymap;
use crate::platform;
use crate::search::{Query, SearchState};
use crate::syntax::StateCache;
use crate::theme::Theme;
use crate::ui::{self, Confirm, ConfirmAction, Level, Message, Overlay, PathPrompt, UiState};

/// Work handed to a background thread.
pub enum Job {
    /// A file finished loading.
    Opened {
        generation: u64,
        path: PathBuf,
        result: Box<Result<LoadedFile, crate::Error>>,
    },
    /// The file finder index finished building.
    Index(Vec<String>),
}

/// Cached bracket pair for the active buffer: (buffer id, revision, cursor,
/// pair). Recomputed only when one of the first three changes.
type BracketCache = Option<(usize, u64, usize, Option<(usize, usize)>)>;

/// Files larger than this are opened on a background thread so the UI keeps
/// painting (and can show "Opening …") instead of freezing.
const ASYNC_OPEN_THRESHOLD: u64 = 4 * 1024 * 1024;

/// Main application object.
pub struct App {
    pub buffers: Vec<Document>,
    pub active: usize,
    pub config: Config,
    pub theme: Theme,
    pub keymap: Keymap,
    pub clipboard: Clipboard,
    pub search: SearchState,
    pub ui: UiState,
    pub cwd: PathBuf,
    pub recent: Vec<PathBuf>,
    pub syntax_cache: HashMap<usize, StateCache>,
    pub running: bool,
    pub exit_code: i32,
    pub enhanced_keys: bool,
    pub config_path: PathBuf,
    pub config_warnings: Vec<String>,

    tx: Sender<Job>,
    rx: Receiver<Job>,
    open_generation: u64,
    pub file_index: Option<Vec<String>>,
    pub index_loading: bool,
    /// Set while waiting for the user to confirm quitting.
    quit_pending: bool,
    brackets_cache: BracketCache,
    /// Last mouse press, for double-click word selection.
    last_click: Option<(Instant, u16, u16)>,
    /// Editor area from the last frame, for mouse coordinate mapping.
    last_editor_area: Rect,
    /// Frames rendered so far (diagnostics).
    pub frame: u64,
    /// When false, recents and session state are neither read nor written.
    /// Integration tests use this so they never touch the real user profile.
    pub persist: bool,
}

impl App {
    /// Build the application.
    ///
    /// `paths` are the files named on the command line; when empty a single
    /// untitled buffer is created so the editor is always usable.
    pub fn new(
        config: Config,
        config_path: PathBuf,
        warnings: Vec<String>,
        enhanced_keys: bool,
        paths: Vec<PathBuf>,
    ) -> Self {
        let mut app = Self::base(config, config_path, warnings, enhanced_keys);

        app.load_recent();

        if paths.is_empty() {
            if app.config.editor.restore_session {
                if !app.restore_session() {
                    app.buffers.push(Document::untitled());
                }
            } else {
                app.buffers.push(Document::untitled());
            }
        } else {
            for path in paths {
                app.open_path(path);
            }
        }
        if app.buffers.is_empty() {
            app.buffers.push(Document::untitled());
        }

        // Surface configuration problems without blocking startup.
        let warnings = app.config_warnings.clone();
        for w in warnings {
            app.warn(w);
        }

        app
    }

    /// Build an application that never reads or writes user state.
    ///
    /// The integration tests drive the editor through this constructor so
    /// they are deterministic and never touch the real profile directory
    /// (recents, session file).
    pub fn headless(cwd: PathBuf, config: Config) -> Self {
        let mut app = Self::base(config, PathBuf::new(), Vec::new(), false);
        app.persist = false;
        app.cwd = cwd;
        app.clipboard = Clipboard::internal_only();
        app.buffers.push(Document::untitled());
        app
    }

    fn base(
        config: Config,
        config_path: PathBuf,
        warnings: Vec<String>,
        enhanced_keys: bool,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let theme = Theme::by_name(&config.ui.theme).unwrap_or_else(Theme::dark);

        let mut app = Self {
            buffers: Vec::new(),
            active: 0,
            config,
            theme,
            keymap: Keymap::defaults(),
            clipboard: Clipboard::new(),
            search: SearchState::new(),
            ui: UiState::default(),
            cwd: platform::current_dir(),
            recent: Vec::new(),
            syntax_cache: HashMap::new(),
            running: true,
            exit_code: 0,
            enhanced_keys,
            config_path,
            config_warnings: warnings,
            tx,
            rx,
            open_generation: 0,
            file_index: None,
            index_loading: false,
            quit_pending: false,
            brackets_cache: None,
            last_click: None,
            last_editor_area: Rect::default(),
            frame: 0,
            persist: true,
        };

        // Key overrides from configuration.
        let mut keymap = Keymap::defaults();
        let key_warnings = keymap.apply_overrides(&app.config.keys);
        app.keymap = keymap;
        for w in key_warnings {
            app.warn(w);
        }
        app.apply_theme_overrides();
        app
    }

    // ------------------------------------------------------------ accessors

    pub fn active_index(&self) -> Option<usize> {
        if self.buffers.is_empty() {
            None
        } else {
            Some(self.active.min(self.buffers.len() - 1))
        }
    }

    pub fn active_doc(&self) -> Option<&Document> {
        self.active_index().map(|i| &self.buffers[i])
    }

    pub fn active_doc_mut(&mut self) -> Option<&mut Document> {
        match self.active_index() {
            Some(i) => Some(&mut self.buffers[i]),
            None => None,
        }
    }

    /// True when any buffer has unsaved changes.
    pub fn has_unsaved(&self) -> bool {
        self.buffers.iter().any(|d| d.is_modified())
    }

    pub fn unsaved_count(&self) -> usize {
        self.buffers.iter().filter(|d| d.is_modified()).count()
    }

    pub fn motion_ctx(&self, page: usize) -> MotionCtx {
        MotionCtx {
            tab_width: self.config.tab_width(),
            page,
        }
    }

    // -------------------------------------------------------------- messages

    pub fn push_message(&mut self, text: impl Into<String>, level: Level) {
        let msg = Message::new(text, level);
        self.ui.status = Some(msg.clone());
        self.ui.messages.push(msg);
        self.ui.prune();
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push_message(text, Level::Info);
    }

    pub fn success(&mut self, text: impl Into<String>) {
        self.push_message(text, Level::Success);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.push_message(text, Level::Warn);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push_message(text, Level::Error);
    }

    // ------------------------------------------------------------------ loop

    /// Draw exactly one frame and return.
    ///
    /// Used by `vela --smoke` to verify that the editor can start, negotiate
    /// the terminal and paint in a real environment (CI, a new terminal
    /// emulator, a packaging check) without needing a human.
    pub fn run_once(&mut self, terminal: &mut crate::terminal::VelaTerminal) -> crate::Result<()> {
        terminal
            .draw(|frame| ui::draw(frame, self))
            .map_err(|e| crate::Error::Message(format!("render failed: {e}")))?;
        self.frame += 1;
        Ok(())
    }

    /// Run until the user quits.
    pub fn run(&mut self, terminal: &mut crate::terminal::VelaTerminal) -> crate::Result<()> {
        info!("event loop started");
        while self.running {
            terminal
                .draw(|frame| ui::draw(frame, self))
                .map_err(|e| crate::Error::Message(format!("render failed: {e}")))?;
            self.frame += 1;

            self.poll_jobs();

            // Wake up regularly for message expiry and background jobs, but do
            // not burn CPU when idle.
            let timeout = if self.ui.messages.is_empty() {
                Duration::from_millis(200)
            } else {
                Duration::from_millis(50)
            };

            match crossterm::event::poll(timeout) {
                Ok(true) => match crossterm::event::read() {
                    Ok(event) => self.handle_event(event),
                    Err(e) => {
                        log_error!("event read failed: {e}");
                        break;
                    }
                },
                Ok(false) => {}
                Err(e) => {
                    log_error!("event poll failed: {e}");
                    break;
                }
            }
        }
        self.on_exit();
        info!("event loop finished");
        Ok(())
    }

    /// Drain finished background jobs.
    pub fn poll_jobs(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Opened {
                    generation,
                    path,
                    result,
                } => {
                    if generation != self.open_generation {
                        debug!("discarding stale open result for {}", path.display());
                        continue;
                    }
                    match *result {
                        Ok(loaded) => {
                            let name = path.display().to_string();
                            self.add_loaded(path, loaded);
                            self.success(format!("Opened {name}"));
                        }
                        Err(e) => self.error(e.user_message()),
                    }
                }
                Job::Index(files) => {
                    self.index_loading = false;
                    let n = files.len();
                    self.file_index = Some(files);
                    debug!("file index built: {n} entries");
                    self.refresh_palette();
                }
            }
        }
    }

    fn on_exit(&mut self) {
        self.save_recent();
        if self.config.editor.restore_session {
            self.save_session();
        }
    }

    // ---------------------------------------------------------------- events

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return;
                }
                self.handle_key(key);
            }
            Event::Paste(text) => {
                if !self.ui.overlay.captures_input() {
                    self.insert_text(&input::translate_paste(&text).into_text());
                } else {
                    self.paste_into_prompt(&text);
                }
            }
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Resize(_, _) => {
                // ratatui resizes on the next draw; nothing to do.
            }
            Event::FocusGained | Event::FocusLost => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.ui.overlay.captures_input() {
            self.handle_overlay_key(key);
            return;
        }

        let action = input::translate(&key, &self.keymap);
        match action {
            Action::InsertText(text) => self.insert_text(&text),
            Action::InsertNewline => self.insert_newline(),
            Action::InsertTab => self.insert_tab(),
            Action::Dedent => self.dedent(),
            Action::Backspace => self.backspace(),
            Action::DeleteForward => self.delete_forward(),
            Action::Move { motion, extend } => self.move_cursor(motion, extend),
            Action::Command(cmd) => self.execute(cmd),
            Action::Escape => self.escape(),
            Action::Ignored => {}
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        if !self.config.ui.mouse || self.ui.overlay.captures_input() {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                let now = Instant::now();
                let is_double = self
                    .last_click
                    .map(|(t, x, y)| {
                        now.duration_since(t) < Duration::from_millis(400)
                            && x.abs_diff(mouse.column) <= 1
                            && y == mouse.row
                    })
                    .unwrap_or(false);
                self.last_click = Some((now, mouse.column, mouse.row));
                self.click_at(mouse.column, mouse.row, is_double);
            }
            _ => {}
        }
    }

    /// Map a screen position to a buffer position and move the cursor there.
    fn click_at(&mut self, column: u16, row: u16, select_word: bool) {
        let area = self.last_editor_area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        if row < area.y || row >= area.y + area.height {
            return;
        }
        let Some(idx) = self.active_index() else {
            return;
        };
        let cfg = &self.config;
        let digits = crate::ui::editor::decimal_digits(self.buffers[idx].line_count()).max(3);
        let gutter = if cfg.editor.line_numbers {
            (digits + 1) as u16
        } else {
            0
        };
        if column < area.x + gutter {
            return;
        }
        let rel_row = (row - area.y) as usize;
        let line = self.buffers[idx].scroll_top() + rel_row;
        if line >= self.buffers[idx].line_count() {
            return;
        }
        let target_col = (column - area.x - gutter) as usize + self.buffers[idx].scroll_left();
        let tab_width = cfg.tab_width();
        let doc = &mut self.buffers[idx];
        let line_start = doc.storage().line_start(line);
        let text = doc.line_text(line);
        let (offset, _) = crate::buffer::motion::char_at_visual_col(&text, target_col, tab_width);
        doc.set_cursor(line_start + offset);

        if select_word {
            let word = doc.word_at_cursor();
            if !word.is_empty() {
                let cursor = doc.cursor();
                let line_start = doc.storage().line_start(line);
                let text = doc.line_text(line);
                let chars: Vec<char> = text.chars().collect();
                let col = cursor - line_start;
                let is_word = |c: char| c.is_alphanumeric() || c == '_';
                let mut lo = col;
                while lo > 0 && is_word(chars[lo - 1]) {
                    lo -= 1;
                }
                let mut hi = col;
                while hi < chars.len() && is_word(chars[hi]) {
                    hi += 1;
                }
                doc.select_range(line_start + lo..line_start + hi);
            }
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let Some(idx) = self.active_index() else {
            return;
        };
        let doc = &mut self.buffers[idx];
        let top = doc.scroll_top() as isize + delta;
        let max = doc.line_count().saturating_sub(1) as isize;
        doc.set_scroll_top(top.clamp(0, max) as usize);
    }

    // ------------------------------------------------------------ edit paths

    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        doc.insert_text(text);
        self.after_edit();
    }

    fn insert_newline(&mut self) {
        let auto = self.config.editor.auto_indent;
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if auto {
            doc.insert_newline();
        } else {
            doc.insert_text("\n");
        }
        self.after_edit();
    }

    fn insert_tab(&mut self) {
        let unit = self.config.indent_unit();
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if doc.has_selection() {
            editing::indent(doc, &unit);
        } else {
            doc.insert_text(&unit);
        }
        self.after_edit();
    }

    fn dedent(&mut self) {
        let width = self.config.tab_width();
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        editing::dedent(doc, width);
        self.after_edit();
    }

    fn backspace(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        doc.backspace();
        self.after_edit();
    }

    fn delete_forward(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        doc.delete_forward();
        self.after_edit();
    }

    fn move_cursor(&mut self, motion: Motion, extend: bool) {
        let page = self.last_editor_area.height.saturating_sub(1).max(1) as usize;
        let ctx = self.motion_ctx(page);
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        doc.move_cursor(motion, extend, ctx);
    }

    fn escape(&mut self) {
        if self.search.active {
            self.search.clear();
            self.info("search cleared");
            return;
        }
        if let Some(doc) = self.active_doc_mut() {
            if doc.has_selection() {
                doc.clear_selection();
                return;
            }
        }
        self.info("Nothing to cancel");
    }

    /// Keep derived state in sync after any document mutation.
    fn after_edit(&mut self) {
        let Some(idx) = self.active_index() else {
            return;
        };
        let doc = &self.buffers[idx];
        let id = doc.id();
        let revision = doc.revision();
        let cursor_line = doc.cursor_line_col().0;

        if let Some(cache) = self.syntax_cache.get_mut(&id) {
            cache.invalidate_from(cursor_line);
        }
        self.brackets_cache = None;

        if self.search.active && !self.search.query.is_empty() {
            let doc = &self.buffers[idx];
            self.search.refresh(doc);
            let _ = revision;
        }
    }

    // ------------------------------------------------------------- commands

    /// Execute a named command.
    pub fn execute(&mut self, cmd: Command) {
        debug!("command: {}", cmd.id());
        match cmd {
            Command::NewBuffer => self.new_buffer(),
            Command::Open => self.open_prompt(),
            Command::QuickOpen => self.open_palette(ui::PaletteMode::Files),
            Command::Save => self.save_active(false),
            Command::SaveAs => self.save_as_prompt(),
            Command::SaveAll => self.save_all(),
            Command::Reload => self.reload_active(),
            Command::CloseBuffer => self.close_active(),
            Command::NextBuffer => self.cycle_buffer(1),
            Command::PrevBuffer => self.cycle_buffer(-1),
            Command::Quit => self.request_quit(false),
            Command::QuitAll => self.request_quit(true),

            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Cut => self.cut(),
            Command::Copy => self.copy(),
            Command::Paste => self.paste(),
            Command::SelectAll => {
                if let Some(doc) = self.active_doc_mut() {
                    doc.select_all();
                }
            }
            Command::DuplicateLine => {
                self.with_doc(editing::duplicate_lines);
            }
            Command::DeleteLine => {
                self.with_doc(editing::delete_lines);
            }
            Command::MoveLineUp => {
                self.with_doc(|d| editing::move_lines(d, true));
            }
            Command::MoveLineDown => {
                self.with_doc(|d| editing::move_lines(d, false));
            }
            Command::ToggleComment => {
                self.with_doc(editing::toggle_comment);
            }
            Command::Indent => {
                let unit = self.config.indent_unit();
                self.with_doc(|d| editing::indent(d, &unit));
            }
            Command::Dedent => {
                let width = self.config.tab_width();
                self.with_doc(|d| editing::dedent(d, width));
            }
            Command::TrimTrailingWhitespace => {
                let changed = self.with_doc(editing::trim_trailing_whitespace);
                if changed {
                    self.success("Trailing whitespace removed");
                } else {
                    self.info("No trailing whitespace");
                }
            }

            Command::GotoLine => self.goto_prompt(),
            Command::GotoBufferStart => self.move_cursor(Motion::DocStart, false),
            Command::GotoBufferEnd => self.move_cursor(Motion::DocEnd, false),

            Command::Find => self.open_find(false),
            Command::Replace => self.open_find(true),
            Command::FindNext => self.find_next(),
            Command::FindPrev => self.find_prev(),
            Command::ReplaceOne => self.replace_one(),
            Command::ReplaceAll => self.replace_all(),
            Command::ClearSearch => {
                self.search.clear();
                self.info("Search cleared");
            }

            Command::ToggleTheme => self.toggle_theme(),
            Command::ToggleLineNumbers => {
                self.config.editor.line_numbers = !self.config.editor.line_numbers;
                self.info(format!(
                    "Line numbers {}",
                    on_off(self.config.editor.line_numbers)
                ));
            }
            Command::ToggleRelativeNumbers => {
                self.config.editor.relative_numbers = !self.config.editor.relative_numbers;
                self.info(format!(
                    "Relative numbers {}",
                    on_off(self.config.editor.relative_numbers)
                ));
            }
            Command::ToggleWhitespace => {
                self.config.editor.show_whitespace = !self.config.editor.show_whitespace;
                self.info(format!(
                    "Visible whitespace {}",
                    on_off(self.config.editor.show_whitespace)
                ));
            }
            Command::ToggleSyntax => {
                self.config.editor.syntax_highlighting = !self.config.editor.syntax_highlighting;
                self.info(format!(
                    "Syntax highlighting {}",
                    on_off(self.config.editor.syntax_highlighting)
                ));
            }
            Command::ToggleHints => {
                self.config.ui.show_hints = !self.config.ui.show_hints;
                self.info(format!(
                    "Shortcut hints {}",
                    on_off(self.config.ui.show_hints)
                ));
            }
            Command::ToggleCursorLine => {
                self.config.editor.highlight_cursor_line =
                    !self.config.editor.highlight_cursor_line;
                self.info(format!(
                    "Cursor line highlight {}",
                    on_off(self.config.editor.highlight_cursor_line)
                ));
            }
            Command::WordCount => self.word_count(),
            Command::ShowStats => self.show_stats(),
            Command::Help => self.open_help(),
            Command::CommandPalette => self.open_palette(ui::PaletteMode::Commands),
        }
    }

    /// Run a document operation, syncing derived state when it changed anything.
    fn with_doc<F>(&mut self, f: F) -> bool
    where
        F: FnOnce(&mut Document) -> bool,
    {
        let Some(doc) = self.active_doc_mut() else {
            return false;
        };
        let changed = f(doc);
        if changed {
            self.after_edit();
        }
        changed
    }

    fn undo(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if doc.undo() {
            self.after_edit();
        } else {
            self.info("Nothing to undo");
        }
    }

    fn redo(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if doc.redo() {
            self.after_edit();
        } else {
            self.info("Nothing to redo");
        }
    }

    fn copy(&mut self) {
        let Some(doc) = self.active_doc() else {
            return;
        };
        let text = if doc.has_selection() {
            doc.selected_text()
        } else {
            // Ctrl+C with no selection copies the current line, like VS Code.
            let line = doc.cursor_line_col().0;
            format!("{}\n", doc.line_text(line))
        };
        if text.is_empty() {
            self.info("Nothing to copy");
            return;
        }
        let outcome = self.clipboard.set(&text);
        let chars = text.chars().count();
        self.success(format!("Copied {chars} characters"));
        if outcome.is_fallback() {
            if let Some(notice) = self.clipboard.take_fallback_notice() {
                self.warn(notice);
            }
        }
    }

    fn cut(&mut self) {
        let Some(doc) = self.active_doc() else {
            return;
        };
        if !doc.has_selection() {
            // Cut the current line.
            let line = doc.cursor_line_col().0;
            let text = format!("{}\n", doc.line_text(line));
            self.clipboard.set(&text);
            self.with_doc(editing::delete_lines);
            self.success("Cut line");
            return;
        }
        let text = doc.selected_text();
        let chars = text.chars().count();
        self.clipboard.set(&text);
        self.with_doc(|d| {
            if let Some(sel) = d.selection() {
                d.delete_range(sel);
                true
            } else {
                false
            }
        });
        self.success(format!("Cut {chars} characters"));
    }

    fn paste(&mut self) {
        let Some(text) = self.clipboard.get() else {
            self.info("Clipboard is empty");
            return;
        };
        if text.is_empty() {
            return;
        }
        let chars = text.chars().count();
        self.insert_text(&text);
        self.success(format!("Pasted {chars} characters"));
    }

    fn word_count(&mut self) {
        let Some(doc) = self.active_doc() else {
            return;
        };
        let stats = doc.statistics();
        self.info(format!(
            "{} words, {} characters, {} lines",
            stats.words, stats.chars, stats.lines
        ));
    }

    fn show_stats(&mut self) {
        let Some(doc) = self.active_doc() else {
            return;
        };
        let stats = doc.statistics();
        let path = doc.display_path(Some(&self.cwd));
        self.info(format!(
            "{path} — {} lines, {} words, {} chars, {} bytes, {}, {}",
            stats.lines,
            stats.words,
            stats.chars,
            stats.bytes,
            doc.eol().label(),
            doc.encoding().label()
        ));
    }

    fn toggle_theme(&mut self) {
        self.config.ui.theme = if self.theme.dark {
            "vela-light".to_string()
        } else {
            "vela-dark".to_string()
        };
        self.theme = if self.theme.dark {
            Theme::light()
        } else {
            Theme::dark()
        };
        self.apply_theme_overrides();
        self.info(format!("Theme: {}", self.theme.name));
    }

    pub fn apply_theme_overrides(&mut self) {
        if let Some(base) = Theme::by_name(&self.config.ui.theme) {
            self.theme = base;
        }
        let unknown = self.theme.apply_overrides(&self.config.colors);
        for u in unknown {
            self.warn(format!("colors: {u}"));
        }
    }

    /// Editor area from the last frame, used to map mouse coordinates.
    pub fn set_editor_area(&mut self, area: Rect) {
        self.last_editor_area = area;
    }

    // ------------------------------------------------------------- brackets

    /// Bracket pair to highlight for the active buffer, cached per revision.
    pub fn brackets_for_active(&mut self) -> Option<(usize, usize)> {
        if !self.config.editor.match_brackets {
            return None;
        }
        let idx = self.active_index()?;
        let doc = &self.buffers[idx];
        let id = doc.id();
        let revision = doc.revision();
        let cursor = doc.cursor();
        if let Some((cached_id, cached_rev, cached_cursor, value)) = self.brackets_cache {
            if cached_id == id && cached_rev == revision && cached_cursor == cursor {
                return value;
            }
        }
        let value = editing::matching_brackets(doc.storage(), cursor);
        self.brackets_cache = Some((id, revision, cursor, value));
        value
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// Helper used by the paste path when a prompt is focused.
impl App {
    fn paste_into_prompt(&mut self, text: &str) {
        prompts::insert_into_focused_prompt(self, text);
    }
}

/// Extension so `Action::InsertText` can be consumed without matching twice.
trait IntoText {
    fn into_text(self) -> String;
}

impl IntoText for Action {
    fn into_text(self) -> String {
        match self {
            Action::InsertText(s) => s,
            _ => String::new(),
        }
    }
}

/// Re-exported for the UI layer.
pub use ui::{Overlay as AppOverlay, PaletteMode as AppPaletteMode};

// Keep the `Confirm` / `PathPrompt` imports meaningful for downstream modules.
#[allow(dead_code)]
fn _assert_types(_: &Confirm, _: &PathPrompt, _: &ConfirmAction, _: &Query, _: &Overlay) {}
