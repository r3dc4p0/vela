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

    /// Number of characters in `line`, excluding the trailing line break.
    ///
    /// Separate from `line_slice(line).chars().count()` because the answer is
    /// sometimes needed to decide *whether* to materialise the line at all: a
    /// single-line file can be gigabytes, and copying it to measure it is the
    /// cost the caller is trying to avoid.
    fn line_len(&self, line: usize) -> usize;

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

    /// A 128-bit fingerprint of `line`, excluding its line break.
    ///
    /// The diff engine compares fingerprints instead of text, so the cost of a
    /// comparison is one integer rather than a string walk — and this method is
    /// where the allocation goes away: an implementation can hash the line in
    /// place, which is the difference between one allocation per line and none
    /// on a file with a million of them.
    ///
    /// 128 bits rather than 64 because a collision would silently make two
    /// different lines compare equal, and a wrong diff is worse than a slow
    /// one. At 128 bits the probability is not a risk worth a verification
    /// pass.
    fn line_hash(&self, line: usize) -> u128;

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

    /// Character at `idx`, or `None` past the end.
    pub fn char_at(&self, idx: usize) -> Option<char> {
        match self {
            Snapshot::Rope(rope) => rope.get_char(idx),
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

    fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        let len = slice.len_chars();
        // `Rope::line` includes the line break; strip it, counting CRLF as one.
        if len > 0 && slice.char(len - 1) == '\n' {
            let without_lf = len - 1;
            if without_lf > 0 && slice.char(without_lf - 1) == '\r' {
                without_lf - 1
            } else {
                without_lf
            }
        } else {
            len
        }
    }

    fn line_hash(&self, line: usize) -> u128 {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        // The line break is not part of the line: `abc\n` and a final `abc`
        // without one are the same text, and a diff that called them different
        // would put a change marker on the last line of every file that ends
        // with a newline.
        let total = slice.len_bytes();
        let mut tail = 0usize;
        if total > 0 && slice.byte(total - 1) == b'\n' {
            tail += 1;
            if total > 1 && slice.byte(total - 2) == b'\r' {
                tail += 1;
            }
        }
        let mut remaining = total - tail;

        // FNV-1a and a rotate/multiply mix, so a collision has to fool two
        // different functions at once.
        let mut a: u64 = 0xcbf2_9ce4_8422_2325;
        let mut b: u64 = 0x9e37_79b9_7f4a_7c15;
        for chunk in slice.chunks() {
            let bytes = chunk.as_bytes();
            let take = remaining.min(bytes.len());
            for &byte in &bytes[..take] {
                a = (a ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
                b = b.rotate_left(5) ^ (byte as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
            }
            remaining -= take;
            if remaining == 0 {
                break;
            }
        }
        ((a as u128) << 64) | b as u128
    }

    fn line_start(&self, line: usize) -> usize {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        self.rope.line_to_char(line)
    }

    fn line_of_char(&self, idx: usize) -> usize {
        let len = self.rope.len_chars();
        // A position at or past the end belongs to the last line — which is the
        // empty line after a trailing newline. Clamping to `len - 1` instead
        // would report a cursor at EOF as sitting on the previous line, so the
        // caret would blink there while typing went to the new one.
        if idx >= len {
            return self.rope.len_lines().saturating_sub(1);
        }
        self.rope.char_to_line(idx)
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
    fn line_of_char_sees_the_final_empty_line() {
        let s = storage("a\nb\n");
        assert_eq!(s.line_of_char(0), 0);
        assert_eq!(s.line_of_char(1), 0, "the newline closes line 0");
        assert_eq!(s.line_of_char(2), 1);
        assert_eq!(s.line_of_char(3), 1, "the last newline closes line 1");
        assert_eq!(
            s.line_of_char(4),
            2,
            "one past the end is the empty line after the trailing newline"
        );
    }

    #[test]
    fn line_of_char_clamps_past_the_end() {
        assert_eq!(storage("abc").line_of_char(3), 0, "no trailing newline");
        assert_eq!(storage("abc").line_of_char(99), 0);
        assert_eq!(storage("").line_of_char(0), 0);
        assert_eq!(storage("").line_of_char(7), 0);
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
