//! Search and replace.
//!
//! Literal search is the fast path and is delegated to the storage backend
//! (memchr over rope chunks). Regular expressions use the `regex` crate over a
//! document snapshot, which is slower and only built when the user actually
//! enables regex mode.
//!
//! Responsiveness strategy:
//!
//! * the query is only re-evaluated when it changes, not on every frame;
//! * results are capped ([`MAX_MATCHES`]) so a query like `e` in a 100 MiB file
//!   cannot blow up memory or time;
//! * next/previous navigation reuses the cached match list instead of
//!   rescanning.

use std::ops::Range;

use regex::RegexBuilder;

use crate::buffer::{Document, TextStorage};
use crate::error::{Error, Result};

/// Upper bound on collected matches.
pub const MAX_MATCHES: usize = 20_000;

/// A search query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Query {
    pub fn literal(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Toggle that affects the result set; used to decide when to re-scan.
    pub fn options(&self) -> (bool, bool, bool) {
        (self.case_sensitive, self.whole_word, self.regex)
    }
}

/// A completed search.
#[derive(Debug, Clone, Default)]
pub struct MatchSet {
    pub ranges: Vec<Range<usize>>,
    /// Index into `ranges` of the focused match.
    pub current: Option<usize>,
    /// True when the scan stopped at [`MAX_MATCHES`].
    pub truncated: bool,
    /// Query error (invalid regex) to display instead of results.
    pub error: Option<String>,
}

impl MatchSet {
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn current_range(&self) -> Option<Range<usize>> {
        self.current.and_then(|i| self.ranges.get(i).cloned())
    }

    /// Human readable counter for the status bar.
    pub fn counter(&self) -> String {
        if let Some(err) = &self.error {
            return err.clone();
        }
        if self.ranges.is_empty() {
            return "no matches".to_string();
        }
        let pos = self.current.map(|i| i + 1).unwrap_or(0);
        if self.truncated {
            format!("{pos} of {}+", self.ranges.len())
        } else {
            format!("{pos} of {}", self.ranges.len())
        }
    }
}

/// Find every match of `query` in `storage`.
pub fn find_all(
    storage: &dyn TextStorage,
    query: &Query,
    limit: usize,
) -> Result<Vec<Range<usize>>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let ranges = if query.regex {
        find_regex(storage, query, limit)?
    } else {
        let starts =
            storage.find_literal(&query.text, query.case_sensitive, limit.saturating_mul(4));
        let len = query.text.chars().count();
        starts.into_iter().map(|s| s..s + len).take(limit).collect()
    };

    if !query.whole_word {
        return Ok(ranges);
    }
    Ok(ranges
        .into_iter()
        .filter(|r| is_whole_word(storage, r))
        .collect())
}

fn is_whole_word(storage: &dyn TextStorage, range: &Range<usize>) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if range.start > 0 {
        if let Some(prev) = storage.char_at(range.start - 1) {
            if is_word(prev) {
                return false;
            }
        }
    }
    if let Some(next) = storage.char_at(range.end) {
        if is_word(next) {
            return false;
        }
    }
    true
}

fn find_regex(storage: &dyn TextStorage, query: &Query, limit: usize) -> Result<Vec<Range<usize>>> {
    let re = RegexBuilder::new(&query.text)
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .build()
        .map_err(|e| Error::Regex(first_line(&e.to_string())))?;

    let text = storage.as_string();
    // Byte offsets from `regex` must become character offsets. One pass over the
    // string keeps this linear even for thousands of matches.
    let mut out = Vec::new();
    let mut byte_cursor = 0usize;
    let mut char_cursor = 0usize;
    for m in re.find_iter(&text) {
        if out.len() >= limit {
            break;
        }
        // Advance the character cursor to the match start.
        while byte_cursor < m.start() {
            byte_cursor += char_len_at(&text, byte_cursor);
            char_cursor += 1;
        }
        let start = char_cursor;
        let mut end_char = char_cursor;
        let mut b = m.start();
        while b < m.end() {
            b += char_len_at(&text, b);
            end_char += 1;
        }
        if end_char > start {
            out.push(start..end_char);
        }
    }
    Ok(out)
}

fn char_len_at(text: &str, byte: usize) -> usize {
    text[byte..]
        .chars()
        .next()
        .map(|c| c.len_utf8())
        .unwrap_or(1)
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).to_string()
}

/// Expand `$1` / `$name` style backreferences in a replacement string.
///
/// Literal mode treats the replacement verbatim; regex mode expands captures.
pub fn expand_replacement(
    replacement: &str,
    haystack: &str,
    range: &Range<usize>,
    regex: Option<&str>,
) -> String {
    if regex.is_none() || !replacement.contains('$') {
        return replacement.to_string();
    }
    // Re-run the pattern against the matched slice to resolve captures.
    let Some(pattern) = regex else {
        return replacement.to_string();
    };
    let Ok(re) = RegexBuilder::new(pattern).multi_line(true).build() else {
        return replacement.to_string();
    };
    let slice: String = haystack
        .chars()
        .skip(range.start)
        .take(range.end - range.start)
        .collect();
    let Some(caps) = re.captures(&slice) else {
        return replacement.to_string();
    };
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // `$$` is a literal dollar.
        if chars.peek() == Some(&'$') {
            chars.next();
            out.push('$');
            continue;
        }
        let mut name = String::new();
        while let Some(&n) = chars.peek() {
            if n.is_alphanumeric() || n == '_' {
                name.push(n);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            out.push('$');
            continue;
        }
        let value = if let Ok(idx) = name.parse::<usize>() {
            caps.get(idx).map(|m| m.as_str()).unwrap_or("")
        } else {
            caps.name(&name).map(|m| m.as_str()).unwrap_or("")
        };
        out.push_str(value);
    }
    out
}

/// Live search state owned by the application.
#[derive(Debug, Default)]
pub struct SearchState {
    pub query: Query,
    pub matches: MatchSet,
    /// Document revision the matches were computed from.
    scanned_revision: u64,
    /// Query the matches were computed from.
    scanned_query: Query,
    pub active: bool,
    /// Cursor position when the search session started.
    ///
    /// Incremental search must not use the live cursor to pick the "current"
    /// match: selecting a match moves the cursor to its end, so the next
    /// keystroke would then pick the *following* match and the view would
    /// creep forward while typing. Anchoring on the origin keeps the highlight
    /// stable until the user navigates explicitly.
    pub origin: usize,
}

impl SearchState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute matches when the document or the query changed.
    ///
    /// Returns `true` when a rescan happened.
    pub fn refresh(&mut self, doc: &Document) -> bool {
        if !self.active && self.query.is_empty() {
            return false;
        }
        let unchanged = self.scanned_revision == doc.revision()
            && self.scanned_query == self.query
            && !self.matches.ranges.is_empty();
        if unchanged {
            return false;
        }
        self.rescan(doc);
        true
    }

    /// Force a rescan.
    pub fn rescan(&mut self, doc: &Document) {
        self.scanned_revision = doc.revision();
        self.scanned_query = self.query.clone();

        if self.query.is_empty() {
            self.matches = MatchSet::default();
            return;
        }

        match find_all(doc.storage(), &self.query, MAX_MATCHES) {
            Ok(ranges) => {
                let truncated = ranges.len() >= MAX_MATCHES;
                self.matches = MatchSet {
                    ranges,
                    current: None,
                    truncated,
                    error: None,
                };
                self.select_nearest(doc.cursor());
            }
            Err(e) => {
                self.matches = MatchSet {
                    ranges: Vec::new(),
                    current: None,
                    truncated: false,
                    error: Some(e.to_string()),
                };
            }
        }
    }

    /// Focus the first match at or after `cursor`, wrapping around.
    pub fn select_nearest(&mut self, cursor: usize) {
        if self.matches.ranges.is_empty() {
            self.matches.current = None;
            return;
        }
        let idx = self
            .matches
            .ranges
            .iter()
            .position(|r| r.start >= cursor)
            .unwrap_or(0);
        self.matches.current = Some(idx);
    }

    /// Advance to the next match, wrapping.
    pub fn next_match(&mut self) -> Option<Range<usize>> {
        let len = self.matches.ranges.len();
        if len == 0 {
            return None;
        }
        let idx = match self.matches.current {
            Some(i) => (i + 1) % len,
            None => 0,
        };
        self.matches.current = Some(idx);
        self.matches.ranges.get(idx).cloned()
    }

    /// Go to the previous match, wrapping.
    pub fn prev_match(&mut self) -> Option<Range<usize>> {
        let len = self.matches.ranges.len();
        if len == 0 {
            return None;
        }
        let idx = match self.matches.current {
            Some(0) | None => len - 1,
            Some(i) => i - 1,
        };
        self.matches.current = Some(idx);
        self.matches.ranges.get(idx).cloned()
    }

    pub fn clear(&mut self) {
        self.query = Query::default();
        self.matches = MatchSet::default();
        self.scanned_query = Query::default();
        self.scanned_revision = 0;
        self.origin = 0;
    }

    /// Current match range, if any.
    pub fn current(&self) -> Option<Range<usize>> {
        self.matches.current_range()
    }

    /// True when `pos` falls inside a match (used to highlight).
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.matches.ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Document;

    fn doc(text: &str) -> Document {
        Document::from_text(text, None)
    }

    #[test]
    fn finds_literal_matches() {
        let d = doc("one two one two");
        let q = Query::literal("one");
        let r = find_all(d.storage(), &q, 100).unwrap();
        assert_eq!(r, vec![0..3, 8..11]);
    }

    #[test]
    fn case_insensitive_search() {
        let d = doc("One ONE one");
        let mut q = Query::literal("one");
        q.case_sensitive = false;
        assert_eq!(find_all(d.storage(), &q, 100).unwrap().len(), 3);
        q.case_sensitive = true;
        assert_eq!(find_all(d.storage(), &q, 100).unwrap().len(), 1);
    }

    #[test]
    fn whole_word_filters_partial_matches() {
        let d = doc("foo foobar barfoo foo");
        let mut q = Query::literal("foo");
        q.whole_word = true;
        assert_eq!(find_all(d.storage(), &q, 100).unwrap(), vec![0..3, 18..21]);
    }

    #[test]
    fn regex_search_and_captures() {
        let d = doc("a1 b22 c333");
        let mut q = Query::literal(r"(\d+)");
        q.regex = true;
        let ranges = find_all(d.storage(), &q, 100).unwrap();
        assert_eq!(ranges, vec![1..2, 4..6, 8..11]);

        let text = d.text();
        let expanded = expand_replacement("<$1>", &text, &ranges[2], Some(r"(\d+)"));
        assert_eq!(expanded, "<333>");
    }

    #[test]
    fn invalid_regex_reports_error() {
        let d = doc("x");
        let mut q = Query::literal("(");
        q.regex = true;
        assert!(find_all(d.storage(), &q, 100).is_err());
    }

    #[test]
    fn unicode_offsets_are_chars() {
        let d = doc("héllo héllo");
        let q = Query::literal("héllo");
        assert_eq!(find_all(d.storage(), &q, 100).unwrap(), vec![0..5, 6..11]);
    }

    #[test]
    fn unicode_matches_do_not_overlap() {
        // A non-ASCII query takes the character-window scanner rather than the
        // byte scanner, and that one used to step forward by a single character
        // after a hit: "😀😀" in "😀😀😀" reported 0..2 *and* 1..3, which
        // double-counted in the status bar and gave replace-all overlapping
        // edits to apply.
        let d = doc("😀😀😀");
        let q = Query::literal("😀😀");
        assert_eq!(find_all(d.storage(), &q, 100).unwrap(), vec![0..2]);
    }

    #[test]
    fn ascii_matches_do_not_overlap_either() {
        let d = doc("abab");
        let q = Query::literal("ab");
        assert_eq!(find_all(d.storage(), &q, 100).unwrap(), vec![0..2, 2..4]);
    }

    #[test]
    fn navigation_wraps() {
        let mut d = doc("a a a");
        let mut s = SearchState::new();
        s.query = Query::literal("a");
        s.active = true;
        s.rescan(&d);
        assert_eq!(s.matches.len(), 3);
        s.select_nearest(0);
        assert_eq!(s.current(), Some(0..1));
        assert_eq!(s.next_match(), Some(2..3));
        assert_eq!(s.next_match(), Some(4..5));
        assert_eq!(s.next_match(), Some(0..1), "wraps to the first match");
        assert_eq!(s.prev_match(), Some(4..5), "wraps backwards");
        let _ = &mut d;
    }

    #[test]
    fn refresh_skips_unchanged_document() {
        let d = doc("abc abc");
        let mut s = SearchState::new();
        s.query = Query::literal("abc");
        s.active = true;
        assert!(s.refresh(&d));
        assert!(!s.refresh(&d), "no rescan when nothing changed");
    }

    #[test]
    fn refresh_reacts_to_edits() {
        let mut d = doc("abc");
        let mut s = SearchState::new();
        s.query = Query::literal("abc");
        s.active = true;
        assert!(s.refresh(&d));
        assert_eq!(s.matches.len(), 1);
        d.set_cursor(3);
        d.insert_text(" abc");
        assert!(s.refresh(&d));
        assert_eq!(s.matches.len(), 2);
    }

    #[test]
    fn dollar_escape_in_replacement() {
        assert_eq!(expand_replacement("$$5", "", &(0..0), Some("x")), "$$5");
        assert_eq!(
            expand_replacement("cost: $$5", "a", &(0..1), None),
            "cost: $$5"
        );
    }
}
