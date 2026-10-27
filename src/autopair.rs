//! Auto-closing brackets and quotes.
//!
//! A pair is one object, and typing it should not mean typing both halves. Four
//! behaviours follow from that, and they all live here as pure functions over
//! the characters around a caret — no document, no terminal, no frame:
//!
//! - an opener inserts its closer and leaves the caret between them;
//! - an opener typed over a selection surrounds it instead of replacing it;
//! - a closer that is already next to the caret is stepped over, not doubled;
//! - backspace between an empty pair removes both halves;
//! - Enter between a bracket pair opens the pair onto three lines.
//!
//! The interesting part is not the table of pairs, it is deciding when *not* to
//! pair. `don't`, a Rust lifetime `&'a` and a numeric suffix `3'` all contain a
//! quote that is punctuation rather than the start of a string, and an editor
//! that pairs them is worse than one that never pairs at all.

/// What to do with one typed character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// Insert it as usual.
    Plain,
    /// Insert `text` in its place, leaving the caret `caret_back` characters
    /// before the end of what was inserted.
    ///
    /// `anchor_back`, when set, leaves a selection behind: the anchor sits that
    /// many characters before the end, so the two together select the text that
    /// was surrounded. That is what lets a wrapped selection be wrapped again.
    Insert {
        text: String,
        caret_back: usize,
        anchor_back: Option<usize>,
    },
    /// The matching character is already next to the caret: step over it.
    Step,
}

/// The text around a caret, as the rules see it.
#[derive(Debug, Clone, Copy)]
pub struct Around<'a> {
    /// The character immediately before the caret's range, if there is one.
    pub before: Option<char>,
    /// The character immediately after it.
    pub after: Option<char>,
    /// The selected text, when the caret holds a selection rather than a point.
    pub selection: Option<&'a str>,
}

/// The character that completes a pair started by `typed`.
///
/// Quotes complete themselves, which is what makes `"` both an opener and a
/// closer in the rules below.
pub fn closer_for(typed: char) -> Option<char> {
    match typed {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' | '\'' | '`' => Some(typed),
        _ => None,
    }
}

/// True for a character that can close a pair: the three bracket closers, and
/// the three quotes, which close themselves.
pub fn closes(typed: char) -> bool {
    matches!(typed, ')' | ']' | '}' | '"' | '\'' | '`')
}

/// True for the three quote characters.
pub fn is_quote(typed: char) -> bool {
    matches!(typed, '"' | '\'' | '`')
}

/// What typing `typed` should do, given what surrounds the caret.
pub fn decide(typed: char, around: Around<'_>) -> Typed {
    // A selection is surrounded, never replaced: `foo` becomes `(foo)`. It also
    // stays selected, so a second bracket wraps the same text again rather than
    // replacing it — which is the whole point of surrounding something.
    if let Some(sel) = around.selection {
        return match closer_for(typed) {
            Some(close) => Typed::Insert {
                text: format!("{typed}{sel}{close}"),
                caret_back: 1,
                anchor_back: Some(1 + sel.chars().count()),
            },
            None => Typed::Plain,
        };
    }

    // Typing a closer that is already there: step over it rather than doubling
    // it. Only closers qualify — typing `(` in front of a `)` wants a new pair,
    // not a jump past the one that is there.
    if closes(typed) && around.after == Some(typed) {
        return Typed::Step;
    }

    // A caret between two letters is inside a word, and a pair there would cut
    // it in half: `hel|lo` must not become `hel(|)lo`. The character is typed
    // as it is, whatever it was.
    if inside_a_word(around.before, around.after) {
        return Typed::Plain;
    }

    let Some(close) = closer_for(typed) else {
        return Typed::Plain;
    };
    // A quote only pairs where a string could plausibly start. A bracket pairs
    // anywhere else: `(` after a word is still an opening bracket.
    if is_quote(typed) && !quote_starts_here(around.before) {
        return Typed::Plain;
    }
    Typed::Insert {
        text: format!("{typed}{close}"),
        caret_back: 1,
        anchor_back: None,
    }
}

/// True for the characters a word is made of.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether the caret sits between two characters of one word.
///
/// Both sides have to be word characters: the end of a word is not the middle
/// of one, so `hello|` still opens a pair, while `hel|lo` does not.
fn inside_a_word(before: Option<char>, after: Option<char>) -> bool {
    matches!((before, after), (Some(b), Some(a)) if is_word(b) && is_word(a))
}

/// Whether a quote typed after `before` starts a string rather than being
/// punctuation.
///
/// A word character means the quote continues a word (`don't`, `3'`), and `&`,
/// `<` and `>` mean it is a Rust lifetime (`&'a`, `Vec<'a>`) — the two ways a
/// quote is most often *not* a string in the files this editor is opened on.
/// A closing bracket is excluded too: `foo()'` is not a string.
fn quote_starts_here(before: Option<char>) -> bool {
    match before {
        None => true,
        Some(c) => !(is_word(c) || "&<>)]}'\"`".contains(c)),
    }
}

/// Whether the characters either side of the caret are the two halves of an
/// empty pair, and backspace should therefore remove both.
pub fn deletes_pair(before: Option<char>, after: Option<char>) -> bool {
    match (before, after) {
        (Some(b), Some(a)) => closer_for(b) == Some(a),
        _ => false,
    }
}

/// Whether Enter between these two characters should open them onto three
/// lines. Quotes are excluded: a string broken over lines is a different edit
/// from a block opened over lines, and guessing at it would be wrong more often
/// than right.
pub fn expands(open: char, close: char) -> bool {
    matches!((open, close), ('(', ')') | ('[', ']') | ('{', '}'))
}

/// The text Enter inserts between a bracket pair, and how far the caret backs
/// up from the end of it.
///
/// The pair opens onto three lines: the opener, a line indented one unit deeper
/// for the caret, and the closer back at the opener's own indentation.
pub fn expanded_newline(indent: &str, unit: &str) -> (String, usize) {
    (
        format!("\n{indent}{unit}\n{indent}"),
        1 + indent.chars().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(before: Option<char>, after: Option<char>) -> Around<'a> {
        Around {
            before,
            after,
            selection: None,
        }
    }

    fn inserted(text: &str, caret_back: usize) -> Typed {
        Typed::Insert {
            text: text.to_string(),
            caret_back,
            anchor_back: None,
        }
    }

    /// An insertion that leaves a selection behind.
    fn surrounded(text: &str, caret_back: usize, anchor_back: usize) -> Typed {
        Typed::Insert {
            text: text.to_string(),
            caret_back,
            anchor_back: Some(anchor_back),
        }
    }

    #[test]
    fn an_opener_inserts_its_closer_and_leaves_the_caret_inside() {
        assert_eq!(decide('(', at(Some(' '), None)), inserted("()", 1));
        assert_eq!(decide('[', at(None, None)), inserted("[]", 1));
        assert_eq!(decide('{', at(Some('x'), Some('\n'))), inserted("{}", 1));
    }

    #[test]
    fn a_selection_is_surrounded_rather_than_replaced() {
        let around = Around {
            before: Some('('),
            after: Some(')'),
            selection: Some("foo"),
        };
        // The caret sits after the surrounded text, one character before the
        // closer, and the anchor one character past the opener.
        assert_eq!(decide('(', around), surrounded("(foo)", 1, 4));

        let around = Around {
            before: Some(' '),
            after: None,
            selection: Some("bar"),
        };
        assert_eq!(decide('"', around), surrounded("\"bar\"", 1, 4));
        assert_eq!(decide(')', around), Typed::Plain, "a closer cannot open");
    }

    #[test]
    fn a_closer_that_is_already_there_is_stepped_over() {
        assert_eq!(decide(')', at(Some('('), Some(')'))), Typed::Step);
        assert_eq!(decide('}', at(Some('{'), Some('}'))), Typed::Step);
        assert_eq!(decide('"', at(Some('x'), Some('"'))), Typed::Step);
    }

    #[test]
    fn an_opener_before_a_closer_still_opens_a_new_pair() {
        // The caret sits in front of a `)`: `(` wants `(|)`, not a jump past it.
        assert_eq!(decide('(', at(Some('('), Some(')'))), inserted("()", 1));
    }

    #[test]
    fn a_closer_with_nothing_to_close_is_just_a_character() {
        assert_eq!(decide(')', at(Some('x'), None)), Typed::Plain);
        assert_eq!(decide(')', at(Some('x'), Some('y'))), Typed::Plain);
    }

    #[test]
    fn a_quote_after_a_word_is_punctuation() {
        // `don't`, `3'`, and Rust's `&'a`: none of these is a string.
        assert_eq!(decide('\'', at(Some('n'), None)), Typed::Plain);
        assert_eq!(decide('\'', at(Some('3'), None)), Typed::Plain);
        assert_eq!(decide('\'', at(Some('&'), None)), Typed::Plain);
        assert_eq!(decide('\'', at(Some('<'), None)), Typed::Plain);
        assert_eq!(decide('\'', at(Some('_'), None)), Typed::Plain);
        assert_eq!(decide('\'', at(Some(')'), None)), Typed::Plain);
    }

    #[test]
    fn a_quote_after_whitespace_or_an_opener_starts_a_string() {
        assert_eq!(decide('"', at(Some(' '), None)), inserted("\"\"", 1));
        assert_eq!(decide('\'', at(None, None)), inserted("''", 1));
        assert_eq!(decide('`', at(Some('('), None)), inserted("``", 1));
        assert_eq!(decide('"', at(Some('\n'), None)), inserted("\"\"", 1));
        assert_eq!(decide('\'', at(Some(','), None)), inserted("''", 1));
        assert_eq!(decide('\'', at(Some('='), None)), inserted("''", 1));
    }

    #[test]
    fn a_bracket_pairs_at_the_edge_of_a_word() {
        assert_eq!(decide('(', at(Some('f'), None)), inserted("()", 1));
        assert_eq!(decide('[', at(Some(']'), None)), inserted("[]", 1));
        assert_eq!(decide('(', at(Some('o'), Some(' '))), inserted("()", 1));
        assert_eq!(decide('(', at(Some(' '), Some('o'))), inserted("()", 1));
        assert_eq!(decide('(', at(None, Some('o'))), inserted("()", 1));
    }

    #[test]
    fn a_pair_is_not_opened_inside_a_word() {
        // `hel|lo` must not become `hel(|)lo`: the pair would cut the word in
        // half, and the character typed there is just a character.
        assert_eq!(decide('(', at(Some('l'), Some('l'))), Typed::Plain);
        assert_eq!(decide('{', at(Some('1'), Some('2'))), Typed::Plain);
        assert_eq!(decide('[', at(Some('a'), Some('_'))), Typed::Plain);
        assert_eq!(decide('(', at(Some('_'), Some('b'))), Typed::Plain);
        assert_eq!(decide('"', at(Some('a'), Some('b'))), Typed::Plain);
    }

    #[test]
    fn a_selection_inside_a_word_is_still_surrounded() {
        // The rule is about a caret between two letters. A selection is not a
        // caret in a word: it is the thing being wrapped.
        let around = Around {
            before: Some('e'),
            after: Some('o'),
            selection: Some("ll"),
        };
        assert_eq!(decide('(', around), surrounded("(ll)", 1, 3));
    }

    #[test]
    fn backspace_between_an_empty_pair_removes_both() {
        assert!(deletes_pair(Some('('), Some(')')));
        assert!(deletes_pair(Some('{'), Some('}')));
        assert!(deletes_pair(Some('"'), Some('"')));
        assert!(!deletes_pair(Some('('), Some(']')), "not a pair");
        assert!(!deletes_pair(Some('a'), Some(')')));
        assert!(!deletes_pair(None, Some(')')));
        assert!(!deletes_pair(Some(')'), Some('(')), "reversed");
    }

    #[test]
    fn enter_opens_a_bracket_pair_onto_three_lines() {
        assert!(expands('{', '}'));
        assert!(expands('(', ')'));
        assert!(!expands('"', '"'), "a quote is not a block");

        let (text, back) = expanded_newline("    ", "  ");
        assert_eq!(text, "\n      \n    ");
        // The caret belongs at the end of the middle line: one newline plus the
        // closer's four columns back from the end.
        assert_eq!(back, 5);
    }

    #[test]
    fn an_unindented_pair_opens_at_the_margin() {
        let (text, back) = expanded_newline("", "\t");
        assert_eq!(text, "\n\t\n");
        assert_eq!(back, 1);
    }
}
