//! Splitting the editing area into panes.
//!
//! A pane is a *view*: which buffer it shows, where in that buffer it is
//! looking, and the rectangle it was given in the last frame. The buffer itself
//! stays in `App::buffers`, shared by every pane that shows it — so two panes
//! on one file are two windows onto one document, and an edit in either shows
//! up in both.
//!
//! The viewport (`top`, `row`, `left`, `detached`) is the interesting part.
//! `Document` carries the same four fields, and they are the *staging area* for
//! whichever pane is being drawn: the renderer lays a pane out by adjusting
//! them, so `ui::draw` installs the pane's viewport into the document before
//! rendering it and harvests it back afterwards. Everything that is not the
//! draw pass — motions, the mouse, the status bar — reads the active pane's
//! viewport, which `ui::draw` leaves installed when it finishes. Without that
//! hand-off, two panes on one buffer would fight over one set of scroll
//! offsets, and the second pane drawn would win.

use ratatui::layout::Rect;

use super::App;

/// One view onto a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pane {
    /// Index into [`App::buffers`].
    pub buffer: usize,
    /// First visible line.
    pub top: usize,
    /// First visible visual row within `top` (only ever set with word wrap on).
    pub row: usize,
    /// First visible display column.
    pub left: usize,
    /// True while the view is scrolled away from the cursor.
    pub detached: bool,
    /// The rectangle this pane was given in the last frame.
    pub rect: Rect,
}

impl Pane {
    pub fn new(buffer: usize) -> Self {
        Self {
            buffer,
            top: 0,
            row: 0,
            left: 0,
            detached: false,
            rect: Rect::default(),
        }
    }

    /// A pane showing the same buffer from the same place, for a split.
    fn mirrored(&self) -> Self {
        Self {
            buffer: self.buffer,
            top: self.top,
            row: self.row,
            left: self.left,
            detached: self.detached,
            rect: Rect::default(),
        }
    }
}

/// The narrowest a pane may become. Below this the gutter and a few columns of
/// text are all that fits, and a split would be a worse view than no split.
pub const MIN_PANE_WIDTH: u16 = 24;

impl App {
    /// Index of the buffer the active pane shows.
    ///
    /// This is derived, never stored: `active` used to be a field kept in step
    /// with the buffer list, and a pane model makes that a second source of
    /// truth for the same fact.
    pub fn active(&self) -> usize {
        self.panes
            .get(self.active_pane)
            .map_or(0, |p| p.buffer.min(self.buffers.len().saturating_sub(1)))
    }

    /// Make sure there is a pane to draw into, and that every pane points at a
    /// buffer that exists.
    ///
    /// Called once the buffers are settled at startup and after anything that
    /// can remove one.
    pub fn ensure_panes(&mut self) {
        let last = self.buffers.len().saturating_sub(1);
        self.panes.retain(|p| p.buffer <= last);
        if self.panes.is_empty() {
            self.panes.push(Pane::new(0));
            self.active_pane = 0;
        }
        if self.active_pane >= self.panes.len() {
            self.active_pane = self.panes.len() - 1;
        }
        self.install_active_view();
    }

    /// The pane the keyboard is talking to.
    pub fn active_pane(&self) -> Option<&Pane> {
        self.panes.get(self.active_pane)
    }

    /// How many panes are on screen.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// The whole editing area, panes and their dividers together.
    ///
    /// `editor_area()` is the *active* pane's rect, which is what the mouse and
    /// the motions need; a split has to know about all of them.
    pub fn editor_span(&self) -> Rect {
        let mut rects = self.panes.iter().map(|p| p.rect).filter(|r| r.width > 0);
        let Some(first) = rects.next() else {
            return Rect::default();
        };
        rects.fold(first, |acc, r| {
            let x = acc.x.min(r.x);
            let y = acc.y.min(r.y);
            let right = (acc.x + acc.width).max(r.x + r.width);
            let bottom = (acc.y + acc.height).max(r.y + r.height);
            Rect::new(x, y, right - x, bottom - y)
        })
    }

    /// The pane containing a screen position, if any.
    pub fn pane_at(&self, column: u16, row: u16) -> Option<usize> {
        self.panes
            .iter()
            .position(|p| p.rect.width > 0 && p.rect.contains((column, row).into()))
    }

    /// Make `index` the pane the keyboard talks to.
    ///
    /// Returns false for an index that does not exist, so a caller can use it
    /// as a guard rather than checking first.
    pub fn focus_pane(&mut self, index: usize) -> bool {
        if index >= self.panes.len() {
            return false;
        }
        // Keep whatever the document learned about the pane being left: between
        // a motion and the next frame the document is the fresher of the two.
        self.harvest_view(self.active_pane);
        self.active_pane = index;
        // The pane's own viewport is now the one every non-draw reader sees.
        self.install_active_view();
        true
    }

    /// Focus the pane under a screen position, and point the coordinate
    /// mapping at it.
    ///
    /// Returns false when the point is not over a pane, which is the case for
    /// the divider column: a click there belongs to the pane it introduces, so
    /// the caller maps it against that pane rather than doing nothing.
    pub fn focus_pane_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.pane_at(column, row) else {
            return false;
        };
        self.last_editor_area = self.panes[index].rect;
        if index != self.active_pane {
            self.focus_pane(index);
        }
        true
    }

    /// Split the active pane, showing the same buffer from the same place in a
    /// new pane to its right.
    ///
    /// The new pane takes the focus: a split the user did not ask to type in is
    /// a split they have to undo.
    pub fn split_pane(&mut self) -> bool {
        let Some(active) = self.panes.get(self.active_pane).copied() else {
            return false;
        };
        // Refuse when the panes would be too narrow to read. The check uses the
        // width of the area the panes share, not of one pane, because the
        // layout divides that area evenly.
        let count = self.panes.len() + 1;
        let width = self.editor_span().width;
        let each = width.saturating_sub(count as u16 - 1) / count as u16;
        if each < MIN_PANE_WIDTH {
            self.warn("No room for another pane — widen the window");
            return false;
        }
        let pane = active.mirrored();
        self.panes.insert(self.active_pane + 1, pane);
        self.active_pane += 1;
        self.install_active_view();
        true
    }

    /// Close the active pane, unless it is the last one.
    pub fn close_pane(&mut self) -> bool {
        if self.panes.len() < 2 {
            self.warn("The last pane cannot be closed");
            return false;
        }
        self.panes.remove(self.active_pane);
        if self.active_pane >= self.panes.len() {
            self.active_pane = self.panes.len() - 1;
        }
        self.install_active_view();
        true
    }

    /// Move the focus one pane left or right, wrapping at the ends.
    pub fn focus_pane_step(&mut self, delta: isize) {
        if self.panes.len() < 2 {
            return;
        }
        let n = self.panes.len() as isize;
        let next = (self.active_pane as isize + delta).rem_euclid(n) as usize;
        self.focus_pane(next);
    }

    /// Point the active pane at another buffer.
    ///
    /// The viewport resets, because it described the buffer being left: keeping
    /// it would drop the caret into an unrelated part of the new file.
    pub fn set_active_buffer(&mut self, index: usize) {
        if index >= self.buffers.len() {
            return;
        }
        if let Some(pane) = self.panes.get_mut(self.active_pane) {
            if pane.buffer != index {
                pane.buffer = index;
                pane.top = 0;
                pane.row = 0;
                pane.left = 0;
                pane.detached = false;
            }
        }
        self.install_active_view();
    }

    /// Fix the panes after buffer `removed` left the list.
    ///
    /// Panes showing it move to its neighbour — the buffer before it, or the
    /// first one — and every later index shifts down to match the list.
    pub fn after_buffer_removed(&mut self, removed: usize) {
        // The diff view holds buffer indices, so it has to be told as well:
        // closing one of the two documents it compares closes the view, since
        // a comparison against a buffer that no longer exists has nothing to
        // draw.
        if let Some(view) = &mut self.diff {
            if view.left == removed || view.right == removed {
                self.diff = None;
            } else {
                if view.left > removed {
                    view.left -= 1;
                }
                if view.right > removed {
                    view.right -= 1;
                }
            }
        }
        let last = self.buffers.len().saturating_sub(1);
        let fallback = removed.saturating_sub(1).min(last);
        for pane in &mut self.panes {
            if pane.buffer == removed {
                pane.buffer = fallback;
                pane.top = 0;
                pane.row = 0;
                pane.left = 0;
                pane.detached = false;
            } else if pane.buffer > removed {
                pane.buffer -= 1;
            }
        }
    }

    /// Text columns a pane has to work with.
    fn pane_text_width(&self, pane: usize) -> usize {
        let Some(entry) = self.panes.get(pane) else {
            return 0;
        };
        let Some(doc) = self.buffers.get(entry.buffer) else {
            return 0;
        };
        (entry.rect.width as usize).saturating_sub(self.gutter_width(doc) as usize)
    }

    /// The wheel scrolls the pane it is over, not the one with the keyboard.
    ///
    /// Focus does not move: scrolling is looking, and stealing the keyboard
    /// from the pane being typed in because the pointer drifted over its
    /// neighbour would be worse than the bug this fixes.
    pub fn wheel_vertical_at(&mut self, column: u16, row: u16, delta: isize) {
        let Some(pane) = self.pane_at(column, row) else {
            return self.scroll_by(delta);
        };
        if pane == self.active_pane {
            return self.scroll_by(delta);
        }
        let Some(entry) = self.panes.get(pane).copied() else {
            return;
        };
        let wrap = if self.config.editor.word_wrap {
            let width = self.pane_text_width(pane);
            (width > 0).then_some(width)
        } else {
            None
        };
        self.install_view(pane);
        self.scroll_vertical(entry.buffer, wrap, delta);
        self.harvest_view(pane);
        self.install_active_view();
    }

    /// The horizontal wheel, on the pane it is over.
    pub fn wheel_horizontal_at(&mut self, column: u16, row: u16, delta: isize) {
        let Some(pane) = self.pane_at(column, row) else {
            return self.scroll_sideways(delta);
        };
        if pane == self.active_pane {
            return self.scroll_sideways(delta);
        }
        if self.config.editor.word_wrap {
            return;
        }
        let Some(entry) = self.panes.get(pane).copied() else {
            return;
        };
        let width = self.pane_text_width(pane);
        self.install_view(pane);
        self.scroll_horizontal(entry.buffer, width, delta);
        self.harvest_view(pane);
        self.install_active_view();
    }

    /// Copy the active pane's viewport into its document.
    ///
    /// The draw pass does this for every pane before rendering it; this is the
    /// same thing for the code that runs between frames.
    pub fn install_active_view(&mut self) {
        self.install_view(self.active_pane);
    }

    /// Copy one pane's viewport into the document it shows.
    pub fn install_view(&mut self, pane: usize) {
        let Some(entry) = self.panes.get(pane).copied() else {
            return;
        };
        if let Some(doc) = self.buffers.get_mut(entry.buffer) {
            doc.set_view_top(entry.top, entry.row);
            doc.set_scroll_left(entry.left);
            if entry.detached {
                doc.detach_view();
            } else {
                doc.attach_view();
            }
        }
    }

    /// Copy a document's viewport back into the pane that owns it.
    ///
    /// Called after the pane has been rendered, because the renderer is what
    /// decides where the viewport ended up.
    pub fn harvest_view(&mut self, pane: usize) {
        let Some(entry) = self.panes.get(pane) else {
            return;
        };
        let buffer = entry.buffer;
        let Some(doc) = self.buffers.get(buffer) else {
            return;
        };
        let (top, row, left, detached) = (
            doc.scroll_top(),
            doc.scroll_row(),
            doc.scroll_left(),
            doc.view_detached(),
        );
        let entry = &mut self.panes[pane];
        entry.top = top;
        entry.row = row;
        entry.left = left;
        entry.detached = detached;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirrored_pane_starts_where_its_parent_looks() {
        let pane = Pane {
            buffer: 3,
            top: 40,
            row: 2,
            left: 8,
            detached: true,
            rect: Rect::new(0, 0, 10, 10),
        };
        let twin = pane.mirrored();
        assert_eq!((twin.buffer, twin.top, twin.row, twin.left), (3, 40, 2, 8));
        assert!(twin.detached, "the split inherits the scroll gesture too");
        assert_eq!(
            twin.rect,
            Rect::default(),
            "the new pane is laid out next frame"
        );
    }
}
