//! Text storage abstraction.
//!
//! The editor never talks to a concrete text data structure directly: it talks
//! to [`TextStorage`]. `RopeStorage` (a [`ropey::Rope`]) is the only shipped
//! backend because it gives O(log n) insert/remove, O(1) snapshots and cheap
//! line lookups, but a gap-buffer or piece-table backend can be dropped in
//! without touching the editor core — see `docs/architecture.md`.
//!
//! All indices exposed by this trait are **character** indices (Unicode scalar
//! values), never bytes, never display columns.

use std::fmt::Debug;

use ropey::Rope;

use super::scan;

/// Storage interface used by [`super::Document`].
pub trait TextStorage: Debug + Send {
    /// Number of characters (Unicode scalar values).
    fn len_chars(&self) -> usize;

    /// Number of lines. A trailing newline yields a final empty line, matching
    /// how every mainstream editor models a file ending in `\n`.
    fn len_lines(&self) -> usize;

    /// Number of UTF-8 bytes.
    fn len_bytes(&self) -> usize;

    /// Character at `idx`, if in range.
    fn char_at(&self, idx: usize) -> Option<char>;

    /// Characters in `start..end`, clamped to the document bounds.
    fn slice_chars(&self, start: usize, end: usize) -> String;

    /// Characters of `line`, excluding the trailing line break.
    fn line_slice(&self, line: usize) -> String;

    /// Character index at which `line` starts.
    fn line_start(&self, line: usize) -> usize;

    /// Line index containing character `idx`.
    fn line_of_char(&self, idx: usize) -> usize;

    /// Insert `text` at character index `char_idx`.
    fn insert(&mut self, char_idx: usize, text: &str);

    /// Remove characters `start..end`.
    fn remove(&mut self, start: usize, end: usize);

    /// Whole document as an owned `String`.
    fn as_string(&self) -> String;

    /// All matches of `needle`, as ascending character offsets, capped at
    /// `limit`. Implementations are expected to be fast (memchr-class) for
    /// literal patterns.
    fn find_literal(&self, needle: &str, case_sensitive: bool, limit: usize) -> Vec<usize>;

    /// A cheap, independent snapshot for background work.
    ///
    /// Returns `None` when the backend cannot produce one; callers then fall
    /// back to running the work inline.
    fn snapshot(&self) -> Option<Snapshot>;
}

/// A cheap handle to a frozen copy of the document used by background jobs.
///
/// For the rope backend this is an `O(1)` `Arc` clone: ropey nodes are
/// reference counted, and the next edit only copies the nodes along the edited
/// path.
#[derive(Debug, Clone)]
pub enum Snapshot {
    Rope(Rope),
}

impl Snapshot {
    /// Literal search over the snapshot.
    pub fn find_literal(&self, needle: &str, case_sensitive: bool, limit: usize) -> Vec<usize> {
        match self {
            Snapshot::Rope(rope) => scan::find_literal(rope, needle, case_sensitive, limit),
        }
    }

    /// Whole snapshot as an owned string (used by the regex engine).
    pub fn to_string_lossy(&self) -> String {
        match self {
            Snapshot::Rope(rope) => rope.to_string(),
        }
    }
}

/// Rope backed [`TextStorage`].
#[derive(Debug, Clone, Default)]
pub struct RopeStorage {
    rope: Rope,
}

impl RopeStorage {
    pub fn new() -> Self {
        Self { rope: Rope::new() }
    }

    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    /// Read-only access to the underlying rope (used by benchmarks and tests).
    pub fn rope(&self) -> &Rope {
        &self.rope
    }
}

impl From<String> for RopeStorage {
    fn from(value: String) -> Self {
        Self {
            rope: Rope::from_str(&value),
        }
    }
}

impl TextStorage for RopeStorage {
    fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    fn char_at(&self, idx: usize) -> Option<char> {
        if idx >= self.rope.len_chars() {
            None
        } else {
            Some(self.rope.char(idx))
        }
    }

    fn slice_chars(&self, start: usize, end: usize) -> String {
        let len = self.rope.len_chars();
        let start = start.min(len);
        let end = end.clamp(start, len);
        if start == end {
            return String::new();
        }
        self.rope.slice(start..end).to_string()
    }

    fn line_slice(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        let slice = self.rope.line(line);
        let mut s = slice.to_string();
        // `Rope::line` includes the line break; strip CR/LF.
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    fn line_start(&self, line: usize) -> usize {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        self.rope.line_to_char(line)
    }

    fn line_of_char(&self, idx: usize) -> usize {
        let len = self.rope.len_chars();
        if len == 0 {
            return 0;
        }
        self.rope.char_to_line(idx.min(len - 1))
    }

    fn insert(&mut self, char_idx: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let idx = char_idx.min(self.rope.len_chars());
        self.rope.insert(idx, text);
    }

    fn remove(&mut self, start: usize, end: usize) {
        let len = self.rope.len_chars();
        let start = start.min(len);
        let end = end.clamp(start, len);
        if start == end {
            return;
        }
        self.rope.remove(start..end);
    }

    fn as_string(&self) -> String {
        self.rope.to_string()
    }

    fn find_literal(&self, needle: &str, case_sensitive: bool, limit: usize) -> Vec<usize> {
        scan::find_literal(&self.rope, needle, case_sensitive, limit)
    }

    fn snapshot(&self) -> Option<Snapshot> {
        Some(Snapshot::Rope(self.rope.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(text: &str) -> RopeStorage {
        RopeStorage::from_text(text)
    }

    #[test]
    fn len_lines_counts_trailing_newline() {
        assert_eq!(storage("a\nb\n").len_lines(), 3);
        assert_eq!(storage("a\nb").len_lines(), 2);
        assert_eq!(storage("").len_lines(), 1);
    }

    #[test]
    fn line_slice_strips_breaks() {
        let s = storage("alpha\r\nbeta\n");
        assert_eq!(s.line_slice(0), "alpha");
        assert_eq!(s.line_slice(1), "beta");
        assert_eq!(s.line_slice(2), "");
    }

    #[test]
    fn unicode_indices_are_chars() {
        let mut s = storage("héllo");
        assert_eq!(s.len_chars(), 5);
        s.insert(1, "🦀");
        assert_eq!(s.as_string(), "h🦀éllo");
        s.remove(1, 2);
        assert_eq!(s.as_string(), "héllo");
    }

    #[test]
    fn slice_clamps_out_of_range() {
        let s = storage("abc");
        assert_eq!(s.slice_chars(1, 99), "bc");
        assert_eq!(s.slice_chars(9, 99), "");
    }
}
