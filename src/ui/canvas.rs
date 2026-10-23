//! Sub-cell drawing: a canvas whose unit is a dot, not a row.
//!
//! A terminal cell is the smallest thing a glyph can occupy, so "one pixel" has
//! to be a fraction of a cell. The block-element repertoire offers exactly four
//! usable fractions — `▔` and `▁` (top and bottom eighth), `▏` and `▕` (left and
//! right eighth) — and those four are enough for every hairline a UI needs: a
//! rule between two bars, a panel border, a divider between tabs. Each one costs
//! a fraction of a cell instead of a whole one, which is what makes a design
//! drawn with 1px borders expressible here at all.
//!
//! The canvas addresses cells in dots: a cell is two dots wide and two dots
//! tall, and a dot on the boundary of a cell is a hairline on that edge. A dot
//! in the *middle* of a cell cannot be drawn by any glyph, and asking for one
//! rounds to the nearest edge — a canvas that silently dropped the request
//! would be worse than one that quantises it.
//!
//! Two rules make composition predictable:
//!
//! * **A fill erases.** It resets the cell, so the text underneath a panel
//!   never shows through it.
//! * **Text wins.** A cell that carries a glyph cannot also carry a hairline, so
//!   [`Canvas::text`] claims its cells and later strokes skip them. That is what
//!   lets a panel title interrupt its own top border instead of being
//!   overwritten by it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Which horizontal edge of a cell a rule sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VEdge {
    Top,
    Bottom,
}

/// Which vertical edge of a cell a rule sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HEdge {
    Left,
    Right,
}

impl VEdge {
    fn glyph(self) -> &'static str {
        match self {
            VEdge::Top => "▔",
            VEdge::Bottom => "▁",
        }
    }
}

impl HEdge {
    fn glyph(self) -> &'static str {
        match self {
            HEdge::Left => "▏",
            HEdge::Right => "▕",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Horizontal(VEdge),
    Vertical(HEdge),
}

impl Edge {
    fn glyph(self) -> &'static str {
        match self {
            Edge::Horizontal(e) => e.glyph(),
            Edge::Vertical(e) => e.glyph(),
        }
    }
}

/// What has been asked of one cell.
#[derive(Debug, Clone, Copy)]
struct Mark {
    edge: Option<Edge>,
    colour: Color,
    text: bool,
}

impl Default for Mark {
    fn default() -> Self {
        Self {
            edge: None,
            colour: Color::Reset,
            text: false,
        }
    }
}

/// A drawing surface measured in dots.
pub struct Canvas<'a> {
    buf: &'a mut Buffer,
    area: Rect,
    marks: Vec<Mark>,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut Buffer, area: Rect) -> Self {
        let marks = vec![Mark::default(); area.width as usize * area.height as usize];
        Self { buf, area, marks }
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.area.x || y < self.area.y {
            return None;
        }
        let dx = x - self.area.x;
        let dy = y - self.area.y;
        if dx >= self.area.width || dy >= self.area.height {
            return None;
        }
        Some(dy as usize * self.area.width as usize + dx as usize)
    }

    /// Paint a block of cells with a background, clearing whatever was there.
    pub fn fill(&mut self, rect: Rect, bg: Color) {
        let r = rect.intersection(self.area);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                let Some(i) = self.index(x, y) else { continue };
                self.marks[i] = Mark::default();
                let cell = &mut self.buf[(x, y)];
                cell.reset();
                cell.set_style(Style::default().bg(bg));
            }
        }
    }

    /// Draw a string at cell coordinates and claim its cells.
    pub fn text(&mut self, x: u16, y: u16, text: &str, style: Style) {
        self.buf.set_string(x, y, text, style);
        for i in 0..text.chars().count() {
            if let Some(idx) = self.index(x.saturating_add(i as u16), y) {
                self.marks[idx].text = true;
            }
        }
    }

    /// A horizontal hairline across the given cells, on one edge of the row.
    pub fn hline(&mut self, x0: u16, x1: u16, y: u16, edge: VEdge, colour: Color) {
        for x in x0..x1 {
            if let Some(i) = self.index(x, y) {
                let m = &mut self.marks[i];
                if m.text {
                    continue;
                }
                m.edge = Some(Edge::Horizontal(edge));
                m.colour = colour;
            }
        }
    }

    /// A vertical hairline down the given cells, on one edge of the column.
    pub fn vline(&mut self, x: u16, y0: u16, y1: u16, edge: HEdge, colour: Color) {
        for y in y0..y1 {
            if let Some(i) = self.index(x, y) {
                let m = &mut self.marks[i];
                if m.text {
                    continue;
                }
                m.edge = Some(Edge::Vertical(edge));
                m.colour = colour;
            }
        }
    }

    /// A one-dot border around a rect of cells.
    ///
    /// The vertical edges start one row below the top and stop one row above
    /// the bottom, so the corner cells keep the horizontal rule. The seam that
    /// leaves at each corner is a single dot — a hundredth of the panel — and
    /// is invisible; a corner glyph that carried both arms would have to be a
    /// quadrant block, which is half a cell thick and would undo the point.
    pub fn stroke(&mut self, rect: Rect, colour: Color) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let r = rect.intersection(self.area);
        if r.width == 0 || r.height == 0 {
            return;
        }
        self.hline(r.x, r.right(), r.y, VEdge::Top, colour);
        if r.height > 1 {
            self.hline(r.x, r.right(), r.bottom() - 1, VEdge::Bottom, colour);
        }
        if r.height > 2 {
            self.vline(r.x, r.y + 1, r.bottom() - 1, HEdge::Left, colour);
            if r.width > 1 {
                self.vline(r.right() - 1, r.y + 1, r.bottom() - 1, HEdge::Right, colour);
            }
        }
    }

    /// Write every hairline into the buffer. Text is already in place.
    pub fn flush(&mut self) {
        for dy in 0..self.area.height {
            for dx in 0..self.area.width {
                let i = dy as usize * self.area.width as usize + dx as usize;
                let mark = self.marks[i];
                let Some(edge) = mark.edge else { continue };
                if mark.text {
                    continue;
                }
                let cell = &mut self.buf[(self.area.x + dx, self.area.y + dy)];
                cell.set_symbol(edge.glyph());
                cell.fg = mark.colour;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn a_stroke_draws_four_hairlines_and_leaves_the_corners_to_the_rules() {
        let mut buf = buffer(6, 4);
        let mut c = Canvas::new(&mut buf, Rect::new(0, 0, 6, 4));
        c.stroke(Rect::new(0, 0, 6, 4), Color::Red);
        c.flush();

        assert_eq!(row(&buf, 0), "▔▔▔▔▔▔");
        assert_eq!(row(&buf, 3), "▁▁▁▁▁▁");
        assert_eq!(row(&buf, 1), "▏    ▕");
        assert_eq!(row(&buf, 2), "▏    ▕");
    }

    #[test]
    fn text_breaks_the_border_instead_of_being_overwritten() {
        let mut buf = buffer(10, 3);
        let mut c = Canvas::new(&mut buf, Rect::new(0, 0, 10, 3));
        c.fill(Rect::new(0, 0, 10, 3), Color::Blue);
        c.text(2, 0, "Title", Style::default().fg(Color::White));
        c.stroke(Rect::new(0, 0, 10, 3), Color::Red);
        c.flush();

        assert_eq!(row(&buf, 0), "▔▔Title▔▔▔");
        // The rule picks up again after the title and still reaches the corner.
        assert_eq!(buf[(9, 0)].symbol(), "▔");
    }

    #[test]
    fn a_fill_erases_what_was_under_it() {
        let mut buf = buffer(4, 1);
        buf.set_string(0, 0, "abcd", Style::default());
        let mut c = Canvas::new(&mut buf, Rect::new(0, 0, 4, 1));
        c.fill(Rect::new(1, 0, 2, 1), Color::Blue);
        c.flush();

        assert_eq!(row(&buf, 0), "a  d");
        assert_eq!(buf[(1, 0)].bg, Color::Blue);
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
    }

    #[test]
    fn lines_are_clipped_to_the_canvas() {
        let mut buf = buffer(8, 4);
        let mut c = Canvas::new(&mut buf, Rect::new(2, 1, 3, 2));
        c.hline(0, 8, 1, VEdge::Top, Color::Red);
        c.vline(2, 2, 4, HEdge::Left, Color::Red);
        c.flush();

        assert_eq!(row(&buf, 1), "  ▔▔▔   ");
        assert_eq!(row(&buf, 2), "  ▏     ");
        assert_eq!(row(&buf, 0), "        ", "outside the canvas, untouched");
        assert_eq!(buf[(1, 1)].symbol(), " ", "clipped on the left");
        assert_eq!(buf[(5, 1)].symbol(), " ", "clipped on the right");
    }

    #[test]
    fn the_last_line_wins_on_a_shared_cell() {
        // A cross is not expressible; the later stroke decides, so a caller can
        // order its calls to say which arm matters.
        let mut buf = buffer(3, 3);
        let mut c = Canvas::new(&mut buf, Rect::new(0, 0, 3, 3));
        c.vline(1, 0, 3, HEdge::Left, Color::Red);
        c.hline(0, 3, 1, VEdge::Top, Color::Green);
        c.flush();
        assert_eq!(buf[(1, 1)].symbol(), "▔");
        assert_eq!(buf[(1, 1)].fg, Color::Green);
    }

    #[test]
    fn a_degenerate_stroke_still_draws_something() {
        let mut buf = buffer(3, 1);
        let mut c = Canvas::new(&mut buf, Rect::new(0, 0, 3, 1));
        c.stroke(Rect::new(0, 0, 3, 1), Color::Red);
        c.flush();
        assert_eq!(row(&buf, 0), "▔▔▔");

        let mut buf = buffer(1, 3);
        let mut c = Canvas::new(&mut buf, Rect::new(0, 0, 1, 3));
        c.stroke(Rect::new(0, 0, 1, 3), Color::Red);
        c.flush();
        assert_eq!(buf[(0, 1)].symbol(), "▏");
    }
}
