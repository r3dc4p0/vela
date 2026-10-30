//! The diff view: two documents side by side, scrolled as one.
//!
//! This is a *view mode*, not a third kind of buffer. It holds no text of its
//! own — only the row list from [`crate::diff`] and where it is scrolled to —
//! and it borrows the two documents that are already open. That is what makes
//! "sync scrolling" fall out of the design rather than having to be arranged:
//! there is one scroll offset, and both columns are drawn from it.
//!
//! The alignment is the reason a diff is readable at all. Two files with an
//! inserted line are one line out of step from the insertion onwards, and a
//! view that scrolled both by the same *line* would show them misaligned for
//! the rest of the file. Here a row is a *pair* of lines, and a row with one
//! side missing is drawn as a blank — so the columns stay in step because the
//! rows do.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::buffer::Document;
use crate::diff::{self, Stats};

use super::App;

/// One open comparison.
#[derive(Debug)]
pub struct DiffView {
    /// Buffer index of the left-hand document.
    pub left: usize,
    /// Buffer index of the right-hand document.
    pub right: usize,
    pub rows: Vec<diff::Row>,
    pub stats: Stats,
    /// Content lines on each side, for the gutters.
    pub left_lines: usize,
    pub right_lines: usize,
    /// File names for the header.
    pub left_label: String,
    pub right_label: String,
    /// First visible row.
    pub scroll: usize,
    /// Rows the last frame could show, below the header.
    ///
    /// Published by the draw pass, like `Pane::rect`: the scroll keys need to
    /// know what a page is, and only the renderer knows how much room it had.
    pub height: usize,
}

impl DiffView {
    /// The furthest the view can scroll and still be full of rows.
    pub fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.height.max(1))
    }

    /// Every maximal run of changed rows, as row ranges.
    ///
    /// This is what "next change" steps through, and what the header counts.
    pub fn blocks(&self) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            if !row.kind.is_change() {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.end == i => last.end = i + 1,
                _ => out.push(i..i + 1),
            }
        }
        out
    }

    /// Which change the view is showing, 1-based, and how many there are.
    ///
    /// Answers "how far through the changes am I" without the reader counting
    /// marks in the gutter.
    pub fn position(&self) -> Option<(usize, usize)> {
        let blocks = self.blocks();
        if blocks.is_empty() {
            return None;
        }
        let bottom = self.scroll + self.height.max(1);
        // The change in view, not the one above it: scrolling to a block lands
        // it a couple of rows below the top edge, and a counter that still said
        // "the previous one" would be wrong on the screen the reader is
        // looking at.
        let here = blocks
            .iter()
            .position(|b| b.start < bottom && b.end > self.scroll)
            // Nothing in view: the last one above, or the first one if the
            // view is above them all.
            .or_else(|| blocks.iter().rposition(|b| b.start <= self.scroll))
            .unwrap_or(0);
        Some((here + 1, blocks.len()))
    }
}

impl App {
    // ------------------------------------------------------------- opening

    /// Compare the active buffer with buffer `other`.
    pub fn compare_with_buffer(&mut self, other: usize) -> bool {
        self.start_diff(self.active(), other)
    }

    /// Compare buffer `left` with buffer `right`.
    ///
    /// Returns false — with a message — when there is nothing to compare: the
    /// same buffer twice, an index that does not exist, or a pair too large to
    /// diff without freezing the window.
    pub fn start_diff(&mut self, left: usize, right: usize) -> bool {
        if left == right {
            self.warn("The same buffer cannot be compared with itself");
            return false;
        }
        if left >= self.buffers.len() || right >= self.buffers.len() {
            return false;
        }
        let Some(found) =
            diff::compare(self.buffers[left].storage(), self.buffers[right].storage())
        else {
            self.warn(format!(
                "Too large to compare — the limit is {} MiB of text",
                diff::MAX_BYTES / (1024 * 1024)
            ));
            return false;
        };

        let stats = found.stats;
        let view = DiffView {
            left,
            right,
            // Open on the first change, with a couple of lines of context: the
            // difference is why the view was opened, and starting at line 1
            // would make the reader scroll to find it.
            scroll: found
                .rows
                .iter()
                .position(|r| r.kind.is_change())
                .map_or(0, |row| row.saturating_sub(2)),
            rows: found.rows,
            stats,
            left_lines: diff::content_lines(self.buffers[left].storage()),
            right_lines: diff::content_lines(self.buffers[right].storage()),
            left_label: label_for(&self.buffers[left]),
            right_label: label_for(&self.buffers[right]),
            height: 0,
        };
        self.diff = Some(view);

        if stats.is_same() {
            self.success("The two files are identical");
        } else {
            self.info(format!(
                "{} change{}: +{} −{} ~{}",
                stats.blocks,
                if stats.blocks == 1 { "" } else { "s" },
                stats.added,
                stats.removed,
                stats.changed
            ));
        }
        true
    }

    pub fn diff_open(&self) -> bool {
        self.diff.is_some()
    }

    pub fn close_diff(&mut self) {
        if self.diff.take().is_some() {
            self.info("Diff closed");
        }
    }

    // ------------------------------------------------------------ scrolling

    /// Scroll the view by `delta` rows.
    pub fn diff_scroll(&mut self, delta: isize) {
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        let max = view.max_scroll() as isize;
        view.scroll = (view.scroll as isize + delta).clamp(0, max) as usize;
    }

    /// Scroll to a row, clamped to the end.
    pub fn diff_scroll_to(&mut self, row: usize) {
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        view.scroll = row.min(view.max_scroll());
    }

    /// Jump to the next or previous change.
    ///
    /// A change taller than the view is stepped *through* rather than skipped:
    /// "next" from inside one moves a page down within it, and only leaves it
    /// once its end is on screen. Otherwise a hundred-line block would be one
    /// keystroke and most of it never seen.
    pub fn diff_step_change(&mut self, forward: bool) {
        let Some(view) = self.diff.as_ref() else {
            return;
        };
        let blocks = view.blocks();
        if blocks.is_empty() {
            self.info("No changes");
            return;
        }
        let top = view.scroll;
        let height = view.height.max(1);
        let bottom = top + height;
        // Two rows of context above the block, where the eye expects it.
        let lead = 2usize;

        if forward {
            let inside = blocks.iter().find(|b| b.start <= top && top < b.end);
            if let Some(block) = inside {
                if block.end > bottom {
                    self.diff_scroll_to((top + height).min(block.end));
                    return;
                }
            }
            // `top + lead` rather than `top`: the view opens two rows above
            // the first change, so the block it is showing starts just below
            // the top edge — and "next" means the one after that, not a
            // re-centring of the same one.
            let next = blocks
                .iter()
                .find(|b| b.start > top + lead)
                .or_else(|| blocks.first());
            if let Some(block) = next {
                self.diff_scroll_to(block.start.saturating_sub(lead));
            }
        } else {
            let inside = blocks.iter().find(|b| b.start <= top && top < b.end);
            if let Some(block) = inside {
                // Scrolled into the middle of a block: go back to its top
                // before leaving it.
                if block.start < top {
                    self.diff_scroll_to(block.start.saturating_sub(lead));
                    return;
                }
            }
            let previous = blocks
                .iter()
                .rev()
                .find(|b| b.end <= top)
                .or_else(|| blocks.last());
            if let Some(block) = previous {
                self.diff_scroll_to(block.start.saturating_sub(lead));
            }
        }
    }

    /// Handle a key while the diff view is open.
    ///
    /// Returns true when the key was used. The view is read-only, so anything
    /// that would edit the buffer underneath it is swallowed rather than
    /// allowed to change a document the user cannot see.
    pub fn handle_diff_key(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;
        let page = self.diff.as_ref().map_or(10, |v| v.height.max(2) - 1);
        match key.code {
            KeyCode::Esc => {
                self.close_diff();
                true
            }
            KeyCode::Up => {
                self.diff_scroll(-1);
                true
            }
            KeyCode::Down => {
                self.diff_scroll(1);
                true
            }
            KeyCode::PageUp => {
                self.diff_scroll(-(page as isize));
                true
            }
            KeyCode::PageDown => {
                self.diff_scroll(page as isize);
                true
            }
            KeyCode::Home => {
                self.diff_scroll_to(0);
                true
            }
            KeyCode::End => {
                let last = self.diff.as_ref().map_or(0, |v| v.max_scroll());
                self.diff_scroll_to(last);
                true
            }
            // `]` and `[` are keys only *here*: bound in the keymap they would
            // stop being characters you can type.
            KeyCode::Char(']') => {
                self.diff_step_change(true);
                true
            }
            KeyCode::Char('[') => {
                self.diff_step_change(false);
                true
            }
            _ => false,
        }
    }

    /// Ask which file to compare the active buffer with.
    pub fn compare_prompt(&mut self) {
        let Some(doc) = self.active_doc() else {
            return;
        };
        if doc.path().is_none() {
            self.warn("Save the buffer first — there is no file to compare");
            return;
        }
        self.open_palette(crate::ui::PaletteMode::Compare);
    }

    /// Compare the active buffer with a path, opening it first if needed.
    pub fn compare_with_path(&mut self, path: PathBuf) {
        let path = self.resolve(&path);
        let left = self.active();
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.path().map(|p| same_path(p, &path)).unwrap_or(false))
        {
            self.start_diff(left, idx);
            return;
        }
        // Not open yet. A large file is loaded on a worker, so the comparison
        // starts when the buffer lands rather than here — see `files::install`.
        self.pending_compare = Some((left, path.clone()));
        self.open_path(path);
    }

    /// Start a comparison that was waiting for its file to finish loading.
    pub(crate) fn finish_pending_compare(&mut self, path: &Path, opened: usize) {
        let waiting = self
            .pending_compare
            .as_ref()
            .is_some_and(|(_, want)| same_path(want, path));
        if !waiting {
            return;
        }
        let Some((left, _)) = self.pending_compare.take() else {
            return;
        };
        // The blank buffer `left` pointed at may have been the one the file
        // replaced; comparing it with itself is not a comparison.
        if left == opened {
            self.warn("There was no file open to compare with");
            return;
        }
        self.start_diff(left, opened);
    }
}

/// Name to show in the header.
fn label_for(doc: &Document) -> String {
    doc.path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string())
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
