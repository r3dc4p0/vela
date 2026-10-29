//! The command registry.
//!
//! Every user-visible action is a [`Command`] variant with a stable string id,
//! a title, a category and default key bindings. Shortcuts, the command palette
//! and (future) menus all resolve through this table, so a command only ever
//! needs to be added in one place to become discoverable and bindable.
//!
//! Adding a command is a three-step change, documented in
//! `docs/extending.md`:
//!
//! 1. add a variant to [`Command`] and to [`Command::ALL`];
//! 2. fill in `id`, `title`, `category` and `default_keys`;
//! 3. handle it in `App::execute`.

/// Grouping used by the palette and the help screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    File,
    Edit,
    Navigate,
    Search,
    View,
    Help,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::File => "File",
            Category::Edit => "Edit",
            Category::Navigate => "Navigate",
            Category::Search => "Search",
            Category::View => "View",
            Category::Help => "Help",
        }
    }
}

/// A named, bindable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    // File
    NewBuffer,
    Open,
    QuickOpen,
    Save,
    SaveAs,
    SaveAll,
    Reload,
    CloseBuffer,
    NextBuffer,
    PrevBuffer,
    Quit,
    QuitAll,

    // Edit
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    DuplicateLine,
    DeleteLine,
    MoveLineUp,
    MoveLineDown,
    ToggleComment,
    Indent,
    Dedent,
    TrimTrailingWhitespace,
    AddCursorAbove,
    AddCursorBelow,
    AddCursorToNextOccurrence,
    CollapseCursors,

    // Navigate
    GotoLine,
    GotoBufferStart,
    GotoBufferEnd,

    // Search
    Find,
    Replace,
    FindNext,
    FindPrev,
    ReplaceOne,
    ReplaceAll,
    ClearSearch,

    // View
    SplitPane,
    ClosePane,
    FocusPaneLeft,
    FocusPaneRight,
    ToggleTheme,
    ToggleUiStyle,
    ToggleAutoPairs,
    ToggleLineNumbers,
    ToggleRelativeNumbers,
    ToggleWhitespace,
    ToggleIndentGuides,
    ToggleSyntax,
    ToggleHints,
    ToggleCursorLine,
    ToggleWordWrap,
    WordCount,
    ShowStats,

    // Help
    Help,
    CommandPalette,
}

impl Command {
    /// Every command, in palette order.
    pub const ALL: &'static [Command] = &[
        Command::NewBuffer,
        Command::Open,
        Command::QuickOpen,
        Command::Save,
        Command::SaveAs,
        Command::SaveAll,
        Command::Reload,
        Command::CloseBuffer,
        Command::NextBuffer,
        Command::PrevBuffer,
        Command::Quit,
        Command::QuitAll,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Copy,
        Command::Paste,
        Command::SelectAll,
        Command::DuplicateLine,
        Command::DeleteLine,
        Command::MoveLineUp,
        Command::MoveLineDown,
        Command::ToggleComment,
        Command::Indent,
        Command::Dedent,
        Command::TrimTrailingWhitespace,
        Command::AddCursorAbove,
        Command::AddCursorBelow,
        Command::AddCursorToNextOccurrence,
        Command::CollapseCursors,
        Command::GotoLine,
        Command::GotoBufferStart,
        Command::GotoBufferEnd,
        Command::Find,
        Command::Replace,
        Command::FindNext,
        Command::FindPrev,
        Command::ReplaceOne,
        Command::ReplaceAll,
        Command::ClearSearch,
        Command::SplitPane,
        Command::ClosePane,
        Command::FocusPaneLeft,
        Command::FocusPaneRight,
        Command::ToggleTheme,
        Command::ToggleUiStyle,
        Command::ToggleAutoPairs,
        Command::ToggleLineNumbers,
        Command::ToggleRelativeNumbers,
        Command::ToggleWhitespace,
        Command::ToggleIndentGuides,
        Command::ToggleSyntax,
        Command::ToggleHints,
        Command::ToggleCursorLine,
        Command::ToggleWordWrap,
        Command::WordCount,
        Command::ShowStats,
        Command::Help,
        Command::CommandPalette,
    ];

    /// Stable identifier used in configuration files.
    pub fn id(self) -> &'static str {
        match self {
            Command::NewBuffer => "file.new",
            Command::Open => "file.open",
            Command::QuickOpen => "file.quickOpen",
            Command::Save => "file.save",
            Command::SaveAs => "file.saveAs",
            Command::SaveAll => "file.saveAll",
            Command::Reload => "file.reload",
            Command::CloseBuffer => "file.close",
            Command::NextBuffer => "buffer.next",
            Command::PrevBuffer => "buffer.prev",
            Command::Quit => "app.quit",
            Command::QuitAll => "app.quitAll",
            Command::Undo => "edit.undo",
            Command::Redo => "edit.redo",
            Command::Cut => "edit.cut",
            Command::Copy => "edit.copy",
            Command::Paste => "edit.paste",
            Command::SelectAll => "edit.selectAll",
            Command::DuplicateLine => "edit.duplicateLine",
            Command::DeleteLine => "edit.deleteLine",
            Command::MoveLineUp => "edit.moveLineUp",
            Command::MoveLineDown => "edit.moveLineDown",
            Command::ToggleComment => "edit.toggleComment",
            Command::Indent => "edit.indent",
            Command::Dedent => "edit.dedent",
            Command::TrimTrailingWhitespace => "edit.trimTrailingWhitespace",
            Command::AddCursorAbove => "edit.addCursorAbove",
            Command::AddCursorBelow => "edit.addCursorBelow",
            Command::AddCursorToNextOccurrence => "edit.addCursorToNext",
            Command::CollapseCursors => "edit.collapseCursors",
            Command::GotoLine => "nav.gotoLine",
            Command::GotoBufferStart => "nav.bufferStart",
            Command::GotoBufferEnd => "nav.bufferEnd",
            Command::Find => "search.find",
            Command::Replace => "search.replace",
            Command::FindNext => "search.findNext",
            Command::FindPrev => "search.findPrev",
            Command::ReplaceOne => "search.replaceOne",
            Command::ReplaceAll => "search.replaceAll",
            Command::ClearSearch => "search.clear",
            Command::SplitPane => "view.splitPane",
            Command::ClosePane => "view.closePane",
            Command::FocusPaneLeft => "view.focusPaneLeft",
            Command::FocusPaneRight => "view.focusPaneRight",
            Command::ToggleTheme => "view.toggleTheme",
            Command::ToggleUiStyle => "view.toggleUiStyle",
            Command::ToggleAutoPairs => "edit.toggleAutoPairs",
            Command::ToggleLineNumbers => "view.toggleLineNumbers",
            Command::ToggleRelativeNumbers => "view.toggleRelativeNumbers",
            Command::ToggleWhitespace => "view.toggleWhitespace",
            Command::ToggleIndentGuides => "view.toggleIndentGuides",
            Command::ToggleSyntax => "view.toggleSyntax",
            Command::ToggleHints => "view.toggleHints",
            Command::ToggleCursorLine => "view.toggleCursorLine",
            Command::ToggleWordWrap => "view.toggleWordWrap",
            Command::WordCount => "view.wordCount",
            Command::ShowStats => "view.stats",
            Command::Help => "help.show",
            Command::CommandPalette => "help.commandPalette",
        }
    }

    /// Title shown in the palette and help screen.
    pub fn title(self) -> &'static str {
        match self {
            Command::NewBuffer => "New Buffer",
            Command::Open => "Open File…",
            Command::QuickOpen => "Quick Open File…",
            Command::Save => "Save",
            Command::SaveAs => "Save As…",
            Command::SaveAll => "Save All",
            Command::Reload => "Revert File from Disk",
            Command::CloseBuffer => "Close Buffer",
            Command::NextBuffer => "Next Buffer",
            Command::PrevBuffer => "Previous Buffer",
            Command::Quit => "Quit",
            Command::QuitAll => "Quit All",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Cut => "Cut",
            Command::Copy => "Copy",
            Command::Paste => "Paste",
            Command::SelectAll => "Select All",
            Command::DuplicateLine => "Duplicate Line",
            Command::DeleteLine => "Delete Line",
            Command::MoveLineUp => "Move Line Up",
            Command::MoveLineDown => "Move Line Down",
            Command::ToggleComment => "Toggle Comment",
            Command::Indent => "Indent",
            Command::Dedent => "Outdent",
            Command::TrimTrailingWhitespace => "Trim Trailing Whitespace",
            Command::AddCursorAbove => "Add Cursor Above",
            Command::AddCursorBelow => "Add Cursor Below",
            Command::AddCursorToNextOccurrence => "Add Cursor at Next Occurrence",
            Command::CollapseCursors => "Collapse to One Cursor",
            Command::GotoLine => "Go to Line…",
            Command::GotoBufferStart => "Go to Start of Buffer",
            Command::GotoBufferEnd => "Go to End of Buffer",
            Command::Find => "Find…",
            Command::Replace => "Find and Replace…",
            Command::FindNext => "Find Next",
            Command::FindPrev => "Find Previous",
            Command::ReplaceOne => "Replace",
            Command::ReplaceAll => "Replace All",
            Command::ClearSearch => "Clear Search Highlights",
            Command::SplitPane => "Split Pane Right",
            Command::ClosePane => "Close Pane",
            Command::FocusPaneLeft => "Focus Pane Left",
            Command::FocusPaneRight => "Focus Pane Right",
            Command::ToggleTheme => "Toggle Dark/Light Theme",
            Command::ToggleUiStyle => "Toggle Pixel/Cell UI Style",
            Command::ToggleAutoPairs => "Toggle Auto-Close Pairs",
            Command::ToggleLineNumbers => "Toggle Line Numbers",
            Command::ToggleRelativeNumbers => "Toggle Relative Line Numbers",
            Command::ToggleWhitespace => "Toggle Visible Whitespace",
            Command::ToggleIndentGuides => "Toggle Indent Guides",
            Command::ToggleSyntax => "Toggle Syntax Highlighting",
            Command::ToggleHints => "Toggle Shortcut Hints",
            Command::ToggleCursorLine => "Toggle Cursor Line Highlight",
            Command::ToggleWordWrap => "Toggle Word Wrap",
            Command::WordCount => "Word Count",
            Command::ShowStats => "Document Statistics",
            Command::Help => "Keyboard Shortcuts",
            Command::CommandPalette => "Command Palette…",
        }
    }

    /// Palette grouping.
    pub fn category(self) -> Category {
        match self {
            Command::NewBuffer
            | Command::Open
            | Command::QuickOpen
            | Command::Save
            | Command::SaveAs
            | Command::SaveAll
            | Command::Reload
            | Command::CloseBuffer
            | Command::NextBuffer
            | Command::PrevBuffer
            | Command::Quit
            | Command::QuitAll => Category::File,
            Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::SelectAll
            | Command::DuplicateLine
            | Command::DeleteLine
            | Command::MoveLineUp
            | Command::MoveLineDown
            | Command::ToggleComment
            | Command::Indent
            | Command::Dedent
            | Command::TrimTrailingWhitespace
            | Command::AddCursorAbove
            | Command::AddCursorBelow
            | Command::AddCursorToNextOccurrence
            | Command::CollapseCursors
            // Typing behaviour rather than something on screen, so it belongs
            // with the editing commands and not with the view toggles.
            | Command::ToggleAutoPairs => Category::Edit,
            Command::GotoLine | Command::GotoBufferStart | Command::GotoBufferEnd => {
                Category::Navigate
            }
            Command::Find
            | Command::Replace
            | Command::FindNext
            | Command::FindPrev
            | Command::ReplaceOne
            | Command::ReplaceAll
            | Command::ClearSearch => Category::Search,
            Command::SplitPane
            | Command::ClosePane
            | Command::FocusPaneLeft
            | Command::FocusPaneRight
            | Command::ToggleTheme
            | Command::ToggleUiStyle
            | Command::ToggleLineNumbers
            | Command::ToggleRelativeNumbers
            | Command::ToggleWhitespace
            | Command::ToggleIndentGuides
            | Command::ToggleSyntax
            | Command::ToggleHints
            | Command::ToggleCursorLine
            | Command::ToggleWordWrap
            | Command::WordCount
            | Command::ShowStats => Category::View,
            Command::Help | Command::CommandPalette => Category::Help,
        }
    }

    /// Default bindings, in `ctrl+shift+p` notation.
    ///
    /// Terminals that do not report modifiers reliably collapse some of these
    /// (see `docs/keybindings.md`); the editor still functions, it just loses
    /// the distinction between e.g. `Ctrl+S` and `Ctrl+Shift+S`.
    pub fn default_keys(self) -> &'static [&'static str] {
        match self {
            Command::Save => &["ctrl+s"],
            Command::SaveAs => &["ctrl+shift+s"],
            Command::SaveAll => &[],
            Command::Open => &["ctrl+o"],
            Command::NewBuffer => &["ctrl+n"],
            Command::Find => &["ctrl+f"],
            Command::Replace => &["ctrl+h"],
            Command::Undo => &["ctrl+z"],
            Command::Redo => &["ctrl+y", "ctrl+shift+z"],
            Command::Cut => &["ctrl+x"],
            // `ctrl+shift+c` is the escape hatch for terminals that deliver
            // `SIGINT` instead of a key event for plain `Ctrl+C`.
            Command::Copy => &["ctrl+c", "ctrl+shift+c"],
            Command::Paste => &["ctrl+v"],
            Command::SelectAll => &["ctrl+a"],
            Command::QuickOpen => &["ctrl+p"],
            Command::CommandPalette => &["ctrl+shift+p"],
            Command::GotoLine => &["ctrl+g"],
            Command::CloseBuffer => &["ctrl+w"],
            Command::NextBuffer => &["ctrl+tab"],
            Command::PrevBuffer => &["ctrl+shift+tab"],
            Command::Help => &["f1"],
            Command::Quit => &["ctrl+q"],
            Command::QuitAll => &["ctrl+shift+q"],
            Command::FindNext => &["f3"],
            Command::FindPrev => &["shift+f3"],
            Command::Reload => &["ctrl+shift+r"],
            Command::DuplicateLine => &["ctrl+shift+d"],
            Command::DeleteLine => &["ctrl+shift+k"],
            Command::MoveLineUp => &["alt+up"],
            Command::MoveLineDown => &["alt+down"],
            Command::ToggleComment => &["ctrl+/"],
            // `ctrl+shift` is the binding most terminals deliver reliably:
            // `ctrl+alt` is swallowed by some of them, and on macOS `alt` is
            // often the compose key. Both are kept, because neither costs
            // anything and the older one is already in people's fingers.
            Command::AddCursorAbove => &["ctrl+shift+up", "ctrl+alt+up"],
            Command::AddCursorBelow => &["ctrl+shift+down", "ctrl+alt+down"],
            Command::AddCursorToNextOccurrence => &["ctrl+d"],
            Command::SplitPane => &["ctrl+\\"],
            Command::ClosePane => &["ctrl+alt+w"],
            Command::FocusPaneLeft => &["ctrl+alt+left"],
            Command::FocusPaneRight => &["ctrl+alt+right"],
            Command::ToggleTheme => &["ctrl+shift+t"],
            Command::ToggleUiStyle => &["ctrl+shift+u"],
            Command::ToggleAutoPairs => &["ctrl+shift+a"],
            Command::ToggleWhitespace => &["alt+z"],
            Command::ToggleIndentGuides => &["ctrl+shift+g"],
            Command::ToggleWordWrap => &["alt+w"],
            Command::GotoBufferStart => &["ctrl+home"],
            Command::GotoBufferEnd => &["ctrl+end"],
            Command::WordCount => &["ctrl+shift+w"],
            _ => &[],
        }
    }

    /// Look a command up by its stable id.
    pub fn from_id(id: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| c.id() == id)
    }

    /// True when the command needs an open buffer.
    pub fn needs_buffer(self) -> bool {
        !matches!(
            self,
            Command::NewBuffer
                | Command::Open
                | Command::QuickOpen
                | Command::Quit
                | Command::QuitAll
                | Command::CommandPalette
                | Command::Help
                | Command::SplitPane
                | Command::ClosePane
                | Command::ToggleTheme
                | Command::ToggleUiStyle
                | Command::ToggleAutoPairs
                | Command::ToggleHints
                | Command::ToggleLineNumbers
                | Command::ToggleRelativeNumbers
                | Command::ToggleSyntax
                | Command::ToggleWhitespace
                | Command::ToggleIndentGuides
                | Command::ToggleCursorLine
                | Command::ToggleWordWrap
                | Command::ShowStats
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_commands_have_unique_ids_and_titles() {
        let mut ids = HashSet::new();
        let mut titles = HashSet::new();
        for c in Command::ALL {
            assert!(ids.insert(c.id()), "duplicate id {}", c.id());
            assert!(titles.insert(c.title()), "duplicate title {}", c.title());
        }
    }

    #[test]
    fn all_variants_are_listed() {
        // Guards against forgetting to add a new variant to `ALL`, which would
        // make it invisible to the palette and the keymap builder.
        assert_eq!(Command::ALL.len(), 59);
    }

    #[test]
    fn from_id_roundtrips() {
        for c in Command::ALL {
            assert_eq!(Command::from_id(c.id()), Some(*c));
        }
        assert_eq!(Command::from_id("nope"), None);
    }

    #[test]
    fn ids_are_namespaced() {
        for c in Command::ALL {
            assert!(c.id().contains('.'), "{} is not namespaced", c.id());
        }
    }
}
