//! Overlay interaction: palette, find/replace, prompts, confirmations, help.
//!
//! All modal input handling lives here so that `App::handle_key` stays a thin
//! router: if an overlay captures input, it goes here, otherwise it goes to the
//! editor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use crate::buffer::Edit;
use crate::commands::Command;
use crate::fuzzy;
use crate::search::{self, Query};
use crate::ui::{
    FindBar, HelpState, Overlay, Palette, PaletteAction, PaletteItem, PaletteMode, ReplaceBar,
    TextInput,
};

/// How many palette rows are built at most.
const PALETTE_LIMIT: usize = 200;

impl App {
    // ------------------------------------------------------------- opening

    pub(crate) fn brackets_cache_reset(&mut self) {
        self.brackets_cache = None;
    }

    pub fn open_palette(&mut self, mode: PaletteMode) {
        if mode == PaletteMode::Files {
            self.ensure_file_index();
        }
        let mut palette = Palette::new(mode);
        // Start from the previous query when re-opening the same picker.
        palette.selected = 0;
        self.ui.overlay = Overlay::Palette(palette);
        self.refresh_palette();
    }

    /// Rebuild palette rows for the current query.
    pub fn refresh_palette(&mut self) {
        let (mode, query) = match &self.ui.overlay {
            Overlay::Palette(p) => (p.mode, p.input.value().to_string()),
            _ => return,
        };
        let items = self.items_for(mode, &query);
        if let Overlay::Palette(palette) = &mut self.ui.overlay {
            palette.items = items;
            palette.selected = 0;
            palette.scroll = 0;
        }
    }

    /// Rebuild the rows of a palette that is currently detached from
    /// `ui.overlay`.
    ///
    /// Modal key handling moves the overlay out of `App` to satisfy the borrow
    /// checker, so a detached palette cannot be refreshed through
    /// [`App::refresh_palette`]. Without this the query field would update but
    /// the list would keep showing every command.
    pub(crate) fn refresh_palette_in_place(&mut self, p: &mut Palette) {
        let query = p.input.value().to_string();
        p.items = self.items_for(p.mode, &query);
        p.selected = 0;
        p.scroll = 0;
    }

    fn items_for(&self, mode: PaletteMode, query: &str) -> Vec<PaletteItem> {
        match mode {
            PaletteMode::Commands => self.command_items(query),
            PaletteMode::Files => self.file_items(query),
        }
    }

    fn command_items(&self, query: &str) -> Vec<PaletteItem> {
        let mut scored: Vec<(i32, PaletteItem)> = Vec::new();
        for cmd in Command::ALL.iter().copied() {
            let Some(m) = fuzzy::score(query, cmd.title()) else {
                continue;
            };
            let keys = self.keymap.label_for(cmd);
            let detail = if keys.is_empty() {
                cmd.category().label().to_string()
            } else {
                keys
            };
            scored.push((
                m.score,
                PaletteItem {
                    label: cmd.title().to_string(),
                    detail,
                    positions: m.positions,
                    action: PaletteAction::Run(cmd),
                },
            ));
        }
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored
            .into_iter()
            .take(PALETTE_LIMIT)
            .map(|(_, item)| item)
            .collect()
    }

    fn file_items(&self, query: &str) -> Vec<PaletteItem> {
        let Some(index) = &self.file_index else {
            return Vec::new();
        };
        let hits = fuzzy::filter(index, query, |s| s.clone(), PALETTE_LIMIT);
        hits.into_iter()
            .map(|(idx, m)| {
                let rel = index[idx].clone();
                let full = self.cwd.join(&rel);
                PaletteItem {
                    label: rel,
                    detail: String::new(),
                    positions: m.positions,
                    action: PaletteAction::OpenFile(full),
                }
            })
            .collect()
    }

    pub fn open_find(&mut self, with_replace: bool) {
        let existing = self.search.query.clone();
        // Anchor incremental search on where the cursor was when the bar
        // opened, not on the live cursor (see `SearchState::origin`).
        self.search.origin = self.active_doc().map(|d| d.cursor()).unwrap_or(0);
        let mut bar = FindBar::new();
        bar.input = TextInput::with(existing.text.clone());
        bar.case_sensitive = existing.case_sensitive;
        bar.whole_word = existing.whole_word;
        bar.regex = existing.regex;

        if with_replace {
            let mut replace = ReplaceBar::new();
            replace.input = bar.input.clone();
            replace.case_sensitive = bar.case_sensitive;
            replace.whole_word = bar.whole_word;
            replace.regex = bar.regex;
            self.ui.overlay = Overlay::Replace(replace);
        } else {
            self.ui.overlay = Overlay::Find(bar);
        }

        // Pre-select the current word so Ctrl+F behaves like other editors.
        if existing.text.is_empty() {
            if let Some(doc) = self.active_doc() {
                let word = doc.word_at_cursor();
                if !word.is_empty() {
                    let input = TextInput::with(word);
                    match &mut self.ui.overlay {
                        Overlay::Find(f) => f.input = input,
                        Overlay::Replace(r) => r.input = input,
                        _ => {}
                    }
                }
            }
        }
        self.sync_search_from_overlay(true);
    }

    pub fn open_help(&mut self) {
        self.ui.overlay = Overlay::Help(HelpState::default());
    }

    pub fn goto_prompt(&mut self) {
        self.ui.overlay = Overlay::GotoLine(TextInput::new());
    }

    /// Push the overlay's query into the search engine.
    ///
    /// Only valid while the overlay is attached; the modal key handlers hold
    /// the bar detached and call [`App::apply_query`] directly.
    fn sync_search_from_overlay(&mut self, reselect: bool) {
        let query = match &self.ui.overlay {
            Overlay::Find(f) => query_of(f.input.value(), f.case_sensitive, f.whole_word, f.regex),
            Overlay::Replace(r) => {
                query_of(r.input.value(), r.case_sensitive, r.whole_word, r.regex)
            }
            _ => return,
        };
        self.apply_query(query, reselect);
    }

    /// Run a query against the active buffer and focus the nearest match.
    pub(crate) fn apply_query(&mut self, query: Query, reselect: bool) {
        self.search.query = query;
        self.search.active = true;
        let Some(idx) = self.active_index() else {
            return;
        };
        // Disjoint field borrows: `search` and `buffers` are separate fields, so
        // this avoids cloning the document or routing through a helper that
        // borrows all of `self`.
        let origin = self.search.origin;
        self.search.rescan(&self.buffers[idx]);
        if reselect {
            self.search.select_nearest(origin);
        }
        // Jump to the first match while typing, like an incremental search.
        if let Some(range) = self.search.current() {
            if let Some(doc) = self.active_doc_mut() {
                doc.select_range(range);
                doc.scroll_to_cursor();
            }
        }
    }

    // ------------------------------------------------------- search actions

    pub fn find_next(&mut self) {
        if self.search.query.is_empty() {
            self.open_find(false);
            return;
        }
        self.search.active = true;
        if let Some(idx) = self.active_index() {
            self.search.refresh(&self.buffers[idx]);
        }
        if let Some(range) = self.search.next_match() {
            if let Some(doc) = self.active_doc_mut() {
                doc.select_range(range);
                doc.scroll_to_cursor();
            }
        } else {
            self.info("No matches");
        }
    }

    pub fn find_prev(&mut self) {
        if self.search.query.is_empty() {
            self.open_find(false);
            return;
        }
        self.search.active = true;
        if let Some(idx) = self.active_index() {
            self.search.refresh(&self.buffers[idx]);
        }
        if let Some(range) = self.search.prev_match() {
            if let Some(doc) = self.active_doc_mut() {
                doc.select_range(range);
                doc.scroll_to_cursor();
            }
        } else {
            self.info("No matches");
        }
    }

    /// Replace the currently focused match.
    ///
    /// Reads the replacement from the attached overlay; modal key handling uses
    /// [`App::replace_one_with`] because it holds the bar detached.
    pub fn replace_one(&mut self) {
        let (replacement, regex) = match &self.ui.overlay {
            Overlay::Replace(r) => (r.replace.value().to_string(), r.regex),
            _ => return,
        };
        self.replace_one_with(&replacement, regex);
    }

    pub(crate) fn replace_one_with(&mut self, replacement: &str, regex: bool) {
        let replacement = replacement.to_string();
        let Some(range) = self.search.current() else {
            self.info("No match to replace");
            return;
        };
        let Some(idx) = self.active_index() else {
            return;
        };
        let text = self.buffers[idx].text();
        let value = if regex {
            search::expand_replacement(&replacement, &text, &range, Some(&self.search.query.text))
        } else {
            replacement
        };
        let start = range.start;
        self.buffers[idx].apply_batch(vec![Edit::replace(range, value)], start);
        self.after_edit();
        self.find_next();
    }

    /// Replace every match as a single undoable transaction.
    pub fn replace_all(&mut self) {
        let (replacement, regex) = match &self.ui.overlay {
            Overlay::Replace(r) => (r.replace.value().to_string(), r.regex),
            _ => return,
        };
        self.replace_all_with(&replacement, regex);
    }

    pub(crate) fn replace_all_with(&mut self, replacement: &str, regex: bool) {
        let replacement = replacement.to_string();
        let Some(idx) = self.active_index() else {
            return;
        };
        if self.search.ranges().is_empty() {
            self.info("No matches to replace");
            return;
        }
        let ranges: Vec<std::ops::Range<usize>> = self.search.ranges().to_vec();
        let source = self.buffers[idx].text();
        let mut edits = Vec::with_capacity(ranges.len());
        for range in &ranges {
            let value = if regex {
                search::expand_replacement(
                    &replacement,
                    &source,
                    range,
                    Some(&self.search.query.text),
                )
            } else {
                replacement.clone()
            };
            edits.push(Edit::replace(range.clone(), value));
        }
        let cursor = ranges[0].start;
        let count = ranges.len();
        self.buffers[idx].apply_batch(edits, cursor);
        self.after_edit();
        self.success(format!("Replaced {count} occurrence(s)"));
    }

    // ------------------------------------------------------- overlay input

    pub(crate) fn handle_overlay_key(&mut self, key: KeyEvent) {
        let overlay = std::mem::replace(&mut self.ui.overlay, Overlay::None);
        match overlay {
            Overlay::None => {
                self.ui.overlay = Overlay::None;
            }
            Overlay::Palette(mut p) => {
                let outcome = palette_key(self, &mut p, &key);
                match outcome {
                    PaletteOutcome::Open => self.ui.overlay = Overlay::Palette(p),
                    PaletteOutcome::Accept(action) => {
                        self.ui.overlay = Overlay::None;
                        self.run_palette_action(action);
                    }
                    PaletteOutcome::Cancel => self.ui.overlay = Overlay::None,
                }
            }
            Overlay::Find(mut f) => {
                let outcome = find_key(self, &mut f, &key);
                if outcome {
                    self.ui.overlay = Overlay::Find(f);
                }
            }
            Overlay::Replace(mut r) => {
                let outcome = replace_key(self, &mut r, &key);
                if outcome {
                    self.ui.overlay = Overlay::Replace(r);
                }
            }
            Overlay::GotoLine(mut input) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let value = input.value().trim().to_string();
                    if let Some((line, col)) = parse_line_col(&value) {
                        if let Some(doc) = self.active_doc_mut() {
                            doc.goto_line_col(
                                line.saturating_sub(1),
                                col.unwrap_or(0).saturating_sub(1),
                            );
                        }
                    } else {
                        self.warn(format!("Not a line number: {value}"));
                        self.ui.overlay = Overlay::GotoLine(input);
                    }
                }
                _ => {
                    edit_prompt(&mut input, &key);
                    self.ui.overlay = Overlay::GotoLine(input);
                }
            },
            Overlay::Open(mut p) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let path = self.resolve(&std::path::PathBuf::from(p.input.value()));
                    self.open_path(path);
                }
                KeyCode::Tab => {
                    if let Some(choice) = p.completion.get(p.selected).cloned() {
                        let value = super::files::apply_completion(p.input.value(), &choice);
                        p.input.set(value);
                        p.completion = super::files::complete_path(p.input.value(), &self.cwd);
                        p.selected = 0;
                    }
                    self.ui.overlay = Overlay::Open(p);
                }
                KeyCode::Up => {
                    p.selected = p.selected.saturating_sub(1);
                    self.ui.overlay = Overlay::Open(p);
                }
                KeyCode::Down => {
                    if p.selected + 1 < p.completion.len() {
                        p.selected += 1;
                    }
                    self.ui.overlay = Overlay::Open(p);
                }
                _ => {
                    edit_prompt(&mut p.input, &key);
                    p.completion = super::files::complete_path(p.input.value(), &self.cwd);
                    p.selected = 0;
                    self.ui.overlay = Overlay::Open(p);
                }
            },
            Overlay::SaveAs(mut p) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let value = p.input.value().trim().to_string();
                    if value.is_empty() {
                        self.warn("Enter a file name");
                        self.ui.overlay = Overlay::SaveAs(p);
                    } else if let Some(idx) = self.active_index() {
                        let path = std::path::PathBuf::from(value);
                        self.save_to_path(idx, path);
                    }
                }
                KeyCode::Tab => {
                    if let Some(choice) = p.completion.get(p.selected).cloned() {
                        let value = super::files::apply_completion(p.input.value(), &choice);
                        p.input.set(value);
                        p.completion = super::files::complete_path(p.input.value(), &self.cwd);
                        p.selected = 0;
                    }
                    self.ui.overlay = Overlay::SaveAs(p);
                }
                KeyCode::Up => {
                    p.selected = p.selected.saturating_sub(1);
                    self.ui.overlay = Overlay::SaveAs(p);
                }
                KeyCode::Down => {
                    if p.selected + 1 < p.completion.len() {
                        p.selected += 1;
                    }
                    self.ui.overlay = Overlay::SaveAs(p);
                }
                _ => {
                    edit_prompt(&mut p.input, &key);
                    p.completion = super::files::complete_path(p.input.value(), &self.cwd);
                    p.selected = 0;
                    self.ui.overlay = Overlay::SaveAs(p);
                }
            },
            Overlay::Confirm(mut c) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    super::files::handle_confirm(self, c.action.clone(), c.selected);
                }
                KeyCode::Left | KeyCode::Up => {
                    c.selected = c.selected.saturating_sub(1);
                    self.ui.overlay = Overlay::Confirm(c);
                }
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                    if c.selected + 1 < c.items.len() {
                        c.selected += 1;
                    }
                    self.ui.overlay = Overlay::Confirm(c);
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    super::files::handle_confirm(self, c.action.clone(), 0);
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    super::files::handle_confirm(self, c.action.clone(), c.items.len() - 1);
                }
                _ => self.ui.overlay = Overlay::Confirm(c),
            },
            Overlay::Help(mut h) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(1) => {}
                KeyCode::Down => {
                    h.scroll += 1;
                    self.ui.overlay = Overlay::Help(h);
                }
                KeyCode::Up => {
                    h.scroll = h.scroll.saturating_sub(1);
                    self.ui.overlay = Overlay::Help(h);
                }
                KeyCode::PageDown => {
                    h.scroll += 15;
                    self.ui.overlay = Overlay::Help(h);
                }
                KeyCode::PageUp => {
                    h.scroll = h.scroll.saturating_sub(15);
                    self.ui.overlay = Overlay::Help(h);
                }
                _ => self.ui.overlay = Overlay::Help(h),
            },
        }
    }

    fn run_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::Run(cmd) => self.execute(cmd),
            PaletteAction::OpenFile(path) => self.open_path(path),
        }
    }
}

/// What a key press did to the palette.
enum PaletteOutcome {
    Open,
    Accept(PaletteAction),
    Cancel,
}

fn palette_key(app: &mut App, p: &mut Palette, key: &KeyEvent) -> PaletteOutcome {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => return PaletteOutcome::Cancel,
        KeyCode::Enter => {
            if let Some(item) = p.items.get(p.selected) {
                return PaletteOutcome::Accept(item.action.clone());
            }
            // Nothing to run: keep the palette open so the query can be fixed.
            return PaletteOutcome::Open;
        }
        KeyCode::Up => {
            p.selected = p.selected.saturating_sub(1);
            return PaletteOutcome::Open;
        }
        KeyCode::Down => {
            if p.selected + 1 < p.items.len() {
                p.selected += 1;
            }
            return PaletteOutcome::Open;
        }
        KeyCode::PageUp => {
            p.selected = p.selected.saturating_sub(10);
            return PaletteOutcome::Open;
        }
        KeyCode::PageDown => {
            p.selected = (p.selected + 10).min(p.items.len().saturating_sub(1));
            return PaletteOutcome::Open;
        }
        KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
            if p.selected + 1 < p.items.len() {
                p.selected += 1;
            }
            return PaletteOutcome::Open;
        }
        KeyCode::Char('p') | KeyCode::Char('k') if ctrl => {
            p.selected = p.selected.saturating_sub(1);
            return PaletteOutcome::Open;
        }
        KeyCode::Char('w') if ctrl => {
            p.input.delete_word_back();
            app.refresh_palette_in_place(p);
            return PaletteOutcome::Open;
        }
        _ => {}
    }
    if edit_prompt(&mut p.input, key) {
        app.refresh_palette_in_place(p);
    }
    PaletteOutcome::Open
}

/// Returns false when the overlay should close.
fn find_key(app: &mut App, f: &mut FindBar, key: &KeyEvent) -> bool {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => return false,
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.find_prev();
            } else {
                app.find_next();
            }
            return true;
        }
        KeyCode::Down => {
            app.find_next();
            return true;
        }
        KeyCode::Up => {
            app.find_prev();
            return true;
        }
        KeyCode::Char('c') if alt => {
            f.case_sensitive = !f.case_sensitive;
            app.apply_query(query_of_find(f), true);
            return true;
        }
        KeyCode::Char('w') if alt => {
            f.whole_word = !f.whole_word;
            app.apply_query(query_of_find(f), true);
            return true;
        }
        KeyCode::Char('r') if alt => {
            f.regex = !f.regex;
            app.apply_query(query_of_find(f), true);
            return true;
        }
        _ => {}
    }
    if edit_prompt(&mut f.input, key) {
        app.apply_query(query_of_find(f), true);
    }
    true
}

/// Returns false when the overlay should close.
fn replace_key(app: &mut App, r: &mut ReplaceBar, key: &KeyEvent) -> bool {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => return false,
        KeyCode::Enter if ctrl => {
            let (replacement, regex) = (r.replace.value().to_string(), r.regex);
            app.replace_one_with(&replacement, regex);
            return true;
        }
        KeyCode::Enter if alt => {
            let (replacement, regex) = (r.replace.value().to_string(), r.regex);
            app.replace_all_with(&replacement, regex);
            return true;
        }
        KeyCode::Enter => {
            if r.focus_replace {
                let (replacement, regex) = (r.replace.value().to_string(), r.regex);
                app.replace_one_with(&replacement, regex);
            } else if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.find_prev();
            } else {
                app.find_next();
            }
            return true;
        }
        KeyCode::Tab => {
            r.focus_replace = !r.focus_replace;
            return true;
        }
        KeyCode::Down => {
            app.find_next();
            return true;
        }
        KeyCode::Up => {
            app.find_prev();
            return true;
        }
        KeyCode::Char('c') if alt => {
            r.case_sensitive = !r.case_sensitive;
            app.apply_query(query_of_replace(r), true);
            return true;
        }
        KeyCode::Char('w') if alt => {
            r.whole_word = !r.whole_word;
            app.apply_query(query_of_replace(r), true);
            return true;
        }
        KeyCode::Char('r') if alt => {
            r.regex = !r.regex;
            app.apply_query(query_of_replace(r), true);
            return true;
        }
        _ => {}
    }
    if r.focus_replace {
        edit_prompt(&mut r.replace, key);
    } else if edit_prompt(&mut r.input, key) {
        app.apply_query(query_of_replace(r), true);
    }
    true
}

/// Build a search query from a find bar.
fn query_of_find(f: &FindBar) -> Query {
    query_of(f.input.value(), f.case_sensitive, f.whole_word, f.regex)
}

/// Build a search query from a replace bar.
fn query_of_replace(r: &ReplaceBar) -> Query {
    query_of(r.input.value(), r.case_sensitive, r.whole_word, r.regex)
}

fn query_of(text: &str, case_sensitive: bool, whole_word: bool, regex: bool) -> Query {
    Query {
        text: text.to_string(),
        case_sensitive,
        whole_word,
        regex,
    }
}

/// Apply the standard single-line editing keys. Returns true when handled.
fn edit_prompt(input: &mut TextInput, key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                match c {
                    'a' => input.home(),
                    'e' => input.end(),
                    'w' => input.delete_word_back(),
                    'u' => input.clear(),
                    'h' => input.backspace(),
                    'k' => {
                        input.end();
                        let keep = input.value().to_string();
                        let _ = keep;
                    }
                    _ => return false,
                }
            } else {
                input.insert_char(c);
            }
            true
        }
        KeyCode::Backspace => {
            input.backspace();
            true
        }
        KeyCode::Delete => {
            input.delete();
            true
        }
        KeyCode::Left => {
            if ctrl {
                input.word_left();
            } else {
                input.left();
            }
            true
        }
        KeyCode::Right => {
            if ctrl {
                input.word_right();
            } else {
                input.right();
            }
            true
        }
        KeyCode::Home => {
            input.home();
            true
        }
        KeyCode::End => {
            input.end();
            true
        }
        _ => false,
    }
}

/// Paste text into whichever prompt is focused.
pub(crate) fn insert_into_focused_prompt(app: &mut App, text: &str) {
    let overlay = std::mem::replace(&mut app.ui.overlay, Overlay::None);
    let mut refresh = false;
    app.ui.overlay = match overlay {
        Overlay::Palette(mut p) => {
            p.input.insert_str(text);
            refresh = true;
            Overlay::Palette(p)
        }
        Overlay::Find(mut f) => {
            f.input.insert_str(text);
            refresh = true;
            Overlay::Find(f)
        }
        Overlay::Replace(mut r) => {
            if r.focus_replace {
                r.replace.insert_str(text);
            } else {
                r.input.insert_str(text);
                refresh = true;
            }
            Overlay::Replace(r)
        }
        Overlay::GotoLine(mut i) => {
            i.insert_str(text);
            Overlay::GotoLine(i)
        }
        Overlay::Open(mut p) => {
            p.input.insert_str(text);
            Overlay::Open(p)
        }
        Overlay::SaveAs(mut p) => {
            p.input.insert_str(text);
            Overlay::SaveAs(p)
        }
        other => other,
    };
    if refresh {
        app.refresh_palette();
        app.sync_search_from_overlay(true);
    }
}

/// Parse `12` or `12:5`.
fn parse_line_col(value: &str) -> Option<(usize, Option<usize>)> {
    let mut parts = value.split(':');
    let line: usize = parts.next()?.trim().parse().ok()?;
    if line == 0 {
        return None;
    }
    let col = parts.next().and_then(|c| c.trim().parse::<usize>().ok());
    Some((line, col))
}
