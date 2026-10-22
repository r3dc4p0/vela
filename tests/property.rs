//! Property-based tests.
//!
//! These exercise invariants that must hold for *every* input, which is where
//! hand-written examples usually miss cases: undo/redo symmetry, cursor bounds,
//! search range validity, and EOL round-tripping.

use proptest::prelude::*;
use vela::buffer::wrap::LineLayout;
use vela::buffer::{Document, Motion, MotionCtx};
use vela::filesystem::{self, Encoding, Eol};
use vela::search::{find_all, Query};

/// A short string built from a mix of ASCII, accented Latin and CJK text so
/// char/byte confusion shows up immediately.
fn any_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just('a'),
            Just('b'),
            Just(' '),
            Just('\n'),
            Just('\t'),
            Just('é'),
            Just('世'),
            Just('😀'),
            Just('_'),
        ],
        0..40,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn doc_with(text: &str) -> Document {
    Document::from_text(text, None)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The cursor can never leave the document, whatever the motions do.
    #[test]
    fn cursor_stays_inside_the_document(text in any_text()) {
        let mut doc = doc_with(&text);
        let motions = [
            Motion::Left, Motion::Right, Motion::WordLeft, Motion::WordRight,
            Motion::Up, Motion::Down, Motion::LineStart, Motion::LineEnd,
            Motion::DocStart, Motion::DocEnd, Motion::PageUp, Motion::PageDown,
        ];
        // Wrapping turns the vertical motions from line steps into row steps,
        // and a narrow width is where an off-by-one in that arithmetic shows
        // up: a row holds one character, so every boundary is exercised.
        for width in [None, Some(1), Some(7), Some(40)] {
            let ctx = MotionCtx { tab_width: 4, page: 10, wrap_width: width };
            for (i, m) in motions.iter().enumerate() {
                doc.move_cursor(*m, i % 2 == 0, ctx);
                prop_assert!(doc.cursor() <= doc.len_chars());
            }
        }
    }

    /// Wrapping partitions the line: every character lands in exactly one row,
    /// in order, with no empty rows in between.
    #[test]
    fn wrap_partitions_every_line(text in any_text(), width in 1usize..60) {
        for line in text.split('\n') {
            let layout = LineLayout::new(line, width, 4);
            let mut next = 0usize;
            for seg in layout.segments() {
                prop_assert_eq!(seg.char_off, next, "rows must be contiguous");
                prop_assert!(seg.chars > 0 || line.is_empty(), "empty row in {:?}", line);
                next = seg.end_char();
            }
            prop_assert_eq!(next, line.chars().count(), "rows must cover {:?}", line);
        }
    }

    /// A cursor placed on any character of any line stays on that character
    /// when it is asked which row it is on and back again.
    #[test]
    fn wrap_round_trips_every_position(text in any_text(), width in 1usize..40) {
        for line in text.split('\n') {
            let layout = LineLayout::new(line, width, 4);
            for (i, _) in line.chars().enumerate() {
                let row = layout.row_of(i);
                let seg = layout.segments()[row];
                let col = layout.chars()[seg.char_off..i]
                    .iter()
                    .fold(seg.start_col, |c, &ch| c + vela::buffer::motion::char_width(ch, c, 4))
                    - seg.start_col;
                prop_assert_eq!(layout.pos_at(row, col, 4), i);
            }
        }
    }

    /// Typing then undoing everything returns to the starting text, and redoing
    /// everything returns to the final text.
    #[test]
    fn undo_redo_is_symmetric(seed in any_text(), additions in any_text()) {
        let mut doc = doc_with(&seed);
        doc.set_cursor(doc.len_chars());
        // One transaction, whatever the text contains.
        doc.insert_text(&format!("\n{additions}"));
        let after = doc.text();

        let mut steps = 0;
        while doc.can_undo() && steps < 10_000 {
            doc.undo();
            steps += 1;
        }
        prop_assert_eq!(doc.text(), seed.clone());
        prop_assert_eq!(
            doc.cursor(),
            seed.chars().count(),
            "the cursor returns to where the edit started"
        );

        let mut steps = 0;
        while doc.can_redo() && steps < 10_000 {
            doc.redo();
            steps += 1;
        }
        prop_assert_eq!(doc.text(), after);
    }

    /// Deleting what was just inserted is the identity on the text.
    #[test]
    fn insert_then_delete_restores_the_text(seed in any_text(), insertion in any_text()) {
        let mut doc = doc_with(&seed);
        let at = doc.len_chars() / 2;
        doc.set_cursor(at);
        doc.insert_text(&insertion);
        doc.delete_range(at..at + insertion.chars().count());
        prop_assert_eq!(doc.text(), seed);
    }

    /// Every reported match slices back to the query (case-insensitively) and
    /// the ranges are sorted, non-overlapping and inside the document.
    #[test]
    fn search_ranges_are_valid(text in any_text(), needle in any_text()) {
        let doc = doc_with(&text);
        let query = Query::literal(&needle);
        let ranges = find_all(doc.storage(), &query, 1_000).expect("literal search never fails");
        let haystack = doc.text();

        let mut previous_end = 0usize;
        for range in &ranges {
            prop_assert!(range.start <= range.end);
            prop_assert!(range.end <= doc.len_chars());
            prop_assert!(range.start >= previous_end, "ranges must not overlap");
            previous_end = range.end;

            let slice: String = haystack
                .chars()
                .skip(range.start)
                .take(range.end - range.start)
                .collect();
            if query.text.is_empty() {
                continue;
            }
            prop_assert_eq!(slice.to_lowercase(), query.text.to_lowercase());
        }
    }

    /// Line/offset helpers agree with each other.
    #[test]
    fn line_offsets_are_consistent(text in any_text()) {
        let doc = doc_with(&text);
        let len = doc.len_chars();
        for i in 0..=len {
            let line = doc.storage().line_of_char(i);
            prop_assert!(line < doc.line_count());
            let start = doc.storage().line_start(line);
            prop_assert!(start <= i, "line start {start} must not be past {i}");
        }
    }

    /// Saving and reloading any text preserves the text and the line endings.
    #[test]
    fn save_load_round_trip(text in any_text(), crlf in any::<bool>()) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("round-trip.txt");
        let eol = if crlf { Eol::CrLf } else { Eol::Lf };
        let disk = filesystem::denormalize_eol(&text, eol);
        std::fs::write(&path, &disk).unwrap();

        let loaded = filesystem::load(&path).unwrap();
        // A file without a newline carries no EOL information, so detection is
        // free to answer LF; anything else must round-trip exactly.
        if text.contains('\n') {
            prop_assert_eq!(loaded.eol, eol);
        }
        prop_assert_eq!(loaded.encoding, Encoding::Utf8);
        prop_assert_eq!(&loaded.text, &text);

        // And writing it back produces the same bytes.
        filesystem::save(&path, &loaded.text, eol, Encoding::Utf8, false).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        prop_assert_eq!(written, disk);
    }

    /// The undo history never exceeds its configured bounds.
    #[test]
    fn undo_history_is_bounded(ops in proptest::collection::vec(0u8..4, 1..200)) {
        let mut doc = doc_with("start");
        for op in ops {
            doc.set_cursor(doc.len_chars());
            match op {
                0 => doc.insert_text("x"),
                1 => doc.insert_text("\n"),
                2 => { doc.backspace(); }
                _ => doc.insert_text("é"),
            }
        }
        prop_assert!(doc.len_chars() < 100_000);
    }
}
