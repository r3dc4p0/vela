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

mod diff;
mod files;
mod panes;
mod prompts;
mod recovery;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use tracing::{debug, error as log_error, info};

use crate::buffer::{Caret, Document, Motion, MotionCtx};
use crate::clipboard::Clipboard;
use crate::commands::Command;
use crate::config::Config;
use crate::editing;
use crate::indent;
use crate::input::{self, Action};
use crate::keymap::Keymap;
use crate::platform;
use crate::search::{Query, SearchState};
use crate::syntax::StateCache;
use crate::theme::Theme;
use crate::ui::{self, Confirm, ConfirmAction, Level, Message, Overlay, PathPrompt, UiState};

pub use diff::DiffView;
pub use files::{Loading, OpenedBuffer, LOADER_DELAY};
pub use panes::{Pane, MIN_PANE_WIDTH};

/// Work handed to a background thread.
pub enum Job {
    /// A file finished loading, with its buffer already built.
    Opened {
        generation: u64,
        path: PathBuf,
        result: Box<Result<OpenedBuffer, crate::Error>>,
    },
    /// The file finder index finished building.
    Index(Vec<String>),
    /// A committed search finished scanning the whole document.
    Searched {
        generation: u64,
        revision: u64,
        result: Result<Vec<std::ops::Range<usize>>, crate::Error>,
    },
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
    /// The views onto those buffers. Always non-empty; `active_pane` indexes it.
    pub panes: Vec<Pane>,
    pub active_pane: usize,
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
    /// Where recents, the session and the recovery snapshot live.
    ///
    /// Empty in headless runs, which must never read or write the user's
    /// profile — that is what keeps the integration tests deterministic.
    pub data_dir: PathBuf,
    /// The generation the last recovery snapshot was written from.
    snapshot_generation: u64,
    /// When that snapshot was written, for the debounce.
    snapshot_at: Option<Instant>,

    tx: Sender<Job>,
    rx: Receiver<Job>,
    open_generation: u64,
    /// The open currently in flight, if any.
    ///
    /// Held as an `Arc` because the worker writes the byte counter into the
    /// same allocation the renderer reads it from — there is no second copy to
    /// keep in step, and no lock on the paint path.
    pub loading: Option<Arc<Loading>>,
    pub file_index: Option<Vec<String>>,
    pub index_loading: bool,
    /// Set while waiting for the user to confirm quitting.
    quit_pending: bool,
    brackets_cache: BracketCache,
    /// Last mouse press, for double- and triple-click selection.
    last_click: Option<(Instant, u16, u16)>,
    /// Clicks in the current run: 1 place, 2 word, 3 line.
    click_count: u8,
    /// Where a drag started, so the selection grows from the press point.
    drag_anchor: Option<usize>,
    /// Editor area from the last frame, for mouse coordinate mapping.
    last_editor_area: Rect,
    /// Tab strip from the last frame, so tabs can be clicked.
    last_tabs_area: Rect,
    /// Frames rendered so far (diagnostics).
    pub frame: u64,
    /// When false, recents and session state are neither read nor written.
    /// Integration tests use this so they never touch the real user profile.
    pub persist: bool,
    /// The comparison being shown, if any.
    ///
    /// A view mode rather than a buffer: it borrows two documents that are
    /// already open, so nothing here can go stale against its text — closing
    /// one of the two closes the view (see `close_active`).
    pub diff: Option<DiffView>,
    /// A comparison waiting for its file to finish loading.
    ///
    /// `(the buffer to compare with, the path being opened)`. Held rather than
    /// computed because a large file arrives from the worker thread later.
    pending_compare: Option<(usize, PathBuf)>,
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

        app.ensure_panes();
        // Whatever the last session left unsaved comes back before anything
        // else can overwrite it.
        app.recover_unsaved();

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
        app.data_dir = PathBuf::new();
        app.cwd = cwd;
        app.clipboard = Clipboard::internal_only();
        app.buffers.push(Document::untitled());
        app.ensure_panes();
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
            panes: Vec::new(),
            active_pane: 0,
            data_dir: platform::data_dir(),
            snapshot_generation: 0,
            snapshot_at: None,
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
            loading: None,
            file_index: None,
            index_loading: false,
            quit_pending: false,
            diff: None,
            pending_compare: None,
            brackets_cache: None,
            last_click: None,
            click_count: 0,
            drag_anchor: None,
            last_editor_area: Rect::default(),
            last_tabs_area: Rect::default(),
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
            Some(self.active())
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
            wrap_width: self.wrap_width(),
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
    /// Used by `vibe --smoke` to verify that the editor can start, negotiate
    /// the terminal and paint in a real environment (CI, a new terminal
    /// emulator, a packaging check) without needing a human.
    pub fn run_once(&mut self, terminal: &mut crate::terminal::VibeTerminal) -> crate::Result<()> {
        terminal
            .draw(|frame| ui::draw(frame, self))
            .map_err(|e| crate::Error::Message(format!("render failed: {e}")))?;
        self.frame += 1;
        Ok(())
    }

    /// Run until the user quits.
    pub fn run(&mut self, terminal: &mut crate::terminal::VibeTerminal) -> crate::Result<()> {
        info!("event loop started");
        while self.running {
            terminal
                .draw(|frame| ui::draw(frame, self))
                .map_err(|e| crate::Error::Message(format!("render failed: {e}")))?;
            self.frame += 1;

            self.poll_jobs();
            self.maybe_snapshot();

            // Wake up regularly for message expiry and background jobs, but do
            // not burn CPU when idle. While a file is opening the timeout also
            // paces the spinner: at 200 ms it would step four times a second,
            // which reads as a stutter rather than as motion.
            let timeout = if self.loading.is_some() {
                Duration::from_millis(80)
            } else if self.ui.messages.is_empty() {
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
        self.shutdown();
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
                    // The loader belongs to this generation, so it goes away
                    // whatever the outcome — an error is not a reason to keep
                    // spinning.
                    self.loading = None;
                    match *result {
                        Ok(opened) => {
                            let name = path.display().to_string();
                            self.install(path, opened);
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
                Job::Searched {
                    generation,
                    revision,
                    result,
                } => {
                    // Discard when the text or the query moved on while the
                    // scan ran: installing it would highlight stale positions.
                    let current = self.active_doc().map(|d| d.revision());
                    if current != Some(revision) {
                        debug!("discarding stale search result");
                        self.search.abandon(generation);
                        continue;
                    }
                    let cursor = self.active_doc().map(|d| d.cursor()).unwrap_or(0);
                    match result {
                        Ok(ranges) => {
                            let truncated = ranges.len() >= crate::search::FULL_MATCHES;
                            if !self.search.install(ranges, truncated, generation, cursor) {
                                debug!("discarding superseded search result");
                            }
                        }
                        // The live scan already validated the pattern, so this is
                        // only reachable if the regex engine failed twice. Leave
                        // the results on screen and stop advertising a scan.
                        Err(e) => {
                            debug!("committed search failed: {e}");
                            self.search.abandon(generation);
                        }
                    }
                }
            }
        }
    }

    /// Finish the session: persist what should persist, drop what should not.
    ///
    /// Public so a test — or a future front end — can end a session without
    /// going through the event loop.
    pub fn shutdown(&mut self) {
        self.save_recent();
        if self.config.editor.restore_session {
            self.save_session();
        }
        // A clean exit means the user was asked about the unsaved buffers and
        // answered. Keeping a copy would make "discard" a lie; what a crash
        // leaves behind is the case that never got to ask.
        self.clear_snapshot();
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

        // The diff view owns the scrolling keys while it is open. Everything
        // else still resolves through the keymap — quitting, the palette, the
        // theme — but an edit would change a document that is not on screen,
        // so text input is dropped rather than applied.
        let in_diff = self.diff.is_some();
        if in_diff && self.handle_diff_key(&key) {
            return;
        }

        let action = input::translate(&key, &self.keymap);
        if in_diff
            && matches!(
                action,
                Action::InsertText(_)
                    | Action::InsertNewline
                    | Action::InsertTab
                    | Action::Dedent
                    | Action::Backspace
                    | Action::DeleteForward
            )
        {
            return;
        }
        match action {
            Action::InsertText(text) => self.type_text(&text),
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
        use crossterm::event::{MouseButton, MouseEventKind};
        if !self.config.ui.mouse || self.ui.overlay.captures_input() {
            // Nothing here takes the pointer, so drop any highlight: a button
            // left lit under a pointer that has moved on is a lie.
            self.ui.hovered_hint = None;
            return;
        }
        // The hint bar is handled before the match, not as an arm of it: it is
        // chrome that sits below everything, and the diff view's catch-all arm
        // below would otherwise swallow the click before it was looked at.
        if matches!(mouse.kind, MouseEventKind::Moved) {
            self.hover_hint(mouse.column, mouse.row);
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.click_hint(mouse.column, mouse.row)
        {
            return;
        }

        match mouse.kind {
            // The diff view covers the whole editing area and has one scroll
            // offset, so the pointer's column is irrelevant there.
            MouseEventKind::ScrollUp if self.diff.is_some() => self.diff_scroll(-3),
            MouseEventKind::ScrollDown if self.diff.is_some() => self.diff_scroll(3),
            MouseEventKind::ScrollUp => self.wheel_vertical_at(mouse.column, mouse.row, -3),
            MouseEventKind::ScrollDown => self.wheel_vertical_at(mouse.column, mouse.row, 3),
            // Clicks and the horizontal wheel have nothing to act on: the
            // columns are read-only and do not scroll sideways.
            _ if self.diff.is_some() => {}
            MouseEventKind::ScrollLeft => self.wheel_horizontal_at(mouse.column, mouse.row, -4),
            MouseEventKind::ScrollRight => self.wheel_horizontal_at(mouse.column, mouse.row, 4),
            MouseEventKind::Down(MouseButton::Left) => {
                // The tab strip is chrome, so it is tested before the text.
                if self.tab_at(mouse.column, mouse.row).is_some() {
                    self.click_tab(mouse.column, mouse.row);
                    return;
                }
                // A click belongs to the pane it lands in, so the focus follows
                // the pointer before the coordinates are mapped.
                self.focus_pane_at(mouse.column, mouse.row);
                // Alt+Click extends the caret set instead of moving the cursor.
                if mouse.modifiers.contains(KeyModifiers::ALT) {
                    self.alt_click_at(mouse.column, mouse.row);
                    return;
                }
                self.click_at(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag_to(mouse.column, mouse.row),
            MouseEventKind::Up(MouseButton::Left) => self.drag_anchor = None,
            _ => {}
        }
    }

    /// Run the hint under a click, if there is one.
    ///
    /// Returns true when the click was a button press, so the caller does not
    /// go on to map the same point into a document — the bar is the bottom row
    /// of the screen, and a click there is never also a click on the text.
    fn click_hint(&mut self, column: u16, row: u16) -> bool {
        let Some(hit) = self
            .ui
            .hint_hits
            .iter()
            .find(|h| h.contains(column, row))
            .copied()
        else {
            return false;
        };
        self.execute(hit.command);
        true
    }

    /// Track which hint the pointer is over, so the bar can light it up.
    fn hover_hint(&mut self, column: u16, row: u16) {
        self.ui.hovered_hint = self
            .ui
            .hint_hits
            .iter()
            .position(|h| h.contains(column, row));
    }

    /// Columns the line-number gutter occupies, which the renderer and every
    /// screen-to-buffer mapping must agree on.
    fn gutter_width(&self, doc: &crate::buffer::Document) -> u16 {
        crate::ui::editor::gutter_width(doc.line_count(), &self.config)
    }

    /// Columns the editor gives to text: its width minus the gutter. Zero
    /// before the first frame, when the layout is not known yet.
    fn text_width(&self) -> usize {
        let Some(idx) = self.active_index() else {
            return 0;
        };
        let gutter = self.gutter_width(&self.buffers[idx]);
        (self.last_editor_area.width as usize).saturating_sub(gutter as usize)
    }

    /// Width to wrap at, or `None` when wrapping is off or the editor has not
    /// been laid out yet.
    ///
    /// Zero columns would wrap every line to a single character, so an unknown
    /// width has to mean "no wrapping" rather than "wrap at zero".
    pub fn wrap_width(&self) -> Option<usize> {
        if !self.config.editor.word_wrap {
            return None;
        }
        let width = self.text_width();
        if width == 0 {
            None
        } else {
            Some(width)
        }
    }

    /// Map a screen position inside the editor to a buffer character index.
    fn buffer_position_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.last_editor_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if row < area.y || row >= area.y + area.height {
            return None;
        }
        let doc = &self.buffers[self.active_index()?];
        let cfg = &self.config;
        let gutter = self.gutter_width(doc);
        if column < area.x + gutter {
            return None;
        }
        let rel_row = (row - area.y) as usize;
        let target = (column - area.x - gutter) as usize;

        // A wrapped row is one segment of a line, and rows no longer count
        // lines: ask the same layout the renderer used which row the pointer is
        // on before deciding which character it is.
        if let Some(width) = self.wrap_width() {
            let mut cache = crate::buffer::LayoutCache::new(Some(width), cfg.tab_width());
            let storage = doc.storage();
            let (line, seg) = cache.row_at(storage, doc.scroll_top(), doc.scroll_row(), rel_row)?;
            let offset = match cache.layout(storage, line) {
                Some(layout) => layout.pos_at(seg, target, cfg.tab_width()),
                // Too long to wrap, so it is painted as one row from a window
                // and the column is measured against the whole line.
                None => {
                    crate::buffer::motion::char_at_visual_col(
                        &storage.line_slice(line),
                        target,
                        cfg.tab_width(),
                    )
                    .0
                }
            };
            return Some(storage.line_start(line) + offset);
        }

        let line = doc.scroll_top() + rel_row;
        if line >= doc.line_count() {
            return None;
        }
        let target_col = target + doc.scroll_left();
        let line_start = doc.storage().line_start(line);
        let text = doc.line_text(line);
        let (offset, _) =
            crate::buffer::motion::char_at_visual_col(&text, target_col, cfg.tab_width());
        Some(line_start + offset)
    }

    /// Like [`buffer_position_at`](Self::buffer_position_at) but forgiving: a
    /// pointer outside the text — in the gutter, past the end of a line, or
    /// beyond the edges during a drag — maps to the nearest position instead of
    /// nothing.
    fn drag_position_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.last_editor_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let doc = &self.buffers[self.active_index()?];
        let cfg = &self.config;
        let row = row.clamp(area.y, area.y + area.height - 1);
        let gutter = self.gutter_width(doc);
        let target = if column < area.x + gutter {
            0
        } else {
            (column - area.x - gutter) as usize
        };

        if let Some(width) = self.wrap_width() {
            let mut cache = crate::buffer::LayoutCache::new(Some(width), cfg.tab_width());
            let storage = doc.storage();
            let rel_row = (row - area.y) as usize;
            let last = doc.line_count().saturating_sub(1);
            // Past the last row, keep extending along the final line.
            let (line, seg) = cache
                .row_at(storage, doc.scroll_top(), doc.scroll_row(), rel_row)
                .unwrap_or((last, 0));
            let offset = match cache.layout(storage, line) {
                Some(layout) => layout.pos_at(seg, target, cfg.tab_width()),
                None => {
                    crate::buffer::motion::char_at_visual_col(
                        &storage.line_slice(line),
                        target,
                        cfg.tab_width(),
                    )
                    .0
                }
            };
            return Some(storage.line_start(line) + offset);
        }

        let line =
            (doc.scroll_top() + (row - area.y) as usize).min(doc.line_count().saturating_sub(1));
        let target_col = if column < area.x + gutter {
            0
        } else {
            target + doc.scroll_left()
        };
        let line_start = doc.storage().line_start(line);
        let text = doc.line_text(line);
        let (offset, _) =
            crate::buffer::motion::char_at_visual_col(&text, target_col, cfg.tab_width());
        Some(line_start + offset)
    }

    /// Handle a left press on the text: place the cursor, or select a word on
    /// the second click and a line on the third.
    fn click_at(&mut self, column: u16, row: u16) {
        let now = Instant::now();
        let repeated = self
            .last_click
            .map(|(t, x, y)| {
                now.duration_since(t) < Duration::from_millis(400)
                    && x.abs_diff(column) <= 1
                    && y == row
            })
            .unwrap_or(false);
        // Cycle 1 -> 2 -> 3 -> 1 so a fourth click starts over.
        self.click_count = if repeated {
            self.click_count % 3 + 1
        } else {
            1
        };
        self.last_click = Some((now, column, row));

        let Some(pos) = self.buffer_position_at(column, row) else {
            self.drag_anchor = None;
            return;
        };
        let Some(idx) = self.active_index() else {
            return;
        };
        let doc = &mut self.buffers[idx];
        match self.click_count {
            2 => {
                doc.set_cursor(pos);
                editing::select_word_at_cursor(doc);
            }
            3 => {
                doc.set_cursor(pos);
                editing::select_line_at_cursor(doc);
            }
            _ => doc.set_cursor(pos),
        }
        // A drag that follows this press selects from where it landed.
        self.drag_anchor = Some(pos);
    }

    /// Extend the selection to the pointer, scrolling when it leaves the text.
    fn drag_to(&mut self, column: u16, row: u16) {
        let area = self.last_editor_area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let Some(anchor) = self.drag_anchor else {
            return;
        };
        // Autoscroll one line per event while the pointer is above or below the
        // text, so a drag past the edge keeps extending instead of stopping.
        if row < area.y {
            self.scroll_by(-1);
        } else if row >= area.y + area.height {
            self.scroll_by(1);
        }
        let Some(pos) = self.drag_position_at(column, row) else {
            return;
        };
        let Some(idx) = self.active_index() else {
            return;
        };
        self.buffers[idx].set_selection(pos, anchor);
    }

    /// The buffer whose tab covers `(column, row)`, if any.
    fn tab_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.last_tabs_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if row < area.y || row >= area.y + area.height {
            return None;
        }
        crate::ui::chrome::tab_spans(self, area)
            .into_iter()
            .find(|span| column >= span.x && column < span.x + span.width)
            .map(|span| span.buffer)
    }

    fn click_tab(&mut self, column: u16, row: u16) {
        if let Some(idx) = self.tab_at(column, row) {
            self.focus_buffer(idx);
        }
    }

    /// Alt+Click: add a caret at the clicked position, or remove the secondary
    /// caret already sitting there.
    fn alt_click_at(&mut self, column: u16, row: u16) {
        let Some(pos) = self.buffer_position_at(column, row) else {
            return;
        };
        let Some(idx) = self.active_index() else {
            return;
        };
        let doc = &mut self.buffers[idx];
        // Clicking the primary caret must not delete the only caret.
        if doc.cursor() == pos {
            return;
        }
        if doc.remove_caret_at(pos) {
            return;
        }
        if !doc.add_caret(Caret::at(pos)) {
            self.info("A caret already covers that position");
            return;
        }
        let count = doc.caret_count();
        self.info(format!("{count} cursors"));
    }

    /// Move the viewport without moving the cursor.
    ///
    /// A wheel scroll is a look-around gesture, not a motion: it may travel the
    /// whole buffer and leaves the caret where it was, even off screen. The
    /// view stops following the cursor until the next cursor movement, which
    /// is what lets it go further than one page away from the caret.
    ///
    /// The step is in *visual rows*, so one notch of the wheel moves the text
    /// by the same number of lines on screen whether or not lines are wrapped.
    fn scroll_by(&mut self, delta: isize) {
        let Some(idx) = self.active_index() else {
            return;
        };
        let wrap = self.wrap_width();
        self.scroll_vertical(idx, wrap, delta);
    }

    /// Vertical scroll of one document, at a given wrap width.
    ///
    /// Split from [`scroll_by`](Self::scroll_by) so the wheel can move a pane
    /// that does not have the focus, at *that* pane's width.
    fn scroll_vertical(&mut self, idx: usize, wrap: Option<usize>, delta: isize) {
        let tab_width = self.config.tab_width();
        let rows = self.buffers[idx].viewport_rows().max(1);
        let doc = &mut self.buffers[idx];

        let mut cache = crate::buffer::LayoutCache::new(wrap, tab_width);
        let (top, sub) = cache.shift(doc.storage(), doc.scroll_top(), doc.scroll_row(), delta);
        // Stop at the last full page rather than at the last row, so the buffer
        // never scrolls up into a screen of blank rows. The last page starts at
        // the *last row* of the last line, which is not the same thing when one
        // wrapped line is taller than the window.
        let last = doc.line_count().saturating_sub(1);
        let last_row = cache.height(doc.storage(), last).saturating_sub(1);
        let (max_top, max_sub) = cache.shift(doc.storage(), last, last_row, -(rows as isize - 1));
        let (top, sub) = if (top, sub) > (max_top, max_sub) {
            (max_top, max_sub)
        } else {
            (top, sub)
        };
        doc.set_view_top(top, sub);
        doc.detach_view();
    }

    /// Horizontal wheel scrolling, for lines wider than the window.
    ///
    /// The view stops once the widest line on screen ends: past that point
    /// every visible cell is blank, and scrolling into it would feel like an
    /// endless canvas. With word wrap on there is nothing to scroll to — every
    /// row already fits — so the gesture is ignored rather than allowed to
    /// shift a view that has no horizontal offset.
    fn scroll_sideways(&mut self, delta: isize) {
        if self.config.editor.word_wrap {
            return;
        }
        let Some(idx) = self.active_index() else {
            return;
        };
        let text_width = self.text_width();
        self.scroll_horizontal(idx, text_width, delta);
    }

    /// Horizontal scroll of one document, in a text area `text_width` wide.
    fn scroll_horizontal(&mut self, idx: usize, text_width: usize, delta: isize) {
        let tab_width = self.config.tab_width();
        let doc = &mut self.buffers[idx];
        let rows = doc.viewport_rows();
        let limit = doc
            .visible_width(tab_width, rows)
            .saturating_sub(text_width) as isize;
        let left = doc.scroll_left() as isize + delta;
        doc.set_scroll_left(left.clamp(0, limit) as usize);
        doc.detach_view();
    }

    // ------------------------------------------------------------ edit paths

    /// Insert text at the cursor, as-is.
    ///
    /// This is the raw path: paste, recovery and tests use it, and a pasted
    /// `(` must stay one character. Typing goes through [`App::type_text`],
    /// which is where the auto-pair rules live.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        editing::insert_at_carets(doc, text);
        self.after_edit();
    }

    /// Insert one typed keystroke, letting the auto-pair rules see it first.
    fn type_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut chars = text.chars();
        let single = chars.next().filter(|_| chars.next().is_none());
        let auto_pairs = self.config.editor.auto_pairs;
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if auto_pairs {
            if let Some(typed) = single {
                if editing::type_char_at_carets(doc, typed) {
                    self.after_edit();
                    return;
                }
            }
        }
        editing::insert_at_carets(doc, text);
        self.after_edit();
    }

    fn insert_newline(&mut self) {
        let auto_indent = self.config.editor.auto_indent;
        // Opening a pair onto three lines is auto-indent applied to a pair, so
        // it needs both settings: turning off auto-indent must not leave the
        // editor inserting indentation through the other switch.
        let expand = self.config.editor.auto_pairs && auto_indent;
        let unit = self.indent_unit();
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        editing::newline_at_carets(doc, auto_indent, expand.then_some(unit.as_str()));
        self.after_edit();
    }

    /// What the loader needs to know to read a file's indentation.
    fn indent_options(&self) -> indent::Options {
        indent::Options {
            detect: self.config.editor.detect_indent,
            fallback_width: self.config.tab_width(),
        }
    }

    /// The indentation the active buffer is written with.
    ///
    /// A detected style wins over the configuration: the file in front of you
    /// is better evidence than a global setting. The setting is what new
    /// buffers, and files whose text says nothing, fall back to.
    pub fn indent_style(&self) -> indent::IndentStyle {
        self.active_doc()
            .and_then(|doc| doc.indent_style())
            .unwrap_or_else(|| indent::IndentStyle {
                use_spaces: self.config.editor.use_spaces,
                width: self.config.tab_width(),
            })
    }

    /// The text one level of indentation inserts in the active buffer.
    fn indent_unit(&self) -> String {
        self.indent_style().unit()
    }

    /// Columns one level of indentation removes from the active buffer.
    fn indent_width(&self) -> usize {
        self.indent_style().width
    }

    fn insert_tab(&mut self) {
        let unit = self.indent_unit();
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if doc.has_multiple_carets() {
            // Every caret gets its own indent unit; a selection on any caret is
            // replaced like any other insertion.
            editing::insert_at_carets(doc, &unit);
        } else if doc.has_selection() {
            editing::indent(doc, &unit);
        } else {
            doc.insert_text(&unit);
        }
        self.after_edit();
    }

    fn dedent(&mut self) {
        let width = self.indent_width();
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        editing::dedent(doc, width);
        self.after_edit();
    }

    fn backspace(&mut self) {
        let delete_pairs = self.config.editor.auto_pairs;
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        editing::backspace_at_carets(doc, delete_pairs);
        self.after_edit();
    }

    fn delete_forward(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        editing::delete_forward_at_carets(doc);
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
        if let Some(doc) = self.active_doc_mut() {
            if doc.has_multiple_carets() {
                doc.collapse_carets();
                return;
            }
        }
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
        // A multi-cursor edit touches several lines; invalidate from the
        // highest one so no stale lexer state survives.
        let first_line = doc
            .carets()
            .iter()
            .map(|c| doc.storage().line_of_char(c.span().start))
            .min()
            .unwrap_or(0);

        if let Some(cache) = self.syntax_cache.get_mut(&id) {
            cache.invalidate_from(first_line);
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
            Command::CompareWith => self.compare_prompt(),
            Command::CloseDiff => self.close_diff(),
            Command::DiffNextChange => self.diff_step_change(true),
            Command::DiffPrevChange => self.diff_step_change(false),
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
                let unit = self.indent_unit();
                self.with_doc(|d| editing::indent(d, &unit));
            }
            Command::Dedent => {
                let width = self.indent_width();
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
            Command::AddCursorAbove => self.add_cursor_vertical(true),
            Command::AddCursorBelow => self.add_cursor_vertical(false),
            Command::AddCursorToNextOccurrence => self.add_cursor_next_occurrence(),
            Command::CollapseCursors => self.collapse_cursors(),

            Command::GotoLine => self.goto_prompt(),
            Command::SetIndentation => self.indent_prompt(),
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

            Command::SplitPane => {
                self.split_pane();
            }
            Command::ClosePane => {
                self.close_pane();
            }
            Command::FocusPaneLeft => self.focus_pane_step(-1),
            Command::FocusPaneRight => self.focus_pane_step(1),
            Command::ToggleTheme => self.toggle_theme(),
            Command::ToggleUiStyle => {
                let next = self.config.ui_style().other();
                self.config.ui.style = next.name().to_string();
                self.info(format!("UI style: {}", next.name()));
            }
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
            Command::ToggleIndentGuides => {
                self.config.editor.show_indent_guides = !self.config.editor.show_indent_guides;
                self.info(format!(
                    "Indent guides {}",
                    on_off(self.config.editor.show_indent_guides)
                ));
            }
            Command::ToggleAutoPairs => {
                self.config.editor.auto_pairs = !self.config.editor.auto_pairs;
                self.info(format!(
                    "Auto-close pairs {}",
                    on_off(self.config.editor.auto_pairs)
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
            Command::ToggleWordWrap => self.toggle_word_wrap(),
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

    // ---------------------------------------------------------- multi-cursor

    /// Add a caret on the line above (`up`) or below the outermost caret.
    fn add_cursor_vertical(&mut self, up: bool) {
        let tab_width = self.config.tab_width();
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        let added = editing::add_cursor_vertical(doc, !up, tab_width);
        let count = doc.caret_count();
        if added {
            self.info(format!("{count} cursors"));
        } else {
            self.info(if up {
                "No line above to add a cursor on"
            } else {
                "No line below to add a cursor on"
            });
        }
    }

    fn add_cursor_next_occurrence(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        let outcome = editing::add_cursor_to_next_occurrence(doc);
        match outcome {
            editing::Occurrence::Added(n) => self.success(format!("{n} cursors")),
            editing::Occurrence::Exhausted => self.info("No further occurrence"),
            editing::Occurrence::NoWord => {
                self.info("Put the cursor on a word, or select the text to match")
            }
            editing::Occurrence::Limit => self.warn(format!(
                "Cursor limit reached ({}); press Esc to collapse",
                editing::MAX_CARETS
            )),
        }
    }

    fn collapse_cursors(&mut self) {
        let Some(doc) = self.active_doc_mut() else {
            return;
        };
        if doc.has_multiple_carets() {
            doc.collapse_carets();
            self.info("Collapsed to one cursor");
        } else {
            self.info("Already a single cursor");
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
        let text = editing::clipboard_text(doc);
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
        let multiple = doc.has_multiple_carets();
        if !multiple && !doc.has_selection() {
            // Cut the current line.
            let line = doc.cursor_line_col().0;
            let text = format!("{}\n", doc.line_text(line));
            self.clipboard.set(&text);
            self.with_doc(editing::delete_lines);
            self.success("Cut line");
            return;
        }
        let text = editing::clipboard_text(doc);
        if text.is_empty() {
            self.info("Nothing to cut");
            return;
        }
        let chars = text.chars().count();
        self.clipboard.set(&text);
        let changed = self.with_doc(editing::cut_at_carets);
        if changed {
            self.success(format!("Cut {chars} characters"));
        }
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

    /// Turn word wrap on or off.
    ///
    /// The wrap width is a property of the window rather than of the buffer, so
    /// this is a session setting like line numbers, not a per-buffer mode.
    /// Toggling it invalidates both halves of the viewport: the horizontal
    /// offset has no meaning in a wrapped view, and the sub-row offset points
    /// into a row that the other mode may not have.
    fn toggle_word_wrap(&mut self) {
        self.config.editor.word_wrap = !self.config.editor.word_wrap;
        let on = self.config.editor.word_wrap;
        if let Some(doc) = self.active_doc_mut() {
            if on {
                doc.set_scroll_left(0);
            }
            doc.set_scroll_row(0);
            // Let the view find the caret again: it was scrolled to a row that
            // just changed identity.
            doc.attach_view();
        }
        self.info(format!("Word wrap {}", on_off(on)));
    }

    fn toggle_theme(&mut self) {
        self.config.ui.theme = if self.theme.dark {
            "vibe-light".to_string()
        } else {
            "vibe-dark".to_string()
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

    /// Tab strip from the last frame, used to map mouse coordinates.
    pub fn set_tabs_area(&mut self, area: Rect) {
        self.last_tabs_area = area;
    }

    /// Editor area as of the last frame.
    pub fn editor_area(&self) -> Rect {
        self.last_editor_area
    }

    /// Tab strip as of the last frame.
    pub fn tabs_area(&self) -> Rect {
        self.last_tabs_area
    }

    // ------------------------------------------------------------- brackets

    /// Bracket pair to highlight for the active buffer, cached per revision.
    pub fn brackets_for_active(&mut self) -> Option<(usize, usize)> {
        let idx = self.active_index()?;
        self.brackets_for(idx)
    }

    /// Bracket pair to highlight for the buffer at `idx`, cached per revision.
    ///
    /// Each pane asks for its own buffer, so the cache is keyed by document id
    /// rather than by "the active one".
    pub fn brackets_for(&mut self, idx: usize) -> Option<(usize, usize)> {
        if !self.config.editor.match_brackets {
            return None;
        }
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
