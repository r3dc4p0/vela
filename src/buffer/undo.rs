//! Undo/redo history.
//!
//! Every mutation is recorded as a [`Transaction`] holding one or more
//! [`Change`]s. A transaction stores the cursor *and* selection anchor before
//! and after the edit, so undo restores exactly the state the user saw.
//!
//! Consecutive single-character typing (and consecutive backspaces) is folded
//! into one transaction, which is what makes undo feel natural instead of
//! undoing one letter at a time. Coalescing is bounded by both a time window and
//! a character budget, and is disabled across a save point so that "save, keep
//! typing, undo" cannot silently return to a state that was never saved.

use std::time::{Duration, Instant};

/// One contiguous replacement: `removed` at `start` became `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub removed: String,
    pub inserted: String,
}

impl Change {
    pub fn len_removed(&self) -> usize {
        self.removed.chars().count()
    }

    pub fn len_inserted(&self) -> usize {
        self.inserted.chars().count()
    }
}

/// Coarse classification used to decide whether two edits may be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxKind {
    Insert,
    DeleteBack,
    DeleteForward,
    Other,
}

/// A single undoable step.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub changes: Vec<Change>,
    pub cursor_before: usize,
    pub anchor_before: Option<usize>,
    pub cursor_after: usize,
    pub anchor_after: Option<usize>,
    pub kind: TxKind,
    pub time: Instant,
}

impl Transaction {
    fn inserted_chars(&self) -> usize {
        self.changes.iter().map(Change::len_inserted).sum()
    }

    fn removed_chars(&self) -> usize {
        self.changes.iter().map(Change::len_removed).sum()
    }

    fn bytes(&self) -> usize {
        self.changes
            .iter()
            .map(|c| c.removed.len() + c.inserted.len())
            .sum()
    }
}

/// Bounded linear undo history.
#[derive(Debug)]
pub struct History {
    entries: std::collections::VecDeque<Transaction>,
    /// Number of applied entries. `entries[index..]` is the redo stack.
    index: usize,
    /// Index of the entry that corresponds to the on-disk state.
    saved_index: Option<usize>,
    /// Suppress merging into the previous entry (set after save / undo / redo).
    barrier: bool,
    limit_entries: usize,
    limit_bytes: usize,
    bytes: usize,
}

/// Typing is coalesced while the gap between keystrokes stays below this.
const COALESCE_WINDOW: Duration = Duration::from_millis(500);
/// ...and while the merged run stays below this many characters.
const COALESCE_MAX_CHARS: usize = 512;

impl Default for History {
    fn default() -> Self {
        Self::new(5_000, 64 * 1024 * 1024)
    }
}

impl History {
    pub fn new(limit_entries: usize, limit_bytes: usize) -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            index: 0,
            saved_index: Some(0),
            barrier: true,
            limit_entries,
            limit_bytes,
            bytes: 0,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.index < self.entries.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_index != Some(self.index)
    }

    /// Number of applied entries (monotonic only within a save epoch).
    pub fn depth(&self) -> usize {
        self.index
    }

    pub fn mark_saved(&mut self) {
        self.saved_index = Some(self.index);
        self.barrier = true;
    }

    /// Forget the saved state (used when a buffer's file disappears).
    pub fn invalidate_save_point(&mut self) {
        self.saved_index = None;
    }

    /// Record an edit. Returns `false` when the edit was merged into the
    /// previous transaction.
    pub fn push(&mut self, mut tx: Transaction) -> bool {
        tx.time = Instant::now();
        // A new edit invalidates the redo stack.
        while self.entries.len() > self.index {
            if let Some(dropped) = self.entries.pop_back() {
                self.bytes = self.bytes.saturating_sub(dropped.bytes());
            }
        }

        if !self.barrier && self.index > 0 {
            if let Some(last) = self.entries.back_mut() {
                if let Some(merged) = try_merge(last, &tx) {
                    self.bytes = self
                        .bytes
                        .saturating_sub(last.bytes())
                        .saturating_add(merged.bytes());
                    *last = merged;
                    self.barrier = false;
                    return false;
                }
            }
        }

        self.bytes = self.bytes.saturating_add(tx.bytes());
        self.entries.push_back(tx);
        self.index = self.entries.len();
        self.barrier = false;
        self.trim();
        true
    }

    /// The transaction to undo, together with the cursor state to restore.
    pub fn undo(&mut self) -> Option<Transaction> {
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        self.barrier = true;
        self.entries.get(self.index).cloned()
    }

    /// The transaction to redo.
    pub fn redo(&mut self) -> Option<Transaction> {
        if self.index >= self.entries.len() {
            return None;
        }
        let tx = self.entries.get(self.index).cloned();
        self.index += 1;
        self.barrier = true;
        tx
    }

    fn trim(&mut self) {
        let mut trimmed = 0usize;
        while self.entries.len() > self.limit_entries || self.bytes > self.limit_bytes {
            if self.entries.len() <= 1 {
                break;
            }
            if let Some(dropped) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(dropped.bytes());
                trimmed += 1;
            }
        }
        if trimmed > 0 {
            self.index = self.index.saturating_sub(trimmed);
            self.saved_index = match self.saved_index {
                // The saved state fell out of the history window: stay dirty
                // until the next explicit save.
                Some(s) if s < trimmed => None,
                Some(s) => Some(s - trimmed),
                None => None,
            };
        }
    }
}

/// Attempt to fold `next` into `prev`. Returns the merged transaction.
fn try_merge(prev: &Transaction, next: &Transaction) -> Option<Transaction> {
    if prev.kind != next.kind {
        return None;
    }
    if next.time.duration_since(prev.time) > COALESCE_WINDOW {
        return None;
    }
    if prev.anchor_after.is_some() || next.anchor_before.is_some() {
        return None;
    }
    if prev.changes.len() != 1 || next.changes.len() != 1 {
        return None;
    }
    let a = &prev.changes[0];
    let b = &next.changes[0];

    let merged = match next.kind {
        TxKind::Insert => {
            if !a.inserted.is_empty() && a.inserted.contains('\n') {
                return None;
            }
            if b.inserted.contains('\n') {
                return None;
            }
            let expected = a.start + a.len_inserted();
            if b.start != expected || !b.removed.is_empty() {
                return None;
            }
            if prev.inserted_chars() + next.inserted_chars() > COALESCE_MAX_CHARS {
                return None;
            }
            Change {
                start: a.start,
                removed: String::new(),
                inserted: format!("{}{}", a.inserted, b.inserted),
            }
        }
        TxKind::DeleteBack => {
            // Deleting backwards: the new deletion ends where the previous began.
            if a.start != b.start + b.len_removed() || !a.inserted.is_empty() {
                return None;
            }
            if a.removed.contains('\n') || b.removed.contains('\n') {
                return None;
            }
            if prev.removed_chars() + next.removed_chars() > COALESCE_MAX_CHARS {
                return None;
            }
            Change {
                start: b.start,
                removed: format!("{}{}", b.removed, a.removed),
                inserted: String::new(),
            }
        }
        TxKind::DeleteForward => {
            if a.start != b.start || !a.inserted.is_empty() || !b.inserted.is_empty() {
                return None;
            }
            if a.removed.contains('\n') || b.removed.contains('\n') {
                return None;
            }
            if prev.removed_chars() + next.removed_chars() > COALESCE_MAX_CHARS {
                return None;
            }
            Change {
                start: a.start,
                removed: format!("{}{}", a.removed, b.removed),
                inserted: String::new(),
            }
        }
        TxKind::Other => return None,
    };

    Some(Transaction {
        changes: vec![merged],
        cursor_before: prev.cursor_before,
        anchor_before: prev.anchor_before,
        cursor_after: next.cursor_after,
        anchor_after: next.anchor_after,
        kind: prev.kind,
        time: next.time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_tx(start: usize, text: &str, kind: TxKind) -> Transaction {
        Transaction {
            changes: vec![Change {
                start,
                removed: String::new(),
                inserted: text.to_string(),
            }],
            cursor_before: start,
            anchor_before: None,
            cursor_after: start + text.chars().count(),
            anchor_after: None,
            kind,
            time: Instant::now(),
        }
    }

    #[test]
    fn coalesces_consecutive_typing() {
        let mut h = History::default();
        h.push(insert_tx(0, "a", TxKind::Insert));
        h.push(insert_tx(1, "b", TxKind::Insert));
        h.push(insert_tx(2, "c", TxKind::Insert));
        assert_eq!(h.depth(), 1);
        let tx = h.undo().unwrap().clone();
        assert_eq!(tx.changes[0].inserted, "abc");
    }

    #[test]
    fn newline_stops_coalescing() {
        let mut h = History::default();
        h.push(insert_tx(0, "a", TxKind::Insert));
        h.push(insert_tx(1, "\n", TxKind::Insert));
        assert_eq!(h.depth(), 2);
    }

    #[test]
    fn redo_stack_cleared_on_new_edit() {
        let mut h = History::default();
        h.push(insert_tx(0, "a", TxKind::Insert));
        h.push(insert_tx(1, "\n", TxKind::Insert));
        assert_eq!(h.depth(), 2);
        h.undo();
        assert!(h.can_redo());
        h.push(insert_tx(1, "z", TxKind::Insert));
        assert!(!h.can_redo());
        assert_eq!(h.depth(), 2);
    }

    #[test]
    fn dirty_tracks_save_point() {
        let mut h = History::default();
        assert!(!h.is_dirty());
        h.push(insert_tx(0, "a", TxKind::Insert));
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        h.push(insert_tx(1, "b", TxKind::Insert));
        assert!(h.is_dirty());
        h.undo();
        assert!(!h.is_dirty(), "undo back to the save point is clean");
    }

    #[test]
    fn save_point_is_not_merged_across() {
        let mut h = History::default();
        h.push(insert_tx(0, "a", TxKind::Insert));
        h.mark_saved();
        h.push(insert_tx(1, "b", TxKind::Insert));
        assert_eq!(h.depth(), 2);
    }

    #[test]
    fn trimming_keeps_indices_consistent() {
        let mut h = History::new(2, usize::MAX);
        for i in 0..5 {
            h.push(insert_tx(i, "\n", TxKind::Insert));
        }
        assert!(h.entries.len() <= 2);
        assert_eq!(h.index, h.entries.len());
    }
}
