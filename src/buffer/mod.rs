//! Document state: text storage, cursor, selection, undo history and metadata.
//!
//! A [`Document`] is the unit of editing. It owns its storage, its cursor, its
//! selection anchor and its undo history. It knows nothing about terminals,
//! rendering or key bindings, which is what makes the editing core testable in
//! isolation (see `tests/editing_core.rs`).

pub mod motion;
pub mod scan;
pub mod storage;
pub mod undo;

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use crate::filesystem::{Encoding, Eol};
use crate::syntax::Language;

pub use motion::{Motion, MotionCtx};
pub use storage::{RopeStorage, Snapshot, TextStorage};
pub use undo::TxKind;

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// One replacement inside a batch edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            range: at..at,
            text: text.into(),
        }
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self {
            range,
            text: String::new(),
        }
    }
}

/// One caret in a multi-cursor session: a position and an optional selection
/// anchor.
///
/// The primary caret is stored separately on [`Document`] as `cursor`/`anchor`
/// because every existing code path (status bar, search, mouse, scroll) reads
/// it; secondary carets live in [`Document::extra_carets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub pos: usize,
    pub anchor: Option<usize>,
}

impl Caret {
    /// A collapsed caret at `pos`.
    pub fn at(pos: usize) -> Self {
        Self { pos, anchor: None }
    }

    /// A caret at `pos` selecting from `anchor`.
    pub fn selecting(pos: usize, anchor: usize) -> Self {
        Self {
            pos,
            anchor: Some(anchor),
        }
    }

    /// Normalised selection range, if this caret has a non-empty one.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.pos {
            return None;
        }
        Some(if anchor < self.pos {
            anchor..self.pos
        } else {
            self.pos..anchor
        })
    }

    /// The region this caret occupies: its selection, or the empty range at
    /// its position.
    pub fn span(&self) -> Range<usize> {
        self.selection().unwrap_or(self.pos..self.pos)
    }

    /// True when the selection was made backwards, i.e. the cursor sits on the
    /// left edge and the anchor on the right.
    ///
    /// Direction is invisible in the painted output — a selection looks the
    /// same either way — but it decides which end an arrow key moves from, so
    /// carets that disagree about it drift apart the moment the user moves.
    pub fn is_backward(&self) -> bool {
        matches!(self.anchor, Some(anchor) if anchor > self.pos)
    }

    /// The same selection with the cursor and anchor swapped, so the cursor
    /// lands on the opposite edge. A collapsed caret is already directionless
    /// and is returned unchanged.
    pub fn flipped(&self) -> Self {
        match self.anchor {
            Some(anchor) => Self {
                pos: anchor,
                anchor: Some(self.pos),
            },
            None => *self,
        }
    }

    /// This caret with its direction forced to match `backward`.
    pub fn aligned_with(&self, backward: bool) -> Self {
        if self.is_backward() == backward {
            *self
        } else {
            self.flipped()
        }
    }
}

/// One replacement belonging to a specific caret.
///
/// `primary` marks the edit produced by the primary caret: after the batch the
/// primary caret lands at the end of that edit's text, which keeps the terminal
/// cursor and the status bar where the user last was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaretEdit {
    pub range: Range<usize>,
    pub text: String,
    pub primary: bool,
}

impl CaretEdit {
    pub fn new(range: Range<usize>, text: impl Into<String>, primary: bool) -> Self {
        Self {
            range,
            text: text.into(),
            primary,
        }
    }

    pub fn insert(at: usize, text: impl Into<String>, primary: bool) -> Self {
        Self::new(at..at, text, primary)
    }

    pub fn delete(range: Range<usize>, primary: bool) -> Self {
        Self::new(range, String::new(), primary)
    }
}

/// A single editable document.
#[derive(Debug)]
pub struct Document {
    id: usize,
    storage: Box<dyn TextStorage>,
    path: Option<PathBuf>,
    /// Cursor as a character index.
    cursor: usize,
    /// Selection anchor. `None` means "no selection".
    anchor: Option<usize>,
    /// Secondary carets, kept sorted by position, deduplicated and
    /// non-overlapping with the primary caret.
    extra: Vec<Caret>,
    /// Visual column remembered across vertical motions.
    preferred_col: Option<usize>,
    history: undo::History,
    /// Bumped on every mutation; used to invalidate caches.
    revision: u64,
    eol: Eol,
    encoding: Encoding,
    bom: bool,
    language: Language,
    /// First visible line.
    scroll_top: usize,
    /// First visible display column.
    scroll_left: usize,
    /// Rows available in the last render (used to centre jumps).
    viewport_rows: usize,
    /// Modification time observed at load/save time.
    disk_mtime: Option<SystemTime>,
    /// Size observed at load/save time.
    disk_len: Option<u64>,
    /// Set when the file changed underneath us.
    externally_modified: bool,
}

impl Document {
    /// A new, empty, untitled buffer.
    pub fn untitled() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            storage: Box::new(RopeStorage::new()),
            path: None,
            cursor: 0,
            anchor: None,
            extra: Vec::new(),
            preferred_col: None,
            history: undo::History::default(),
            revision: 1,
            eol: Eol::Lf,
            encoding: Encoding::Utf8,
            bom: false,
            language: Language::Plain,
            scroll_top: 0,
            scroll_left: 0,
            viewport_rows: 20,
            disk_mtime: None,
            disk_len: None,
            externally_modified: false,
        }
    }

    /// A buffer holding `text`, optionally backed by `path`.
    pub fn from_text(text: &str, path: Option<PathBuf>) -> Self {
        let mut doc = Self::untitled();
        doc.storage = Box::new(RopeStorage::from_text(text));
        doc.language = path
            .as_deref()
            .map(Language::from_path)
            .unwrap_or(Language::Plain);
        doc.path = path;
        doc
    }

    /// Stable identifier used by the UI and by async job bookkeeping.
    pub fn id(&self) -> usize {
        self.id
    }

    // ---------------------------------------------------------------- storage

    pub fn storage(&self) -> &dyn TextStorage {
        self.storage.as_ref()
    }

    pub fn len_chars(&self) -> usize {
        self.storage.len_chars()
    }

    pub fn line_count(&self) -> usize {
        self.storage.len_lines()
    }

    pub fn text(&self) -> String {
        self.storage.as_string()
    }

    pub fn line_text(&self, line: usize) -> String {
        self.storage.line_slice(line)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Cheap frozen snapshot for background work (search).
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.storage.snapshot()
    }

    // ------------------------------------------------------------------ paths

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.language = path
            .as_deref()
            .map(Language::from_path)
            .unwrap_or(Language::Plain);
        self.path = path;
    }

    /// File name shown in tabs and the top bar.
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string_lossy().to_string()),
            None => "untitled".to_string(),
        }
    }

    /// Path relative to `base` when possible, for the status bar.
    pub fn display_path(&self, base: Option<&Path>) -> String {
        match &self.path {
            None => "[No Name]".to_string(),
            Some(p) => {
                if let Some(base) = base {
                    if let Ok(rel) = p.strip_prefix(base) {
                        return rel.to_string_lossy().to_string();
                    }
                }
                p.to_string_lossy().to_string()
            }
        }
    }

    // -------------------------------------------------------------- metadata

    pub fn eol(&self) -> Eol {
        self.eol
    }

    pub fn set_eol(&mut self, eol: Eol) {
        self.eol = eol;
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn bom(&self) -> bool {
        self.bom
    }

    pub fn set_encoding(&mut self, encoding: Encoding, bom: bool) {
        self.encoding = encoding;
        self.bom = bom;
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn disk_mtime(&self) -> Option<SystemTime> {
        self.disk_mtime
    }

    pub fn set_disk_meta(&mut self, mtime: Option<SystemTime>, len: Option<u64>) {
        self.disk_mtime = mtime;
        self.disk_len = len;
        self.externally_modified = false;
    }

    pub fn disk_len(&self) -> Option<u64> {
        self.disk_len
    }

    pub fn externally_modified(&self) -> bool {
        self.externally_modified
    }

    pub fn set_externally_modified(&mut self, value: bool) {
        self.externally_modified = value;
    }

    // -------------------------------------------------------------- viewport

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    pub fn set_scroll_top(&mut self, value: usize) {
        self.scroll_top = value.min(self.line_count().saturating_sub(1));
    }

    pub fn scroll_left(&self) -> usize {
        self.scroll_left
    }

    pub fn set_scroll_left(&mut self, value: usize) {
        self.scroll_left = value;
    }

    // ---------------------------------------------------------------- cursor

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Move the cursor, clearing any selection and any secondary carets.
    pub fn set_cursor(&mut self, idx: usize) {
        self.cursor = idx.min(self.len_chars());
        self.anchor = None;
        self.preferred_col = None;
        self.extra.clear();
    }

    /// Move the cursor while keeping the anchor (used by mouse drags).
    pub fn set_cursor_extending(&mut self, idx: usize) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = idx.min(self.len_chars());
        self.extra.clear();
    }

    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    pub fn preferred_col(&self) -> Option<usize> {
        self.preferred_col
    }

    /// Cursor position as zero-based `(line, column_in_chars)`.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let line = self.storage.line_of_char(self.cursor);
        let start = self.storage.line_start(line);
        (line, self.cursor.saturating_sub(start))
    }

    /// Cursor position as one-based `(line, column_in_chars)` for display.
    pub fn cursor_line_col_1(&self) -> (usize, usize) {
        let (l, c) = self.cursor_line_col();
        (l + 1, c + 1)
    }

    /// Apply a motion, optionally extending the selection.
    pub fn move_cursor(&mut self, motion: Motion, extend: bool, ctx: MotionCtx) {
        // Direction of the primary selection before the motion. Secondary
        // carets are aligned to it so that every caret advances from the same
        // edge of its own selection; a caret whose cursor sits on the opposite
        // edge would otherwise move the wrong way and the group would drift.
        let backward = self.primary_caret().is_backward();
        let (next, col) = motion::apply(
            self.storage.as_ref(),
            self.cursor,
            motion,
            ctx,
            self.preferred_col,
        );
        // A motion that is not vertical clears the remembered column.
        self.preferred_col = match motion {
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => col,
            _ => None,
        };
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
            self.cursor = next;
        } else {
            self.cursor = next;
            self.anchor = None;
        }

        // Secondary carets follow the same motion, each keeping its own column.
        if !self.extra.is_empty() {
            let mut moved = Vec::with_capacity(self.extra.len());
            for caret in &self.extra {
                let aligned = caret.aligned_with(backward);
                let (pos, _) = motion::apply(self.storage.as_ref(), aligned.pos, motion, ctx, None);
                moved.push(if extend {
                    Caret {
                        pos,
                        anchor: Some(aligned.anchor.unwrap_or(aligned.pos)),
                    }
                } else {
                    Caret::at(pos)
                });
            }
            self.set_extra_carets(moved);
        }
    }

    // ------------------------------------------------------------- selection

    /// Normalised selection range, if a non-empty selection exists.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(if anchor < self.cursor {
            anchor..self.cursor
        } else {
            self.cursor..anchor
        })
    }

    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    pub fn selected_text(&self) -> String {
        match self.selection() {
            Some(r) => self.storage.slice_chars(r.start, r.end),
            None => String::new(),
        }
    }

    pub fn select_all(&mut self) {
        if self.len_chars() == 0 {
            return;
        }
        self.anchor = Some(0);
        self.cursor = self.len_chars();
        self.extra.clear();
    }

    pub fn select_range(&mut self, range: Range<usize>) {
        self.anchor = Some(range.start.min(self.len_chars()));
        self.cursor = range.end.min(self.len_chars());
        self.preferred_col = None;
        self.extra.clear();
    }

    /// Set the primary caret's selection without disturbing secondary carets.
    ///
    /// Used by "add cursor at next occurrence", which needs to select the word
    /// under the cursor while keeping the cursors already stacked.
    pub fn set_primary_selection(&mut self, range: Range<usize>) {
        let len = self.len_chars();
        self.anchor = Some(range.start.min(len));
        self.cursor = range.end.min(len);
        self.preferred_col = None;
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    // ----------------------------------------------------------------- carets

    /// The primary caret.
    pub fn primary_caret(&self) -> Caret {
        Caret {
            pos: self.cursor,
            anchor: self.anchor,
        }
    }

    /// Secondary carets, sorted by position.
    pub fn extra_carets(&self) -> &[Caret] {
        &self.extra
    }

    /// Every caret, primary included, sorted by position.
    pub fn carets(&self) -> Vec<Caret> {
        let mut all = Vec::with_capacity(self.extra.len() + 1);
        all.push(self.primary_caret());
        all.extend_from_slice(&self.extra);
        all.sort_by_key(|c| c.pos);
        all
    }

    /// Number of carets, always at least one.
    pub fn caret_count(&self) -> usize {
        self.extra.len() + 1
    }

    pub fn has_multiple_carets(&self) -> bool {
        !self.extra.is_empty()
    }

    /// Every non-empty selection, ascending. Used by the renderer.
    pub fn selections(&self) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::with_capacity(self.extra.len() + 1);
        out.extend(self.selection());
        out.extend(self.extra.iter().filter_map(Caret::selection));
        out.sort_by_key(|r| r.start);
        out
    }

    /// Replace the secondary carets, dropping any that collide with the
    /// primary caret or with each other.
    pub fn set_extra_carets(&mut self, extra: Vec<Caret>) {
        self.extra = normalise_carets(self.primary_caret(), extra, self.len_chars());
    }

    /// Add a secondary caret. Returns `false` when it was rejected because it
    /// coincides with, or overlaps, an existing caret.
    pub fn add_caret(&mut self, caret: Caret) -> bool {
        let mut next = self.extra.clone();
        next.push(caret);
        let normalised = normalise_carets(self.primary_caret(), next, self.len_chars());
        if normalised.len() == self.extra.len() {
            return false;
        }
        self.extra = normalised;
        true
    }

    /// Remove the secondary caret at `pos`, if any. Returns whether one was
    /// removed.
    pub fn remove_caret_at(&mut self, pos: usize) -> bool {
        let before = self.extra.len();
        self.extra.retain(|c| c.pos != pos);
        self.extra.len() != before
    }

    /// Drop every secondary caret, keeping the primary one.
    pub fn collapse_carets(&mut self) {
        self.extra.clear();
    }

    /// The caret covering `idx`: either its selection contains the position, or
    /// it is a collapsed caret sitting exactly there.
    ///
    /// `Range::contains` is unusable here: a collapsed caret's span is empty,
    /// and an empty range contains nothing.
    pub fn caret_at(&self, idx: usize) -> Option<Caret> {
        self.carets()
            .into_iter()
            .find(|caret| match caret.selection() {
                Some(sel) => sel.start <= idx && idx < sel.end,
                None => caret.pos == idx,
            })
    }

    /// Highest offset touched by any caret; used to place the next occurrence.
    pub fn max_caret_end(&self) -> usize {
        self.carets()
            .iter()
            .map(|c| c.span().end)
            .max()
            .unwrap_or(self.cursor)
    }

    /// The word under the cursor (used for bracket matching / word count).
    pub fn word_at_cursor(&self) -> String {
        let line = self.storage.line_of_char(self.cursor);
        let start = self.storage.line_start(line);
        let text = self.storage.line_slice(line);
        let col = self.cursor.saturating_sub(start);
        let chars: Vec<char> = text.chars().collect();
        if col >= chars.len() {
            return String::new();
        }
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut lo = col;
        while lo > 0 && is_word(chars[lo - 1]) {
            lo -= 1;
        }
        let mut hi = col;
        while hi < chars.len() && is_word(chars[hi]) {
            hi += 1;
        }
        chars[lo..hi].iter().collect()
    }

    // --------------------------------------------------------------- editing

    /// Replace `start..end` with `text`, recording an undoable transaction.
    ///
    /// Returns `true` when the document actually changed.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str, kind: TxKind) -> bool {
        let len = self.len_chars();
        let start = start.min(len);
        let end = end.clamp(start, len);
        let removed = self.storage.slice_chars(start, end);
        if removed == text {
            // No-op edit: still move the cursor to the end of the "new" text.
            self.cursor = start + text.chars().count();
            self.anchor = None;
            return false;
        }

        let cursor_before = self.cursor;
        let anchor_before = self.anchor;
        let extra_before = std::mem::take(&mut self.extra);

        if !removed.is_empty() {
            self.storage.remove(start, end);
        }
        if !text.is_empty() {
            self.storage.insert(start, text);
        }

        let cursor_after = start + text.chars().count();
        self.cursor = cursor_after;
        self.anchor = None;
        self.preferred_col = None;
        self.revision += 1;

        self.history.push(undo::Transaction {
            changes: vec![undo::Change {
                start,
                removed,
                inserted: text.to_string(),
            }],
            cursor_before,
            anchor_before,
            cursor_after,
            anchor_after: None,
            extra_before,
            extra_after: Vec::new(),
            kind,
            time: std::time::Instant::now(),
        });
        true
    }

    /// Insert text at the cursor, replacing the selection if there is one.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let kind = if text.contains('\n') {
            TxKind::Other
        } else {
            TxKind::Insert
        };
        match self.selection() {
            Some(sel) => {
                self.replace_range(sel.start, sel.end, text, kind);
            }
            None => {
                let c = self.cursor;
                self.replace_range(c, c, text, kind);
            }
        }
    }

    /// Insert a newline, keeping the current indentation (auto-indent).
    pub fn insert_newline(&mut self) {
        let line = self.storage.line_of_char(self.cursor);
        let start = self.storage.line_start(line);
        let before = self.storage.slice_chars(start, self.cursor);
        if before.trim().is_empty() {
            // Cursor sits inside the indentation (or on an empty line): a plain
            // newline is what the user wants, not duplicated indentation.
            self.insert_text("\n");
        } else {
            let indent: String = before
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let text = format!("\n{indent}");
            self.insert_text(&text);
        }
    }

    /// Delete the selection, or the grapheme before the cursor.
    pub fn backspace(&mut self) {
        if let Some(sel) = self.selection() {
            self.replace_range(sel.start, sel.end, "", TxKind::Other);
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let start = motion::prev_grapheme(self.storage.as_ref(), self.cursor);
        self.replace_range(start, self.cursor, "", TxKind::DeleteBack);
    }

    /// Delete the selection, or the grapheme after the cursor.
    pub fn delete_forward(&mut self) {
        if let Some(sel) = self.selection() {
            self.replace_range(sel.start, sel.end, "", TxKind::Other);
            return;
        }
        if self.cursor >= self.len_chars() {
            return;
        }
        let end = motion::next_grapheme(self.storage.as_ref(), self.cursor);
        self.replace_range(self.cursor, end, "", TxKind::DeleteForward);
    }

    /// Delete an explicit range (used by cut and by word deletion).
    pub fn delete_range(&mut self, range: Range<usize>) {
        let start = range.start.min(self.len_chars());
        let end = range.end.clamp(start, self.len_chars());
        if start == end {
            return;
        }
        self.cursor = start;
        self.anchor = None;
        self.replace_range(start, end, "", TxKind::Other);
    }

    /// Apply several edits as one undoable transaction.
    ///
    /// Edits are applied from the highest offset downwards so that every range
    /// is expressed in the pre-edit coordinate system, then stored in ascending
    /// order. This is what makes "replace all" a single undo step without
    /// copying the whole document.
    ///
    /// Secondary carets are dropped: a batch edit shifts offsets in a way only
    /// the caller can map, and silently leaving stale positions behind would
    /// put carets in the wrong place.
    pub fn apply_batch(&mut self, edits: Vec<Edit>, cursor_after: usize) -> bool {
        self.apply_edits(edits, Caret::at(cursor_after), Vec::new(), TxKind::Other)
    }

    /// Apply one edit per caret as a single undoable transaction.
    ///
    /// Each caret ends up immediately after the text it inserted, so a deletion
    /// leaves it at the deletion point and a replacement at the end of the new
    /// text. Carets that collapse onto each other are merged.
    pub fn apply_caret_edits(&mut self, edits: Vec<CaretEdit>, kind: TxKind) -> bool {
        if edits.is_empty() {
            return false;
        }
        let len = self.len_chars();
        let mut sorted = edits;
        sorted.sort_by_key(|e| e.range.start);

        // Post-edit caret positions. Every edit below a caret shifts it by the
        // difference between what was inserted and what was removed.
        let mut shift: isize = 0;
        let mut placed: Vec<(Caret, bool)> = Vec::with_capacity(sorted.len());
        let mut plain: Vec<Edit> = Vec::with_capacity(sorted.len());
        for edit in &sorted {
            let start = edit.range.start.min(len);
            let end = edit.range.end.clamp(start, len);
            let inserted = edit.text.chars().count();
            let pos = (start as isize + shift + inserted as isize).max(0) as usize;
            placed.push((Caret::at(pos), edit.primary));
            shift += inserted as isize - (end - start) as isize;
            plain.push(Edit::replace(edit.range.clone(), edit.text.clone()));
        }

        let primary = placed
            .iter()
            .find(|(_, is_primary)| *is_primary)
            .map(|(caret, _)| *caret)
            .unwrap_or_else(|| Caret::at(self.cursor));
        let extra: Vec<Caret> = placed
            .iter()
            .filter(|(_, is_primary)| !*is_primary)
            .map(|(caret, _)| *caret)
            .collect();

        self.apply_edits(plain, primary, extra, kind)
    }

    /// Shared batch implementation: apply bottom-up, then place the carets.
    fn apply_edits(
        &mut self,
        edits: Vec<Edit>,
        primary_after: Caret,
        extra_after: Vec<Caret>,
        kind: TxKind,
    ) -> bool {
        if edits.is_empty() {
            return false;
        }
        let cursor_before = self.cursor;
        let anchor_before = self.anchor;
        let extra_before = self.extra.clone();

        let mut sorted = edits;
        sorted.sort_by_key(|e| e.range.start);

        let mut changes = Vec::with_capacity(sorted.len());
        for edit in sorted.iter().rev() {
            let start = edit.range.start.min(self.len_chars());
            let end = edit.range.end.clamp(start, self.len_chars());
            let removed = self.storage.slice_chars(start, end);
            if removed == edit.text {
                continue;
            }
            if !removed.is_empty() {
                self.storage.remove(start, end);
            }
            if !edit.text.is_empty() {
                self.storage.insert(start, &edit.text);
            }
            changes.push(undo::Change {
                start,
                removed,
                inserted: edit.text.clone(),
            });
        }

        if changes.is_empty() {
            return false;
        }
        changes.reverse();

        let len = self.len_chars();
        self.cursor = primary_after.pos.min(len);
        self.anchor = primary_after.anchor.map(|a| a.min(len));
        self.set_extra_carets(extra_after);
        self.preferred_col = None;
        self.revision += 1;

        let extra_after = self.extra.clone();
        self.history.push(undo::Transaction {
            changes,
            cursor_before,
            anchor_before,
            cursor_after: self.cursor,
            anchor_after: self.anchor,
            extra_before,
            extra_after,
            kind,
            time: std::time::Instant::now(),
        });
        true
    }

    // ------------------------------------------------------------------ undo

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Undo one transaction. Returns `false` when there is nothing to undo.
    ///
    /// Changes are stored in ascending order and were applied from the end of
    /// the document backwards, so undo replays them forwards (lowest offset
    /// first) and redo replays them backwards.
    pub fn undo(&mut self) -> bool {
        let Some(tx) = self.history.undo() else {
            return false;
        };
        for change in tx.changes.iter() {
            let inserted_len = change.len_inserted();
            if inserted_len > 0 {
                self.storage
                    .remove(change.start, change.start + inserted_len);
            }
            if !change.removed.is_empty() {
                self.storage.insert(change.start, &change.removed);
            }
        }
        self.cursor = tx.cursor_before.min(self.len_chars());
        self.anchor = tx.anchor_before;
        self.set_extra_carets(tx.extra_before);
        self.preferred_col = None;
        self.revision += 1;
        true
    }

    /// Redo one transaction. Returns `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(tx) = self.history.redo() else {
            return false;
        };
        for change in tx.changes.iter().rev() {
            let removed_len = change.len_removed();
            if removed_len > 0 {
                self.storage
                    .remove(change.start, change.start + removed_len);
            }
            if !change.inserted.is_empty() {
                self.storage.insert(change.start, &change.inserted);
            }
        }
        self.cursor = tx.cursor_after.min(self.len_chars());
        self.anchor = tx.anchor_after;
        self.set_extra_carets(tx.extra_after);
        self.preferred_col = None;
        self.revision += 1;
        true
    }

    // ----------------------------------------------------------------- dirty

    /// True when the buffer differs from the last saved state.
    pub fn is_modified(&self) -> bool {
        self.history.is_dirty()
    }

    /// Record that the current content is on disk.
    pub fn mark_saved(&mut self) {
        self.history.mark_saved();
    }

    /// Force the dirty flag (used after a failed write or a reload).
    pub fn invalidate_save_point(&mut self) {
        self.history.invalidate_save_point();
    }

    /// Replace the whole content without creating undo history (used by
    /// reload-from-disk).
    pub fn reset_contents(&mut self, text: &str) {
        self.storage = Box::new(RopeStorage::from_text(text));
        self.cursor = self.cursor.min(self.storage.len_chars());
        self.anchor = None;
        self.extra.clear();
        self.history = undo::History::default();
        self.revision += 1;
    }

    // ------------------------------------------------------------ navigation

    /// Move the cursor to the start of `line` (zero-based).
    pub fn goto_line(&mut self, line: usize) {
        let line = line.min(self.line_count().saturating_sub(1));
        self.set_cursor(self.storage.line_start(line));
        self.center_scroll(line);
    }

    /// Scroll so the cursor is centred, without touching the cursor or the
    /// selection.
    ///
    /// Search jumps select the match and then only need the view to follow;
    /// using [`Document::goto_line`] there would collapse the selection onto
    /// the first column and lose the match.
    pub fn scroll_to_cursor(&mut self) {
        let line = self.storage.line_of_char(self.cursor);
        self.center_scroll(line);
    }

    /// Move the cursor to a zero-based line and column.
    pub fn goto_line_col(&mut self, line: usize, col: usize) {
        let line = line.min(self.line_count().saturating_sub(1));
        let start = self.storage.line_start(line);
        let len = self.storage.line_slice(line).chars().count();
        self.set_cursor(start + col.min(len));
        self.center_scroll(line);
    }

    /// Number of rows the UI last gave this buffer. Used to centre jumps.
    pub fn viewport_rows(&self) -> usize {
        self.viewport_rows
    }

    pub fn set_viewport_rows(&mut self, rows: usize) {
        self.viewport_rows = rows.max(1);
    }

    fn center_scroll(&mut self, line: usize) {
        let rows = self.viewport_rows;
        self.scroll_top = line.saturating_sub(rows / 2);
    }

    /// Statistics shown by the word count command.
    pub fn statistics(&self) -> DocumentStats {
        let text = self.storage.as_string();
        let words = text.split_whitespace().count();
        let chars = text.chars().count();
        let lines = self.storage.len_lines();
        let bytes = text.len();
        DocumentStats {
            words,
            chars,
            lines,
            bytes,
        }
    }
}

/// Clamp, sort and de-duplicate secondary carets.
///
/// A caret is dropped when it collides with the primary caret or with one
/// already accepted. Overlap is rejected rather than merged because two carets
/// whose selections intersect cannot be edited in one batch: their edits would
/// have to be applied to a shared range.
fn normalise_carets(primary: Caret, extra: Vec<Caret>, len: usize) -> Vec<Caret> {
    let clamp = |c: Caret| Caret {
        pos: c.pos.min(len),
        anchor: c.anchor.map(|a| a.min(len)),
    };
    let primary = clamp(primary);
    let mut out: Vec<Caret> = Vec::with_capacity(extra.len());
    for caret in extra.into_iter().map(clamp) {
        if caret.pos == primary.pos {
            continue;
        }
        if overlaps(&caret, &primary) {
            continue;
        }
        if out.iter().any(|kept| overlaps(&caret, kept)) {
            continue;
        }
        out.push(caret);
    }
    out.sort_by_key(|c| c.pos);
    out
}

/// True when two carets cannot coexist: their selections intersect, or one is
/// a collapsed caret inside the other's selection (or on its edge).
fn overlaps(a: &Caret, b: &Caret) -> bool {
    let (a0, a1) = (a.span().start, a.span().end);
    let (b0, b1) = (b.span().start, b.span().end);
    if a0 == a1 && b0 == b1 {
        return a0 == b0;
    }
    if a0 == a1 {
        return b0 <= a0 && a0 <= b1;
    }
    if b0 == b1 {
        return a0 <= b0 && b0 <= a1;
    }
    a0 < b1 && b0 < a1
}

/// Document statistics for the word count command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentStats {
    pub words: usize,
    pub chars: usize,
    pub lines: usize,
    pub bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::from_text(text, None)
    }

    #[test]
    fn movement_aligns_carets_that_disagree_about_direction() {
        let mut d = doc("abc xabcdd");
        d.set_cursor(3);
        d.set_cursor_extending(0); // backwards: cursor on 'a', anchor after 'c'
        assert!(d.primary_caret().is_backward());
        d.add_caret(Caret::selecting(8, 5)); // forwards: cursor on 'd'
        assert!(!d.extra_carets()[0].is_backward());

        d.move_cursor(Motion::Right, false, MotionCtx::default());

        assert_eq!(d.cursor(), 1, "the primary moves off 'a'");
        assert_eq!(
            d.extra_carets()[0].pos,
            6,
            "the secondary advances from the same edge instead of running past 'd'"
        );
    }

    #[test]
    fn typing_inserts_at_cursor() {
        let mut d = doc("ac");
        d.set_cursor(1);
        d.insert_text("b");
        assert_eq!(d.text(), "abc");
        assert_eq!(d.cursor(), 2);
        assert!(d.is_modified());
    }

    #[test]
    fn backspace_merges_into_one_undo() {
        let mut d = doc("");
        for c in "hello".chars() {
            d.insert_text(&c.to_string());
        }
        assert_eq!(d.text(), "hello");
        assert!(d.undo());
        assert_eq!(d.text(), "", "typing coalesces into one undo step");
    }

    #[test]
    fn undo_redo_roundtrip_restores_cursor() {
        let mut d = doc("hello world");
        d.set_cursor(5);
        d.insert_text("\n");
        assert_eq!(d.text(), "hello\n world");
        assert_eq!(d.cursor(), 6);
        assert!(d.undo());
        assert_eq!(d.text(), "hello world");
        assert_eq!(d.cursor(), 5);
        assert!(d.redo());
        assert_eq!(d.text(), "hello\n world");
        assert_eq!(d.cursor(), 6);
    }

    #[test]
    fn undo_restores_replaced_text() {
        let mut d = doc("abc");
        d.select_range(0..2);
        d.insert_text("XY");
        assert_eq!(d.text(), "XYc");
        assert!(d.undo());
        assert_eq!(d.text(), "abc");
        assert_eq!(d.selection(), Some(0..2));
    }

    #[test]
    fn selection_normalises_both_directions() {
        let mut d = doc("abcdef");
        d.set_cursor(4);
        d.set_cursor_extending(1);
        assert_eq!(d.selection(), Some(1..4));
        assert_eq!(d.selected_text(), "bcd");
    }

    #[test]
    fn backspace_removes_whole_grapheme() {
        let mut d = doc("a👨‍👩‍👧");
        d.set_cursor(d.len_chars());
        d.backspace();
        assert_eq!(d.text(), "a");
    }

    #[test]
    fn delete_forward_at_eof_is_safe() {
        let mut d = doc("abc");
        d.set_cursor(3);
        d.delete_forward();
        assert_eq!(d.text(), "abc");
    }

    #[test]
    fn auto_indent_on_newline() {
        let mut d = doc("    let x = 1;");
        d.set_cursor(14);
        d.insert_newline();
        assert_eq!(d.text(), "    let x = 1;\n    ");
    }

    #[test]
    fn saved_state_tracks_undo() {
        let mut d = doc("a");
        d.mark_saved();
        assert!(!d.is_modified());
        d.insert_text("b");
        assert!(d.is_modified());
        d.undo();
        assert!(!d.is_modified());
    }

    #[test]
    fn replace_range_is_bounded() {
        let mut d = doc("abc");
        d.replace_range(1, 999, "X", TxKind::Other);
        assert_eq!(d.text(), "aX");
    }

    #[test]
    fn statistics_counts_words() {
        let d = doc("one two\nthree\n");
        let stats = d.statistics();
        assert_eq!(stats.words, 3);
        assert_eq!(stats.lines, 3);
    }

    // ------------------------------------------------------------ multi-cursor

    fn caret_positions(d: &Document) -> Vec<usize> {
        d.carets().iter().map(|c| c.pos).collect()
    }

    #[test]
    fn carets_stay_sorted_and_unique() {
        let mut d = doc("abcdef");
        d.set_cursor(0);
        assert!(d.add_caret(Caret::at(4)));
        assert!(d.add_caret(Caret::at(2)));
        assert_eq!(caret_positions(&d), vec![0, 2, 4]);
        assert!(!d.add_caret(Caret::at(4)), "duplicate is rejected");
        assert!(!d.add_caret(Caret::at(0)), "the primary is rejected");
        assert_eq!(d.caret_count(), 3);
    }

    #[test]
    fn overlapping_selections_are_rejected() {
        let mut d = doc("abcdef");
        d.select_range(1..4);
        assert!(
            d.add_caret(Caret::selecting(6, 4)),
            "a selection starting where another ends is fine"
        );
        assert!(!d.add_caret(Caret::at(2)), "inside an existing selection");
        assert!(!d.add_caret(Caret::selecting(3, 5)), "overlaps it");
    }

    #[test]
    fn caret_edits_place_every_caret() {
        let mut d = doc("aaa\nbbb\nccc");
        d.set_cursor(0);
        d.add_caret(Caret::at(4));
        d.add_caret(Caret::at(8));
        let edits: Vec<CaretEdit> = d
            .carets()
            .iter()
            .map(|c| CaretEdit::insert(c.pos, "X", c.pos == 0))
            .collect();
        assert!(d.apply_caret_edits(edits, TxKind::Insert));
        assert_eq!(d.text(), "Xaaa\nXbbb\nXccc");
        assert_eq!(caret_positions(&d), vec![1, 6, 11]);
        assert_eq!(d.cursor(), 1, "the primary stays primary");
    }

    #[test]
    fn caret_edits_are_one_undo_step() {
        let mut d = doc("a\nb\n");
        d.set_cursor(0);
        d.add_caret(Caret::at(2));
        let edits: Vec<CaretEdit> = d
            .carets()
            .iter()
            .map(|c| CaretEdit::insert(c.pos, "X", c.pos == 0))
            .collect();
        d.apply_caret_edits(edits, TxKind::Insert);
        assert_eq!(d.text(), "Xa\nXb\n");

        assert!(d.undo());
        assert_eq!(d.text(), "a\nb\n", "one undo reverts every caret");
        assert_eq!(
            caret_positions(&d),
            vec![0, 2],
            "and restores the caret set"
        );
        assert_eq!(d.caret_count(), 2);

        assert!(d.redo());
        assert_eq!(d.text(), "Xa\nXb\n");
        assert_eq!(caret_positions(&d), vec![1, 4]);
    }

    #[test]
    fn motion_moves_every_caret() {
        let mut d = doc("abcdef\nghijkl");
        d.set_cursor(1);
        d.add_caret(Caret::at(4));
        d.move_cursor(Motion::Down, false, MotionCtx::default());
        assert_eq!(caret_positions(&d), vec![8, 11]);
    }

    #[test]
    fn carets_that_land_together_merge() {
        let mut d = doc("abc");
        d.set_cursor(1);
        d.add_caret(Caret::at(2));
        d.move_cursor(Motion::Left, false, MotionCtx::default());
        assert_eq!(caret_positions(&d), vec![0, 1]);
        // One more step left and both carets land on 0.
        d.move_cursor(Motion::Left, false, MotionCtx::default());
        assert_eq!(d.caret_count(), 1);
        assert_eq!(d.cursor(), 0);
    }

    #[test]
    fn batch_edit_collapses_secondary_carets() {
        let mut d = doc("a\nb\n");
        d.add_caret(Caret::at(2));
        assert!(d.apply_batch(vec![Edit::insert(0, "X")], 1));
        assert_eq!(d.caret_count(), 1, "batch edits cannot remap carets");
    }

    #[test]
    fn absolute_jumps_collapse_carets() {
        let mut d = doc("a\nb\nc");
        d.add_caret(Caret::at(2));
        d.set_cursor(4);
        assert_eq!(d.caret_count(), 1);
    }

    #[test]
    fn selections_lists_every_caret_range() {
        let mut d = doc("foo bar foo");
        d.select_range(0..3);
        d.add_caret(Caret::selecting(11, 8));
        assert_eq!(d.selections(), vec![0..3, 8..11]);
    }
}
