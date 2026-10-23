//! Rendering and UI state.
//!
//! The UI owns presentation state (which overlay is open, prompt contents,
//! transient messages) and reads application state. It never mutates a document
//! except through the documented `scroll` fields of the active buffer, which are
//! pure view state.

pub mod chrome;
pub mod editor;
pub mod input;
pub mod loading;
pub mod overlays;

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::Frame;

use crate::app::App;
use crate::commands::Command;
use crate::theme::Theme;

pub use chrome::OverlayKind;
pub use input::TextInput;

/// Severity of a transient message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warn,
    Error,
}

/// A transient status message.
#[derive(Debug, Clone)]
pub struct Message {
    pub text: String,
    pub level: Level,
    pub born: Instant,
}

impl Message {
    pub fn new(text: impl Into<String>, level: Level) -> Self {
        Self {
            text: text.into(),
            level,
            born: Instant::now(),
        }
    }
}

/// Which picker a [`Palette`] is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,
    Files,
}

/// What selecting a palette entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    Run(Command),
    OpenFile(std::path::PathBuf),
}

/// One row in the palette.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub label: String,
    pub detail: String,
    pub positions: Vec<usize>,
    pub action: PaletteAction,
}

/// Command palette / file finder state.
#[derive(Debug, Clone)]
pub struct Palette {
    pub mode: PaletteMode,
    pub input: TextInput,
    pub items: Vec<PaletteItem>,
    pub selected: usize,
    pub scroll: usize,
}

impl Palette {
    pub fn new(mode: PaletteMode) -> Self {
        Self {
            mode,
            input: TextInput::new(),
            items: Vec::new(),
            selected: 0,
            scroll: 0,
        }
    }
}

/// Incremental find bar.
#[derive(Debug, Clone)]
pub struct FindBar {
    pub input: TextInput,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl FindBar {
    pub fn new() -> Self {
        Self {
            input: TextInput::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
        }
    }
}

impl Default for FindBar {
    fn default() -> Self {
        Self::new()
    }
}

/// Find and replace bar.
#[derive(Debug, Clone)]
pub struct ReplaceBar {
    pub input: TextInput,
    pub replace: TextInput,
    pub focus_replace: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl ReplaceBar {
    pub fn new() -> Self {
        Self {
            input: TextInput::new(),
            replace: TextInput::new(),
            focus_replace: false,
            case_sensitive: false,
            whole_word: false,
            regex: false,
        }
    }
}

impl Default for ReplaceBar {
    fn default() -> Self {
        Self::new()
    }
}

/// Path prompt with filesystem completion.
#[derive(Debug, Clone)]
pub struct PathPrompt {
    pub input: TextInput,
    pub completion: Vec<String>,
    pub selected: usize,
}

impl PathPrompt {
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            input: TextInput::with(initial),
            completion: Vec::new(),
            selected: 0,
        }
    }
}

/// What confirming a dialog will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    Quit,
    CloseBuffer(usize),
    Reload,
    OverwriteExternal,
}

/// A modal yes/no/… dialog.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub prompt: String,
    pub items: Vec<String>,
    pub selected: usize,
    pub action: ConfirmAction,
}

/// Help overlay scroll state.
#[derive(Debug, Clone, Default)]
pub struct HelpState {
    pub scroll: usize,
}

/// The single overlay slot: at most one modal UI is open at a time.
#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Palette(Palette),
    Find(FindBar),
    Replace(ReplaceBar),
    GotoLine(TextInput),
    Open(PathPrompt),
    SaveAs(PathPrompt),
    Confirm(Confirm),
    Help(HelpState),
}

impl Overlay {
    pub fn kind(&self) -> OverlayKind {
        match self {
            Overlay::None => OverlayKind::None,
            Overlay::Palette(_) => OverlayKind::Palette,
            Overlay::Find(_) => OverlayKind::Find,
            Overlay::Replace(_) => OverlayKind::Replace,
            Overlay::GotoLine(_) => OverlayKind::GotoLine,
            Overlay::Open(_) => OverlayKind::Open,
            Overlay::SaveAs(_) => OverlayKind::SaveAs,
            Overlay::Confirm(_) => OverlayKind::Confirm,
            Overlay::Help(_) => OverlayKind::Help,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Overlay::None)
    }

    /// True when the editor must not receive text input.
    pub fn captures_input(&self) -> bool {
        !self.is_none()
    }
}

/// All presentation state.
#[derive(Debug)]
pub struct UiState {
    pub overlay: Overlay,
    pub messages: Vec<Message>,
    /// Last message shown in the status bar (kept after the toast expires).
    pub status: Option<Message>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            overlay: Overlay::None,
            messages: Vec::new(),
            status: None,
        }
    }
}

impl UiState {
    /// Messages that are still within their display window.
    pub fn visible_messages(&self) -> Vec<&Message> {
        let now = Instant::now();
        self.messages
            .iter()
            .filter(|m| now.duration_since(m.born).as_millis() < 4_000)
            .collect()
    }

    /// Drop expired messages and cap the queue.
    pub fn prune(&mut self) {
        let now = Instant::now();
        self.messages
            .retain(|m| now.duration_since(m.born).as_millis() < 8_000);
        while self.messages.len() > 16 {
            self.messages.remove(0);
        }
    }
}

/// Compute the row rectangles for the whole window.
struct Layout {
    topbar: Option<Rect>,
    tabs: Option<Rect>,
    editor: Rect,
    status: Option<Rect>,
    hints: Option<Rect>,
}

fn layout(area: Rect, app: &App) -> Layout {
    let want_top = app.config.ui.show_topbar;
    let want_tabs = app.config.ui.show_tabs && app.buffers.len() > 1;
    let want_hints = app.config.ui.show_hints;

    // Fixed priority: the editing area always keeps at least one row, then the
    // status bar, then the top bar, then tabs, then hints. Every step is only
    // taken when a spare row remains, so the sum can never exceed the height
    // (which would draw outside the buffer and panic).
    let mut budget = area.height as usize;
    let mut status = 0usize;
    let mut top = 0usize;
    let mut tabs = 0usize;
    let mut hints = 0usize;
    if budget >= 2 {
        status = 1;
        budget -= 1;
    }
    if want_top && budget >= 2 {
        top = 1;
        budget -= 1;
    }
    if want_tabs && budget >= 2 {
        tabs = 1;
        budget -= 1;
    }
    if want_hints && budget >= 2 {
        hints = 1;
    }

    let editor_h = area
        .height
        .saturating_sub((top + tabs + hints + status) as u16);

    let mut y = area.y;
    let topbar = if top > 0 {
        let r = Rect::new(area.x, y, area.width, 1);
        y += 1;
        Some(r)
    } else {
        None
    };
    let tabs_rect = if tabs > 0 {
        let r = Rect::new(area.x, y, area.width, 1);
        y += 1;
        Some(r)
    } else {
        None
    };
    let editor = Rect::new(area.x, y, area.width, editor_h);
    y += editor_h;
    let status_rect = if status > 0 {
        let r = Rect::new(area.x, y, area.width, 1);
        y += 1;
        Some(r)
    } else {
        None
    };
    let hints_rect = if hints > 0 {
        Some(Rect::new(area.x, y, area.width, 1))
    } else {
        None
    };

    Layout {
        topbar,
        tabs: tabs_rect,
        editor,
        status: status_rect,
        hints: hints_rect,
    }
}

/// Render one frame.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme.clone();
    frame
        .buffer_mut()
        .set_style(area, Style::default().bg(theme.bg).fg(theme.fg));

    let l = layout(area, app);
    // Mouse hit-testing and the page size both need the rect the editor was
    // actually given; only the draw pass knows it. Same for the tab strip,
    // which is clickable.
    app.set_editor_area(l.editor);
    app.set_tabs_area(l.tabs.unwrap_or_default());

    if let Some(r) = l.topbar {
        chrome::draw_topbar(frame.buffer_mut(), r, app);
    }
    if let Some(r) = l.tabs {
        chrome::draw_tabs(frame.buffer_mut(), r, app);
    }

    draw_editor(frame, l.editor, app);

    if let Some(r) = l.status {
        chrome::draw_status(frame.buffer_mut(), r, app);
    }
    if let Some(r) = l.hints {
        chrome::draw_hints(frame.buffer_mut(), r, app);
    }

    // Notifications float over the bottom of the editing area, never over the
    // status bar or the hint bar: those carry information the user is reading.
    chrome::draw_messages(frame.buffer_mut(), l.editor, app);

    // A file still being opened gets the middle of the editing area, under any
    // overlay the user has opened since.
    loading::draw(frame.buffer_mut(), l.editor, app);

    if let Some((x, y)) = overlays::draw(frame, app) {
        frame.set_cursor_position((x, y));
    }
}

fn draw_editor(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(idx) = app.active_index() else {
        return;
    };
    let brackets = app.brackets_for_active();
    let doc_id = app.buffers[idx].id();

    let cache = app.syntax_cache.entry(doc_id).or_default();

    let mut renderer = editor::EditorRenderer {
        doc: &mut app.buffers[idx],
        theme: &app.theme,
        cfg: &app.config,
        search: &app.search,
        brackets,
        cache,
        focused: app.ui.overlay.is_none(),
    };
    renderer.render(frame, area);
}

/// Convenience for tests: render an app into a test backend and return the
/// buffer as plain text rows.
///
/// Exposed (rather than `#[cfg(test)]`) so the integration tests in `tests/`
/// can assert on what the UI actually paints.
#[doc(hidden)]
pub fn render_to_lines(app: &mut App, width: u16, height: u16) -> Vec<String> {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// The theme used by the UI, exposed so tests can assert on colours.
pub fn theme_of(app: &App) -> &Theme {
    &app.theme
}
