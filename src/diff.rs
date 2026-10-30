//! Line diff between two documents.
//!
//! The output is a **row list**, not a list of hunks: every row says which line
//! of the left document and which line of the right one belong side by side, so
//! a split view can draw it without asking a second question. A row with no
//! left line is an insertion, one with no right line a deletion, and a row with
//! both that differs is a change.
//!
//! Three decisions carry the performance, and they are worth stating because
//! the obvious implementation is quadratic:
//!
//! - **Fingerprints, not text.** Each line is reduced to a 128-bit hash once
//!   ([`TextStorage::line_hash`]), so a comparison is an integer compare. That
//!   also removes the allocation a `line_slice` per line would cost.
//! - **Common prefix and suffix are trimmed first.** Two versions of a file
//!   usually agree for hundreds of lines at each end, and trimming them is
//!   linear and removes almost all of the work.
//! - **Unique lines are anchors.** What is left is split at lines that appear
//!   exactly once on both sides, keeping the longest run of them that is in the
//!   same order (patience diff). Real edits — a function added, a block moved —
//!   have plenty of such lines, and each anchor cuts the remaining work into
//!   two smaller pieces. A region with no unique line is emitted directly: pairs
//!   of lines side by side, then whatever is left over.
//!
//! The result is `O(n log n)` with linear memory and no worst case that blows
//! up. The price is that the edit script is not guaranteed minimal — a moved
//! block reads as a deletion plus an insertion rather than as a move, which is
//! what every side-by-side view shows anyway.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::buffer::storage::TextStorage;

/// Sentinel for "this row has no line on this side".
pub const NONE: u32 = u32::MAX;

/// Refuse to diff more than this much text.
///
/// Hashing is linear, so the limit is about how long a keystroke may be blocked
/// rather than about correctness: 64 MiB of text hashes in well under a second
/// on the machines this runs on, and a diff view of anything larger is not
/// readable anyway.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// Refuse to build more rows than this.
///
/// One row is 12 bytes, so a million rows is 12 MiB — the point at which the
/// row list itself starts to be a memory story.
pub const MAX_ROWS: usize = 1_000_000;

/// Depth limit for the anchor recursion.
///
/// Anchors usually halve the region, so the depth is logarithmic; the limit is
/// there for the adversarial case (every split removing one line) so a hostile
/// input cannot overflow the stack.
const MAX_DEPTH: usize = 32;

/// A hasher that does nothing, for maps whose keys are already hashes.
///
/// The anchor pass is the one place a diff of a large file spends real time:
/// two maps of one entry per line, and the default SipHash over a key that is
/// itself a hash is pure cost. Feeding the key through unchanged measured
/// roughly twice as fast on a 400 000-line file.
#[derive(Default)]
struct PassThrough(u64);

impl Hasher for PassThrough {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // Only `u64` keys are used, and `write_u64` is what the map calls for
        // them; this is here because the trait demands it.
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ byte as u64;
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

/// A map keyed by a line fingerprint.
type Seen = HashMap<u64, (u32, usize), BuildHasherDefault<PassThrough>>;

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The two lines are the same, or both sides are absent.
    Same,
    /// Both lines are present and differ.
    Changed,
    /// Only the left line is present.
    Removed,
    /// Only the right line is present.
    Added,
}

impl Kind {
    pub fn is_change(self) -> bool {
        !matches!(self, Kind::Same)
    }
}

/// One row of the side-by-side view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub left: u32,
    pub right: u32,
    pub kind: Kind,
}

impl Row {
    pub fn left_line(&self) -> Option<usize> {
        (self.left != NONE).then_some(self.left as usize)
    }

    pub fn right_line(&self) -> Option<usize> {
        (self.right != NONE).then_some(self.right as usize)
    }
}

/// Counts for the header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    /// Maximal runs of changed rows — what "next change" steps through.
    pub blocks: usize,
}

impl Stats {
    fn of(rows: &[Row]) -> Self {
        let mut stats = Stats::default();
        let mut in_block = false;
        for row in rows {
            match row.kind {
                Kind::Same => in_block = false,
                Kind::Changed => {
                    stats.changed += 1;
                    if !in_block {
                        stats.blocks += 1;
                    }
                    in_block = true;
                }
                Kind::Removed => {
                    stats.removed += 1;
                    if !in_block {
                        stats.blocks += 1;
                    }
                    in_block = true;
                }
                Kind::Added => {
                    stats.added += 1;
                    if !in_block {
                        stats.blocks += 1;
                    }
                    in_block = true;
                }
            }
        }
        stats
    }

    /// True when the two documents are the same.
    pub fn is_same(&self) -> bool {
        self.added == 0 && self.removed == 0 && self.changed == 0
    }
}

/// The aligned rows and their counts.
#[derive(Debug, Clone)]
pub struct Diff {
    pub rows: Vec<Row>,
    pub stats: Stats,
}

impl Diff {
    /// The first row that belongs to a change, if any.
    pub fn first_change(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.kind.is_change())
    }

    /// The first row of every change block, in order.
    pub fn block_starts(&self) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut in_block = false;
        for (i, row) in self.rows.iter().enumerate() {
            if row.kind.is_change() {
                if !in_block {
                    starts.push(i);
                }
                in_block = true;
            } else {
                in_block = false;
            }
        }
        starts
    }

    /// The row a line of the left document sits on.
    ///
    /// Rows are ordered by both sides, so this is a binary search rather than a
    /// scan — it is what lets the view keep two documents in step.
    pub fn row_of_left(&self, line: usize) -> Option<usize> {
        let target = line as u32;
        self.rows
            .binary_search_by(|row| match row.left {
                NONE => std::cmp::Ordering::Greater,
                l => l.cmp(&target),
            })
            .ok()
    }

    /// The row a line of the right document sits on.
    pub fn row_of_right(&self, line: usize) -> Option<usize> {
        let target = line as u32;
        self.rows
            .binary_search_by(|row| match row.right {
                NONE => std::cmp::Ordering::Greater,
                r => r.cmp(&target),
            })
            .ok()
    }
}

/// Lines that carry content, which is what a diff compares.
///
/// The editor models a file ending in a newline as having a final empty line —
/// that is where the caret goes when you press `Ctrl+End` — but that line is
/// not *content*: comparing it would make `abc` and `abc\n` differ, and would
/// draw a phantom last row on every file in the world. An empty document has no
/// lines at all, for the same reason.
pub fn content_lines(storage: &dyn TextStorage) -> usize {
    if storage.len_chars() == 0 {
        return 0;
    }
    let lines = storage.len_lines();
    if storage.char_at(storage.len_chars() - 1) == Some('\n') {
        lines.saturating_sub(1)
    } else {
        lines
    }
}

/// Compare two documents, or return `None` when they are too big to diff.
pub fn compare(left: &dyn TextStorage, right: &dyn TextStorage) -> Option<Diff> {
    if left.len_bytes() + right.len_bytes() > MAX_BYTES {
        return None;
    }
    let (lc, rc) = (content_lines(left), content_lines(right));
    if lc + rc > MAX_ROWS {
        return None;
    }

    let a: Vec<u128> = (0..lc).map(|i| left.line_hash(i)).collect();
    let b: Vec<u128> = (0..rc).map(|i| right.line_hash(i)).collect();

    let mut rows = Vec::with_capacity(lc.max(rc));
    align(&a, &b, 0, a.len(), 0, b.len(), &mut rows, 0);
    let stats = Stats::of(&rows);
    Some(Diff { rows, stats })
}

/// Align `a[a0..a1]` with `b[b0..b1]`, appending rows in order.
#[allow(clippy::too_many_arguments)]
fn align(
    a: &[u128],
    b: &[u128],
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
    out: &mut Vec<Row>,
    depth: usize,
) {
    let mut i = a0;
    let mut j = b0;
    // The shared prefix: same lines, same order, nothing to decide.
    while i < a1 && j < b1 && a[i] == b[j] {
        out.push(Row {
            left: i as u32,
            right: j as u32,
            kind: Kind::Same,
        });
        i += 1;
        j += 1;
    }
    // The shared suffix, counted now and emitted last so the rows stay in
    // document order.
    let mut ia = a1;
    let mut jb = b1;
    while ia > i && jb > j && a[ia - 1] == b[jb - 1] {
        ia -= 1;
        jb -= 1;
    }

    if i == ia {
        for jj in j..jb {
            out.push(Row {
                left: NONE,
                right: jj as u32,
                kind: Kind::Added,
            });
        }
    } else if j == jb {
        for ii in i..ia {
            out.push(Row {
                left: ii as u32,
                right: NONE,
                kind: Kind::Removed,
            });
        }
    } else if depth >= MAX_DEPTH {
        emit_gap(a, b, i, ia, j, jb, out);
    } else {
        let found = anchors(&a[i..ia], &b[j..jb]);
        if found.is_empty() {
            emit_gap(a, b, i, ia, j, jb, out);
        } else {
            let (mut pa, mut pb) = (i, j);
            for (x, y) in found {
                let (x, y) = (i + x, j + y);
                align(a, b, pa, x, pb, y, out, depth + 1);
                out.push(Row {
                    left: x as u32,
                    right: y as u32,
                    kind: Kind::Same,
                });
                pa = x + 1;
                pb = y + 1;
            }
            align(a, b, pa, ia, pb, jb, out, depth + 1);
        }
    }

    for k in 0..(a1 - ia) {
        out.push(Row {
            left: (ia + k) as u32,
            right: (jb + k) as u32,
            kind: Kind::Same,
        });
    }
}

/// Emit a region with no useful anchors: pair the lines up, then whatever is
/// left over.
///
/// Pairing is what a reader expects — a replaced block shows as rows of
/// changed lines rather than as every old line followed by every new one — and
/// the equality check inside keeps a region that merely *looks* unrelated (the
/// same lines in a different order, duplicated lines with no unique anchor)
/// from being marked changed where it is not.
fn emit_gap(
    a: &[u128],
    b: &[u128],
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
    out: &mut Vec<Row>,
) {
    let n = (a1 - a0).min(b1 - b0);
    for k in 0..n {
        let same = a[a0 + k] == b[b0 + k];
        out.push(Row {
            left: (a0 + k) as u32,
            right: (b0 + k) as u32,
            kind: if same { Kind::Same } else { Kind::Changed },
        });
    }
    for k in n..(a1 - a0) {
        out.push(Row {
            left: (a0 + k) as u32,
            right: NONE,
            kind: Kind::Removed,
        });
    }
    for k in n..(b1 - b0) {
        out.push(Row {
            left: NONE,
            right: (b0 + k) as u32,
            kind: Kind::Added,
        });
    }
}

/// Lines that occur exactly once on both sides, reduced to the longest run that
/// is in the same order on both.
///
/// Returned as offsets into the two slices.
fn anchors(a: &[u128], b: &[u128]) -> Vec<(usize, usize)> {
    if a.len() < 2 || b.len() < 2 {
        return Vec::new();
    }
    let mut in_a: Seen = Seen::with_capacity_and_hasher(a.len(), BuildHasherDefault::default());
    for (i, &h) in a.iter().enumerate() {
        let entry = in_a.entry(h as u64).or_insert((0, i));
        entry.0 = entry.0.saturating_add(1);
    }
    let mut in_b: Seen = Seen::with_capacity_and_hasher(b.len(), BuildHasherDefault::default());
    for (j, &h) in b.iter().enumerate() {
        let entry = in_b.entry(h as u64).or_insert((0, j));
        entry.0 = entry.0.saturating_add(1);
    }

    // Candidates come out ordered by their position in `a`, which is what the
    // LIS below needs.
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    for (i, &h) in a.iter().enumerate() {
        let key = h as u64;
        if in_a[&key].0 != 1 {
            continue;
        }
        if let Some(&(1, j)) = in_b.get(&key) {
            candidates.push((i, j));
        }
    }
    longest_increasing(&candidates)
}

/// Longest subsequence of `candidates` whose right-hand offsets increase.
fn longest_increasing(candidates: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    // Patience sorting: `tails[k]` is the index of the smallest tail of a run
    // of length k+1, and `prev` rebuilds the chain.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<isize> = vec![-1; candidates.len()];
    for (k, &(_, j)) in candidates.iter().enumerate() {
        let at = tails.partition_point(|&t| candidates[t].1 < j);
        if at > 0 {
            prev[k] = tails[at - 1] as isize;
        }
        if at == tails.len() {
            tails.push(k);
        } else {
            tails[at] = k;
        }
    }

    let mut out = Vec::with_capacity(tails.len());
    let mut cursor = *tails.last().expect("non-empty") as isize;
    while cursor >= 0 {
        out.push(candidates[cursor as usize]);
        cursor = prev[cursor as usize];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::storage::RopeStorage;

    fn diff(left: &str, right: &str) -> Diff {
        let a = RopeStorage::from_text(left);
        let b = RopeStorage::from_text(right);
        compare(&a, &b).expect("small enough")
    }

    /// Every line of each side appears exactly once, in order.
    ///
    /// This is the invariant the view depends on: a line that appears twice
    /// would be drawn twice, and one that is missing would be invisible.
    fn check(rows: &[Row], left: usize, right: usize) {
        let mut next_left = 0usize;
        let mut next_right = 0usize;
        for row in rows {
            if let Some(l) = row.left_line() {
                assert_eq!(l, next_left, "left line {l} out of order or repeated");
                next_left += 1;
            }
            if let Some(r) = row.right_line() {
                assert_eq!(r, next_right, "right line {r} out of order or repeated");
                next_right += 1;
            }
        }
        assert_eq!(next_left, left, "every left line is accounted for");
        assert_eq!(next_right, right, "every right line is accounted for");
    }

    fn kinds(d: &Diff) -> String {
        d.rows
            .iter()
            .map(|r| match r.kind {
                Kind::Same => '.',
                Kind::Changed => '~',
                Kind::Removed => '-',
                Kind::Added => '+',
            })
            .collect()
    }

    #[test]
    fn identical_documents_have_no_changes() {
        let d = diff("one\ntwo\nthree\n", "one\ntwo\nthree\n");
        assert_eq!(kinds(&d), "...");
        assert!(d.stats.is_same());
        assert_eq!(d.rows.len(), 3);
        check(&d.rows, 3, 3);
    }

    #[test]
    fn a_missing_final_newline_is_not_a_change() {
        // `abc` and `abc\n` are the same line of text. A diff that said
        // otherwise would mark the last line of half the files on disk.
        let d = diff("abc\n", "abc");
        assert!(d.stats.is_same(), "{}", kinds(&d));
    }

    #[test]
    fn an_inserted_line_pushes_the_rest_down() {
        let d = diff("a\nb\nc\n", "a\nx\nb\nc\n");
        assert_eq!(kinds(&d), ".+..");
        assert_eq!(d.stats.added, 1);
        assert_eq!(d.stats.blocks, 1);
        check(&d.rows, 3, 4);
        // The inserted row has no left line, so `b` still sits beside `b`.
        assert_eq!(d.rows[1].left_line(), None);
        assert_eq!(d.rows[2].left_line(), Some(1));
        assert_eq!(d.rows[2].right_line(), Some(2));
    }

    #[test]
    fn a_deleted_line_pulls_the_rest_up() {
        let d = diff("a\nx\nb\n", "a\nb\n");
        assert_eq!(kinds(&d), ".-.");
        assert_eq!(d.stats.removed, 1);
        check(&d.rows, 3, 2);
    }

    #[test]
    fn a_rewritten_line_is_one_changed_row() {
        let d = diff("a\nold\nc\n", "a\nnew\nc\n");
        assert_eq!(kinds(&d), ".~.");
        assert_eq!(d.stats.changed, 1);
        assert_eq!(d.stats.blocks, 1);
        check(&d.rows, 3, 3);
    }

    #[test]
    fn a_rewritten_block_is_rows_of_changed_lines() {
        // Pairing, not "all removals then all additions": a reader compares the
        // two versions line by line.
        let d = diff("a\n1\n2\n3\nz\n", "a\none\ntwo\nz\n");
        assert_eq!(kinds(&d), ".~~-.", "three old lines, two new");
        check(&d.rows, 5, 4);
    }

    #[test]
    fn an_empty_side_is_all_additions_or_all_removals() {
        let d = diff("", "a\nb\n");
        assert_eq!(kinds(&d), "++");
        assert_eq!(d.stats.added, 2);
        check(&d.rows, 0, 2);

        let d = diff("a\nb\n", "");
        assert_eq!(kinds(&d), "--");
        assert_eq!(d.stats.removed, 2);
        check(&d.rows, 2, 0);
    }

    #[test]
    fn both_empty_is_no_rows() {
        let d = diff("", "");
        assert!(d.rows.is_empty());
        assert!(d.stats.is_same());
    }

    #[test]
    fn nothing_in_common_does_not_hang() {
        let left: String = (0..500).map(|i| format!("left {i}\n")).collect();
        let right: String = (0..500).map(|i| format!("right {i}\n")).collect();
        let d = diff(&left, &right);
        check(&d.rows, 500, 500);
        assert_eq!(d.stats.changed, 500);
        assert_eq!(d.stats.blocks, 1);
    }

    #[test]
    fn duplicated_lines_have_no_anchor_and_still_align() {
        // No line is unique, so the anchor pass finds nothing and the region is
        // emitted directly — and the equality check inside keeps the shared
        // lines from being marked changed.
        let left = "}\n".repeat(40);
        let right = "}\n".repeat(40);
        let d = diff(&left, &right);
        assert!(d.stats.is_same(), "{}", kinds(&d));
        check(&d.rows, 40, 40);
    }

    #[test]
    fn a_large_file_with_one_edit_is_cheap_and_exact() {
        // Big enough to take the anchor path rather than the direct one.
        let mut left = String::new();
        for i in 0..4000 {
            left.push_str(&format!("line {i}\n"));
        }
        let right = left.replace("line 2000\n", "line 2000 changed\n");
        let d = diff(&left, &right);
        assert_eq!(d.stats.changed, 1, "{}", kinds(&d));
        assert_eq!(d.stats.blocks, 1);
        check(&d.rows, 4000, 4000);
    }

    #[test]
    fn a_block_inserted_in_the_middle_stays_local() {
        let mut left = String::new();
        for i in 0..3000 {
            left.push_str(&format!("fn f{i}() {{}}\n"));
        }
        let mut right = String::new();
        for i in 0..3000 {
            right.push_str(&format!("fn f{i}() {{}}\n"));
            if i == 1500 {
                right.push_str("fn inserted() {}\n");
            }
        }
        let d = diff(&left, &right);
        assert_eq!(d.stats.added, 1);
        assert_eq!(d.stats.blocks, 1);
        check(&d.rows, 3000, 3001);
        // The insertion sits right after line 1500, not at the end.
        let row = d.row_of_right(1501).expect("the new line has a row");
        assert_eq!(d.rows[row].kind, Kind::Added);
    }

    #[test]
    fn rows_can_be_looked_up_by_line() {
        let d = diff("a\nb\nc\n", "a\nx\nc\n");
        assert_eq!(d.row_of_left(2), Some(2));
        assert_eq!(d.row_of_right(2), Some(2));
        assert_eq!(d.row_of_left(9), None);
        assert_eq!(d.first_change(), Some(1));
        assert_eq!(d.block_starts(), vec![1]);
    }

    #[test]
    fn the_lis_keeps_the_order_and_drops_the_crossings() {
        // (0,3) crosses everything after it; the longest run in order is the
        // three that ascend.
        let cand = vec![(0, 3), (1, 0), (2, 1), (3, 4)];
        assert_eq!(longest_increasing(&cand), vec![(1, 0), (2, 1), (3, 4)]);

        // A single crossing in the middle: (2,1) cannot join, so the run skips
        // it and keeps going.
        let cand = vec![(0, 0), (1, 5), (2, 1), (3, 6)];
        assert_eq!(longest_increasing(&cand), vec![(0, 0), (2, 1), (3, 6)]);

        assert!(longest_increasing(&[]).is_empty());
    }

    #[test]
    fn too_much_text_is_refused_rather_than_diffed() {
        // A sparse stand-in: the cap is on bytes, and the check runs before any
        // hashing.
        #[derive(Debug)]
        struct Big(usize);
        impl TextStorage for Big {
            fn len_chars(&self) -> usize {
                self.0
            }
            fn len_lines(&self) -> usize {
                1
            }
            fn len_bytes(&self) -> usize {
                self.0
            }
            fn char_at(&self, _idx: usize) -> Option<char> {
                None
            }
            fn slice_chars(&self, _start: usize, _end: usize) -> String {
                String::new()
            }
            fn line_slice(&self, _line: usize) -> String {
                String::new()
            }
            fn line_len(&self, _line: usize) -> usize {
                0
            }
            fn line_start(&self, _line: usize) -> usize {
                0
            }
            fn line_of_char(&self, _idx: usize) -> usize {
                0
            }
            fn insert(&mut self, _char_idx: usize, _text: &str) {}
            fn remove(&mut self, _start: usize, _end: usize) {}
            fn as_string(&self) -> String {
                String::new()
            }
            fn find_literal(&self, _n: &str, _c: bool, _l: usize) -> Vec<usize> {
                Vec::new()
            }
            fn line_hash(&self, _line: usize) -> u128 {
                0
            }
            fn snapshot(&self) -> Option<crate::buffer::storage::Snapshot> {
                None
            }
        }
        assert!(compare(&Big(MAX_BYTES), &Big(1)).is_none());
        assert!(compare(&Big(1024), &Big(1024)).is_some());
    }
}
