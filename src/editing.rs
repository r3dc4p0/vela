//! Higher level editing operations.
//!
//! These build on the primitives in [`crate::buffer::Document`] and are used by
//! commands that act on whole lines or selections. Everything here is a pure
//! function of `(&mut Document, settings)`, which keeps it testable without a
//! terminal and keeps `App` free of editing logic.

use std::ops::Range;

use crate::buffer::{Document, Edit};
use crate::syntax::Language;

/// Line comment marker for a language, if it has one.
pub fn line_comment(lang: Language) -> Option<&'static str> {
    match lang {
        Language::Rust
        | Language::JavaScript
        | Language::TypeScript
        | Language::C
        | Language::Cpp
        | Language::Go
        | Language::Java
        | Language::Json => Some("//"),
        Language::Python | Language::Shell | Language::Toml | Language::Yaml => Some("#"),
        Language::Sql => Some("--"),
        // Markdown, HTML and CSS have no line comment token we can safely
        // insert without knowing the context.
        Language::Markdown | Language::Html | Language::Css | Language::Plain => None,
    }
}

/// Inclusive line range covered by the cursor and the selection.
pub fn selected_lines(doc: &Document) -> Range<usize> {
    let storage = doc.storage();
    // Use the selection bounds, not the cursor: the cursor is the moving end of
    // a selection and may sit on the line *after* the last selected one.
    let (lo, hi, has_selection) = match doc.selection() {
        Some(sel) => (sel.start, sel.end, true),
        None => (doc.cursor(), doc.cursor(), false),
    };
    let len = doc.len_chars();
    let first = storage.line_of_char(lo.min(len));
    let mut last = storage.line_of_char(hi.min(len));
    // A selection ending exactly at a line start must not include that line.
    // Only applies to real selections: a collapsed cursor sits on a line start
    // most of the time and must still address its own line.
    if has_selection && last > first && hi == storage.line_start(last) {
        last -= 1;
    }
    first.min(last)..last.max(first) + 1
}

/// Character index just past the content of `line` (excluding the newline).
pub fn line_end(doc: &Document, line: usize) -> usize {
    let start = doc.storage().line_start(line);
    start + doc.storage().line_slice(line).chars().count()
}

/// Re-select the given line range after a batch edit that shifted offsets.
///
/// Line-oriented commands (comment toggle, indent) must leave the user's
/// selection in place so the command can be repeated.
fn restore_line_selection(doc: &mut Document, lines: Range<usize>) {
    if lines.start >= doc.line_count() {
        return;
    }
    let start = doc.storage().line_start(lines.start);
    let last = (lines.end - 1).min(doc.line_count() - 1);
    let end = line_end(doc, last);
    doc.select_range(start..end);
}

/// Character index after the newline that terminates `line`, if any.
pub fn line_end_with_newline(doc: &Document, line: usize) -> usize {
    let end = line_end(doc, line);
    if end < doc.len_chars() {
        end + 1
    } else {
        end
    }
}

/// Duplicate the current line or selected block below itself.
pub fn duplicate_lines(doc: &mut Document) -> bool {
    let lines = selected_lines(doc);
    let block: Vec<String> = (lines.start..lines.end).map(|l| doc.line_text(l)).collect();
    if block.is_empty() {
        return false;
    }
    let text = block.join("\n");
    let column = doc
        .cursor()
        .saturating_sub(doc.storage().line_start(lines.start));
    let insert_at = line_end(doc, lines.end - 1);
    let payload = format!("\n{text}");
    doc.replace_range(insert_at, insert_at, &payload, crate::buffer::TxKind::Other);
    // Land on the duplicated block at the same column, which is what users
    // expect from "duplicate line" followed by typing.
    doc.set_cursor(insert_at + 1 + column);
    true
}

/// Delete the current line or selected block, including its line break.
pub fn delete_lines(doc: &mut Document) -> bool {
    if doc.len_chars() == 0 {
        return false;
    }
    let lines = selected_lines(doc);
    let start = doc.storage().line_start(lines.start);
    let mut end = line_end(doc, lines.end - 1);
    // Remove the trailing newline when there is one; otherwise remove the
    // preceding one so we do not leave a blank line behind.
    if end < doc.len_chars() {
        end += 1;
    } else if start > 0 {
        doc.delete_range(start - 1..end);
        let new_cursor = (start - 1).min(doc.len_chars());
        doc.set_cursor(new_cursor);
        return true;
    }
    if start == end {
        return false;
    }
    doc.delete_range(start..end);
    doc.set_cursor(start.min(doc.len_chars()));
    true
}

/// Move the current line or selected block up or down by one line.
pub fn move_lines(doc: &mut Document, up: bool) -> bool {
    let lines = selected_lines(doc);
    let last_line = doc.line_count().saturating_sub(1);
    if up && lines.start == 0 {
        return false;
    }
    if !up && lines.end > last_line {
        return false;
    }

    let (region_start, region_end) = if up {
        (
            doc.storage().line_start(lines.start - 1),
            line_end_with_newline(doc, lines.end - 1),
        )
    } else {
        (
            doc.storage().line_start(lines.start),
            line_end_with_newline(doc, lines.end),
        )
    };

    let region = doc.storage().slice_chars(region_start, region_end);
    let had_trailing_newline = region.ends_with('\n');
    let body = region.strip_suffix('\n').unwrap_or(&region);
    let block: Vec<&str> = body.split('\n').collect();
    let rebuilt = if up {
        // [above, block...] -> [block..., above]
        let mut parts: Vec<&str> = block[1..].to_vec();
        parts.push(block[0]);
        parts.join("\n")
    } else {
        // [block..., below] -> [below, block...]
        let n = block.len();
        if n < 2 {
            return false;
        }
        let mut parts: Vec<&str> = Vec::with_capacity(n);
        parts.push(block[n - 1]);
        parts.extend_from_slice(&block[..n - 1]);
        parts.join("\n")
    };

    // Preserve the trailing newline structure of the region.
    let rebuilt = if had_trailing_newline {
        format!("{rebuilt}\n")
    } else {
        rebuilt
    };
    if rebuilt == region {
        return false;
    }

    let cursor_col = doc
        .cursor()
        .saturating_sub(doc.storage().line_start(lines.start));
    doc.replace_range(
        region_start,
        region_end,
        &rebuilt,
        crate::buffer::TxKind::Other,
    );

    let new_first = if up { lines.start - 1 } else { lines.start + 1 };
    let new_start = doc
        .storage()
        .line_start(new_first.min(doc.line_count() - 1));
    doc.set_cursor(new_start + cursor_col);
    true
}

/// Comment or uncomment the selected lines.
pub fn toggle_comment(doc: &mut Document) -> bool {
    let Some(marker) = line_comment(doc.language()) else {
        return false;
    };
    let lines = selected_lines(doc);
    let had_selection = doc.has_selection();

    // Uncomment when every non-blank line already carries the marker.
    let mut all_commented = true;
    let mut any_content = false;
    for line in lines.clone() {
        let text = doc.line_text(line);
        if text.trim().is_empty() {
            continue;
        }
        any_content = true;
        if !text.trim_start().starts_with(marker) {
            all_commented = false;
            break;
        }
    }
    if !any_content {
        return false;
    }

    let mut edits = Vec::new();
    for line in lines.clone() {
        let text = doc.line_text(line);
        if text.trim().is_empty() {
            continue;
        }
        let indent = text.len() - text.trim_start().len();
        let line_start = doc.storage().line_start(line);
        if all_commented {
            // Remove the marker and one following space if present.
            let after = text[indent + marker.len()..].to_string();
            let extra = usize::from(after.starts_with(' '));
            edits.push(Edit::delete(
                line_start + indent..line_start + indent + marker.len() + extra,
            ));
        } else {
            edits.push(Edit::insert(line_start + indent, format!("{marker} ")));
        }
    }
    if edits.is_empty() {
        return false;
    }
    let cursor = doc.cursor();
    let changed = doc.apply_batch(edits, cursor);
    if changed && had_selection {
        restore_line_selection(doc, lines);
    }
    changed
}

/// Indent every selected line by one unit, or insert a unit at the cursor.
pub fn indent(doc: &mut Document, unit: &str) -> bool {
    if doc.has_selection() {
        let lines = selected_lines(doc);
        let mut edits = Vec::new();
        for line in lines.clone() {
            let start = doc.storage().line_start(line);
            edits.push(Edit::insert(start, unit.to_string()));
        }
        let cursor = doc.cursor() + unit.chars().count();
        let changed = doc.apply_batch(edits, cursor);
        if changed {
            restore_line_selection(doc, lines);
        }
        return changed;
    }
    // A tab at the cursor expands to the next tab stop.
    doc.insert_text(unit);
    true
}

/// Remove one indent level from every selected line, or one tab stop before the
/// cursor.
pub fn dedent(doc: &mut Document, tab_width: usize) -> bool {
    let had_selection = doc.has_selection();
    let lines = if had_selection {
        selected_lines(doc)
    } else {
        let (line, _) = doc.cursor_line_col();
        line..line + 1
    };

    let mut edits = Vec::new();
    for line in lines.clone() {
        let text = doc.line_text(line);
        let start = doc.storage().line_start(line);
        let mut removed = 0usize;
        for c in text.chars() {
            if c == ' ' && removed < tab_width {
                removed += 1;
            } else if c == '\t' && removed == 0 {
                removed = 1;
                break;
            } else {
                break;
            }
        }
        if removed > 0 {
            edits.push(Edit::delete(start..start + removed));
        }
    }
    if edits.is_empty() {
        return false;
    }
    let shift: usize = edits
        .iter()
        .filter(|e| e.range.start <= doc.cursor())
        .map(|e| e.range.end - e.range.start)
        .sum();
    let cursor = doc.cursor().saturating_sub(shift);
    let changed = doc.apply_batch(edits, cursor);
    if changed && had_selection {
        restore_line_selection(doc, lines);
    }
    changed
}

/// Strip trailing whitespace from every line.
pub fn trim_trailing_whitespace(doc: &mut Document) -> bool {
    let mut edits = Vec::new();
    for line in 0..doc.line_count() {
        let text = doc.line_text(line);
        let trimmed_len = text.trim_end_matches([' ', '\t']).len();
        if trimmed_len != text.len() {
            let start = doc.storage().line_start(line);
            let from = start + text[..trimmed_len].chars().count();
            let to = start + text.chars().count();
            edits.push(Edit::delete(from..to));
        }
    }
    if edits.is_empty() {
        return false;
    }
    let cursor = doc.cursor();
    doc.apply_batch(edits, cursor)
}

/// How far bracket matching scans before giving up. A bound keeps the cost
/// predictable on huge files; the result is cached per revision in `App`.
const BRACKET_SCAN_LIMIT: usize = 20_000;

/// Find the bracket pair around `cursor`.
///
/// Looks at the character under the cursor and the one before it (the usual
/// convention), then scans for the balanced partner. Strings and comments are
/// **not** skipped — that would need the syntax tree; the scan is therefore
/// approximate for pathological input but never wrong about nesting depth.
pub fn matching_brackets(
    storage: &dyn crate::buffer::TextStorage,
    cursor: usize,
) -> Option<(usize, usize)> {
    const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

    let candidates = [cursor, cursor.saturating_sub(1)];
    for pos in candidates {
        let Some(c) = storage.char_at(pos) else {
            continue;
        };
        for (open, close) in PAIRS {
            if c == open {
                if let Some(found) = scan_bracket(storage, pos, open, close, true) {
                    return Some((pos, found));
                }
            } else if c == close {
                if let Some(found) = scan_bracket(storage, pos, close, open, false) {
                    return Some((found, pos));
                }
            }
        }
    }
    None
}

fn scan_bracket(
    storage: &dyn crate::buffer::TextStorage,
    from: usize,
    same: char,
    other: char,
    forward: bool,
) -> Option<usize> {
    let mut depth = 0i32;
    let len = storage.len_chars();
    if forward {
        let end = (from + BRACKET_SCAN_LIMIT).min(len);
        for i in from..end {
            let c = storage.char_at(i)?;
            if c == same {
                depth += 1;
            } else if c == other {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    } else {
        let start = from.saturating_sub(BRACKET_SCAN_LIMIT);
        for i in (start..=from).rev() {
            let c = storage.char_at(i)?;
            if c == same {
                depth += 1;
            } else if c == other {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Insert text at the cursor on every line touched by the selection (block
/// editing); not exposed as a default binding yet.
pub fn insert_on_each_line(doc: &mut Document, text: &str) -> bool {
    let lines = selected_lines(doc);
    let mut edits = Vec::new();
    for line in lines {
        let start = doc.storage().line_start(line);
        edits.push(Edit::insert(start, text.to_string()));
    }
    let cursor = doc.cursor();
    doc.apply_batch(edits, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::from_text(text, None)
    }

    #[test]
    fn duplicate_single_line() {
        let mut d = doc("abc");
        d.set_cursor(1);
        assert!(duplicate_lines(&mut d));
        assert_eq!(d.text(), "abc\nabc");
        assert_eq!(d.cursor(), 5);
    }

    #[test]
    fn duplicate_block() {
        let mut d = doc("a\nb\nc");
        d.select_range(0..3); // lines a and b
        assert!(duplicate_lines(&mut d));
        assert_eq!(d.text(), "a\nb\na\nb\nc");
    }

    #[test]
    fn delete_middle_line_keeps_layout() {
        let mut d = doc("a\nb\nc\n");
        d.goto_line(1);
        assert!(delete_lines(&mut d));
        assert_eq!(d.text(), "a\nc\n");
    }

    #[test]
    fn delete_last_line_removes_previous_newline() {
        let mut d = doc("a\nb");
        d.goto_line(1);
        assert!(delete_lines(&mut d));
        assert_eq!(d.text(), "a");
    }

    #[test]
    fn delete_only_line_empties_buffer() {
        let mut d = doc("only");
        assert!(delete_lines(&mut d));
        assert_eq!(d.text(), "");
    }

    #[test]
    fn move_line_up_and_down() {
        let mut d = doc("a\nb\nc\n");
        d.goto_line(1);
        assert!(move_lines(&mut d, true));
        assert_eq!(d.text(), "b\na\nc\n");
        assert!(move_lines(&mut d, false));
        assert_eq!(d.text(), "a\nb\nc\n");
    }

    #[test]
    fn move_line_at_boundary_is_noop() {
        let mut d = doc("a\nb");
        d.goto_line(0);
        assert!(!move_lines(&mut d, true));
        d.goto_line(1);
        assert!(!move_lines(&mut d, false));
    }

    #[test]
    fn toggle_comment_adds_and_removes() {
        let mut d = Document::from_text("let a = 1;\nlet b = 2;\n", None);
        d.set_path(Some(std::path::PathBuf::from("x.rs")));
        d.select_all();
        assert!(toggle_comment(&mut d));
        assert_eq!(d.text(), "// let a = 1;\n// let b = 2;\n");
        assert!(toggle_comment(&mut d));
        assert_eq!(d.text(), "let a = 1;\nlet b = 2;\n");
    }

    #[test]
    fn toggle_comment_preserves_indent() {
        let mut d = Document::from_text("    x\n", None);
        d.set_path(Some(std::path::PathBuf::from("x.rs")));
        d.select_all();
        toggle_comment(&mut d);
        assert_eq!(d.text(), "    // x\n");
    }

    #[test]
    fn toggle_comment_unsupported_language() {
        let mut d = doc("plain");
        assert!(!toggle_comment(&mut d));
    }

    #[test]
    fn indent_and_dedent_block() {
        let mut d = doc("a\nb\n");
        d.select_all();
        assert!(indent(&mut d, "  "));
        assert_eq!(d.text(), "  a\n  b\n");
        assert!(dedent(&mut d, 2));
        assert_eq!(d.text(), "a\nb\n");
    }

    #[test]
    fn dedent_stops_at_content() {
        let mut d = doc("  x\n");
        d.select_all();
        assert!(dedent(&mut d, 4));
        assert_eq!(d.text(), "x\n");
    }

    #[test]
    fn trim_trailing_whitespace_removes_spaces_and_tabs() {
        let mut d = doc("a  \nb\t\nc\n");
        assert!(trim_trailing_whitespace(&mut d));
        assert_eq!(d.text(), "a\nb\nc\n");
        assert!(!trim_trailing_whitespace(&mut d), "idempotent");
    }

    #[test]
    fn batch_edit_is_one_undo_step() {
        let mut d = doc("a  \nb  \n");
        assert!(trim_trailing_whitespace(&mut d));
        assert_eq!(d.text(), "a\nb\n");
        assert!(d.undo());
        assert_eq!(d.text(), "a  \nb  \n", "one undo restores every line");
    }

    #[test]
    fn duplicate_then_undo_restores() {
        let mut d = doc("x\ny");
        d.goto_line(0);
        duplicate_lines(&mut d);
        assert_eq!(d.text(), "x\nx\ny");
        assert!(d.undo());
        assert_eq!(d.text(), "x\ny");
    }
}
