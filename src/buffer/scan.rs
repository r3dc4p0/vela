//! Fast literal search over a [`ropey::Rope`].
//!
//! Two properties matter here:
//!
//! * **No full-document copy.** The rope is walked chunk by chunk into a
//!   reusable 64 KiB window with a small overlap, so memory stays flat and a
//!   100 MiB file is scanned without allocating 100 MiB.
//! * **Byte level matching.** `memchr::memmem` gives SIMD accelerated
//!   substring search; byte offsets are converted to character offsets in a
//!   single pass instead of `chars().count()` per hit (which would be
//!   quadratic for frequent needles).
//!
//! Case-insensitive search folds ASCII only on the fast path. That covers the
//! overwhelming majority of code and prose; needles containing non-ASCII
//! characters fall back to a correct but slower character-window scan.

use memchr::memmem;
use ropey::Rope;

/// Window size used while streaming the rope.
const WINDOW: usize = 64 * 1024;

/// Find all non-overlapping occurrences of `needle`, returning ascending
/// character offsets. Stops after `limit` hits.
pub fn find_literal(rope: &Rope, needle: &str, case_sensitive: bool, limit: usize) -> Vec<usize> {
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let needle_len = needle.len();

    // Non-ASCII case-insensitive search needs real Unicode folding.
    if !case_sensitive && !needle.is_ascii() {
        return find_ci_unicode(rope, needle, limit);
    }

    let folded;
    let pattern: &[u8] = if case_sensitive {
        needle.as_bytes()
    } else {
        folded = needle.to_ascii_lowercase();
        folded.as_bytes()
    };
    let finder = memmem::Finder::new(pattern);

    let mut out: Vec<usize> = Vec::new();
    let mut buf: Vec<u8> = Vec::with_capacity(WINDOW + needle_len);
    let mut base_char = 0usize;
    let keep = needle_len - 1;

    for chunk in rope.chunks() {
        buf.extend_from_slice(chunk.as_bytes());
        if buf.len() < WINDOW {
            continue;
        }
        // Only accept matches that start strictly before `cut`; everything from
        // `cut` on is retained (at a char boundary) and rescanned with the next
        // chunk, so no match is missed or counted twice.
        let mut cut = buf.len().saturating_sub(keep);
        while cut > 0 && cut < buf.len() && (buf[cut] & 0xC0) == 0x80 {
            cut -= 1;
        }
        scan_window(
            &buf,
            base_char,
            &finder,
            !case_sensitive,
            cut,
            &mut out,
            limit,
        );
        if out.len() >= limit {
            return out;
        }
        base_char += count_chars(&buf[..cut]);
        buf.drain(..cut);
    }

    if !buf.is_empty() {
        let cut = buf.len();
        scan_window(
            &buf,
            base_char,
            &finder,
            !case_sensitive,
            cut,
            &mut out,
            limit,
        );
    }
    out
}

/// Scan one contiguous window, appending character offsets of matches whose
/// byte start is `< accept_before`.
fn scan_window(
    buf: &[u8],
    base_char: usize,
    finder: &memmem::Finder<'_>,
    fold: bool,
    accept_before: usize,
    out: &mut Vec<usize>,
    limit: usize,
) {
    let lowered;
    let hay: &[u8] = if fold {
        lowered = buf.to_ascii_lowercase();
        &lowered
    } else {
        buf
    };

    let remaining = limit.saturating_sub(out.len());
    if remaining == 0 {
        return;
    }
    let positions: Vec<usize> = finder
        .find_iter(hay)
        .take_while(|&p| p < accept_before)
        .take(remaining)
        .collect();
    if positions.is_empty() {
        return;
    }

    // Single pass: walk the window once, counting characters, and emit a hit
    // whenever the byte cursor reaches the next match offset.
    let mut next = 0usize;
    let mut chars = 0usize;
    let mut i = 0usize;
    while i < hay.len() {
        if next < positions.len() && positions[next] == i {
            out.push(base_char + chars);
            next += 1;
            if next == positions.len() {
                return;
            }
        }
        // Advance one UTF-8 code point.
        let b = hay[i];
        let step = utf8_len(b);
        i += step;
        chars += 1;
    }
}

/// Length in bytes of the UTF-8 sequence starting with `b`.
#[inline]
fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Character count of a valid UTF-8 slice.
#[inline]
fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count()
}

/// Correct-but-slower case-insensitive scan for needles containing non-ASCII.
fn find_ci_unicode(rope: &Rope, needle: &str, limit: usize) -> Vec<usize> {
    let needle: Vec<char> = needle.chars().collect();
    let n = needle.len();
    let needle_folded: Vec<Vec<char>> = needle.iter().map(|c| c.to_lowercase().collect()).collect();

    let mut out = Vec::new();
    let mut window: std::collections::VecDeque<char> = std::collections::VecDeque::with_capacity(n);
    let mut idx = 0usize;
    for ch in rope.chars() {
        if window.len() == n {
            window.pop_front();
        }
        window.push_back(ch);
        idx += 1;
        if window.len() == n {
            let matches = window.iter().zip(needle_folded.iter()).all(|(hay, want)| {
                if hay.is_ascii() {
                    hay.eq_ignore_ascii_case(&want[0])
                } else {
                    hay.to_lowercase().eq(want.iter().copied())
                }
            });
            if matches {
                out.push(idx - n);
                if out.len() >= limit {
                    break;
                }
                // Start looking again only after this match: matches must not
                // overlap, or a query of "ab" in "abab" would report 0..2,
                // 1..3 and 2..4, double-counting in the status bar and giving
                // replace-all overlapping edits to apply. The ASCII path gets
                // this for free from `memmem`'s non-overlapping iterator.
                window.clear();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str, needle: &str, cs: bool) -> Vec<usize> {
        find_literal(&Rope::from_str(text), needle, cs, usize::MAX)
    }

    #[test]
    fn finds_simple_occurrences() {
        assert_eq!(find("abcabc", "abc", true), vec![0, 3]);
        assert_eq!(find("abcabc", "bc", true), vec![1, 4]);
        assert_eq!(find("abc", "zzz", true), vec![]);
    }

    #[test]
    fn offsets_are_chars_not_bytes() {
        // 'é' is two bytes; the match starts at char index 1, not 2.
        assert_eq!(find("éabc", "abc", true), vec![1]);
    }

    #[test]
    fn case_insensitive_ascii() {
        assert_eq!(find("Hello HELLO hello", "hello", false), vec![0, 6, 12]);
    }

    #[test]
    fn case_insensitive_unicode_fallback() {
        assert_eq!(find("ÄÖÜ äöü", "äöü", false), vec![0, 4]);
        assert_eq!(find("ÄÖÜ äöü", "äöü", true), vec![4]);
    }

    #[test]
    fn overlapping_matches_are_not_double_counted() {
        assert_eq!(find("aaaa", "aa", true), vec![0, 2]);
    }

    #[test]
    fn matches_across_window_boundary() {
        // Force the streaming path with a document larger than one window and a
        // needle straddling the 64 KiB cut.
        let mut text = "x".repeat(WINDOW - 3);
        text.push_str("NEEDLE");
        text.push_str(&"y".repeat(1000));
        let rope = Rope::from_str(&text);
        let hits = find_literal(&rope, "NEEDLE", true, 10);
        assert_eq!(hits, vec![WINDOW - 3]);
    }

    #[test]
    fn respects_limit() {
        let text = "a".repeat(100);
        let hits = find_literal(&Rope::from_str(&text), "a", true, 5);
        assert_eq!(hits.len(), 5);
    }

    #[test]
    fn empty_needle_matches_nothing() {
        assert!(find("abc", "", true).is_empty());
    }

    #[test]
    fn needle_longer_than_window_is_safe() {
        let text = "ab".repeat(40_000);
        let rope = Rope::from_str(&text);
        let needle = "ab".repeat(40_000);
        assert_eq!(find_literal(&rope, &needle, true, 4), vec![0]);
    }
}
