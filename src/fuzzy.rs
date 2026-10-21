//! Fuzzy matching for the command palette and the file finder.
//!
//! Scores a subsequence match with the bonuses that make fuzzy finders feel
//! right: matches at word boundaries, consecutive runs, matches near the start
//! and case-consistent matches all score higher; long gaps score lower.
//!
//! The matcher is intentionally allocation-light (one `Vec` for positions) and
//! is used on every keystroke over a few thousand candidates, so it stays
//! O(query * target) with early exit on the first non-matching character.

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// Character positions in the target that matched, for highlighting.
    pub positions: Vec<usize>,
}

const BONUS_BOUNDARY: i32 = 30;
const BONUS_CONSECUTIVE: i32 = 60;
const BONUS_START: i32 = 20;
const BONUS_EXACT_CASE: i32 = 4;
const PENALTY_GAP: i32 = 3;

/// Score `query` against `target`. Returns `None` when the query is not a
/// subsequence of the target.
///
/// An empty query matches everything with a neutral score, which is what the
/// palette wants when it first opens.
pub fn score(query: &str, target: &str) -> Option<Match> {
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    let target_chars: Vec<char> = target.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    if query_chars.len() > target_chars.len() {
        return None;
    }

    // Greedy forward pass: earliest match for each query character.
    let mut positions = Vec::with_capacity(query_chars.len());
    let mut ti = 0usize;
    for (qi, &qc) in query_chars.iter().enumerate() {
        let ql = qc.to_ascii_lowercase();
        let mut found = None;
        while ti < target_chars.len() {
            let tc = target_chars[ti];
            if tc.to_ascii_lowercase() == ql {
                found = Some(ti);
                ti += 1;
                break;
            }
            ti += 1;
        }
        match found {
            Some(p) => positions.push(p),
            None => {
                let _ = qi;
                return None;
            }
        }
    }

    // Backward refinement: pull matches left where possible so that trailing
    // characters align with the end of a word.
    for qi in (0..query_chars.len()).rev() {
        let lower_bound = if qi == 0 { 0 } else { positions[qi - 1] + 1 };
        let mut best = positions[qi];
        let mut candidate = best;
        while candidate > lower_bound {
            candidate -= 1;
            if target_chars[candidate].eq_ignore_ascii_case(&query_chars[qi]) {
                best = candidate;
            } else {
                break;
            }
        }
        positions[qi] = best;
    }

    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    for (qi, &p) in positions.iter().enumerate() {
        if p == 0 {
            score += BONUS_START;
        } else {
            let prev_char = target_chars[p - 1];
            if is_boundary(prev_char) {
                score += BONUS_BOUNDARY;
            }
        }
        if let Some(prev_pos) = prev {
            let gap = p.saturating_sub(prev_pos + 1);
            if gap == 0 {
                score += BONUS_CONSECUTIVE;
            } else {
                score -= (gap as i32).min(10) * PENALTY_GAP;
            }
        }
        if target_chars[p] == query_chars[qi] {
            score += BONUS_EXACT_CASE;
        }
        prev = Some(p);
    }

    // Prefer shorter targets when scores tie (typical for `main.rs` vs
    // `main_test_helper.rs`).
    score -= (target_chars.len() as i32) / 8;

    Some(Match { score, positions })
}

fn is_boundary(c: char) -> bool {
    matches!(
        c,
        ' ' | '_' | '-' | '/' | '\\' | '.' | ':' | '(' | '[' | '{' | ','
    )
}

/// Rank and truncate candidates.
///
/// `key` extracts the display string from an item. Results are sorted by
/// descending score with the original order as a stable tie breaker.
pub fn filter<T, F>(items: &[T], query: &str, key: F, limit: usize) -> Vec<(usize, Match)>
where
    F: Fn(&T) -> String,
{
    let mut scored: Vec<(usize, Match)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| score(query, &key(item)).map(|m| (i, m)))
        .collect();
    scored.sort_by(|a, b| b.1.score.cmp(&a.1.score).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        let m = score("", "anything").unwrap();
        assert_eq!(m.score, 0);
    }

    #[test]
    fn rejects_non_subsequence() {
        assert!(score("xyz", "abc").is_none());
        assert!(score("abcd", "abc").is_none());
    }

    #[test]
    fn matches_subsequence_case_insensitively() {
        let m = score("cs", "Ctrl+S").unwrap();
        assert_eq!(m.positions, vec![0, 5]);
    }

    #[test]
    fn boundary_matches_beat_scattered_matches() {
        let boundary = score("fs", "file_save").unwrap();
        let scattered = score("fs", "aafxxsxx").unwrap();
        assert!(
            boundary.score > scattered.score,
            "{} vs {}",
            boundary.score,
            scattered.score
        );
    }

    #[test]
    fn consecutive_matches_beat_gapped() {
        let consecutive = score("save", "save").unwrap();
        let gapped = score("save", "s_a_v_e").unwrap();
        assert!(consecutive.score > gapped.score);
    }

    #[test]
    fn positions_are_ascending_and_in_bounds() {
        let target = "src/buffer/storage.rs";
        let m = score("bsr", target).unwrap();
        assert_eq!(m.positions.len(), 3);
        assert!(m.positions.windows(2).all(|w| w[0] < w[1]));
        assert!(*m.positions.last().unwrap() < target.chars().count());
    }

    #[test]
    fn unicode_targets_do_not_panic() {
        assert!(score("é", "café").is_some());
        assert!(score("日本", "日本語").is_some());
    }

    #[test]
    fn filter_ranks_and_limits() {
        let items = vec!["alpha.rs", "beta.rs", "gamma.rs"];
        let out = filter(&items, "ar", |s| s.to_string(), 2);
        assert_eq!(out.len(), 2);
        assert!(out[0].1.score >= out[1].1.score);
    }
}
