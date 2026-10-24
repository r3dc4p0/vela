//! The loader shown while a large file is being opened.
//!
//! Opening is not instant — a two-gigabyte file spends about five seconds
//! reading and another 1.6 s building its rope — and a window that changes
//! nothing for six seconds is indistinguishable from a hung one. This panel is
//! the difference between "working" and "dead".
//!
//! It reports *real* bytes, not a guess: `filesystem::load_with` counts them as
//! the read loop runs and the worker publishes the count through an atomic, so
//! the percentage on screen is the fraction of the file that is actually in
//! memory. The one phase that cannot be measured — turning the text into a
//! rope — says so instead of pretending, because a bar that stops moving at
//! 90% is worse than one that admits it is done reading.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::chrome::{fit, panel, Deco};
use super::App;
use crate::app::{Loading, LOADER_DELAY};
use crate::theme::Theme;

/// Braille spinner. Ten frames, the same cycle every terminal progress
/// indicator uses, so it reads as motion at a glance.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Panel geometry: wide enough for a name and a percentage, and short enough to
/// sit in the middle of a 24-row terminal without crowding the file behind it.
const PANEL_WIDTH: u16 = 52;
const PANEL_HEIGHT: u16 = 5;

/// Below this the panel would be all border and no content.
const MIN_WIDTH: u16 = 24;

/// Draw the loader over the editing area, if one is in flight.
pub fn draw(buf: &mut Buffer, area: Rect, app: &App) {
    let Some(loading) = app.loading.as_ref() else {
        return;
    };
    // An overlay means the user is typing somewhere else; a panel underneath it
    // would be in the way of what they are actually doing.
    if !app.ui.overlay.is_none() {
        return;
    }
    // Work that finishes before this has been read is not worth a flash.
    if loading.elapsed() < LOADER_DELAY {
        return;
    }
    if area.width < MIN_WIDTH || area.height < PANEL_HEIGHT {
        return;
    }

    let rect = centered(area, PANEL_WIDTH.min(area.width), PANEL_HEIGHT);
    let deco = Deco::for_app(app);
    let inner = panel(buf, rect, "Opening", &app.theme, deco);
    if inner.height < 3 {
        return;
    }
    draw_body(buf, inner, &app.theme, deco, loading, app.frame);
}

fn draw_body(
    buf: &mut Buffer,
    inner: Rect,
    theme: &Theme,
    deco: Deco,
    loading: &Loading,
    frame: u64,
) {
    let width = inner.width as usize;

    // Row 0: spinner, then the file's name.
    let glyph = SPINNER[(frame / 2) as usize % SPINNER.len()];
    buf.set_string(
        inner.x,
        inner.y,
        glyph,
        Style::default()
            .fg(theme.statusbar_accent)
            .add_modifier(Modifier::BOLD),
    );
    buf.set_string(
        inner.x + 2,
        inner.y,
        fit(&loading.name(), width.saturating_sub(2)),
        Style::default().fg(theme.fg),
    );

    // Row 1: the bar and the percentage. The percentage is part of the row, so
    // the bar gives up six cells for it rather than being overdrawn.
    let bar = match (loading.preparing(), loading.fraction()) {
        (true, _) => Bar::Indeterminate,
        (false, Some(f)) => Bar::Fraction(f),
        (false, None) => Bar::Indeterminate,
    };
    let bar_w = width.saturating_sub(6);
    if bar_w >= 8 {
        // The pixel style paints the track as a surface and the progress as a
        // solid block on it, so the bar reads as a filled rectangle rather than
        // as a run of shaded characters.
        let (filled, empty) = if deco.solid_track {
            ("█", " ")
        } else {
            ("█", "░")
        };
        let empty_bg = if deco.solid_track {
            theme.border
        } else {
            theme.overlay_bg
        };
        for (i, done) in bar_cells(bar, bar_w, frame).iter().enumerate() {
            let (glyph, fg, bg) = if *done {
                (filled, theme.statusbar_accent, theme.overlay_bg)
            } else {
                (empty, theme.border, empty_bg)
            };
            buf[(inner.x + i as u16, inner.y + 1)]
                .set_symbol(glyph)
                .set_style(Style::default().fg(fg).bg(bg));
        }
        if let Bar::Fraction(f) = bar {
            buf.set_string(
                inner.x + bar_w as u16 + 1,
                inner.y + 1,
                format!("{:>3}%", percent(f)),
                Style::default().fg(theme.dim),
            );
        }
    }

    // Row 2: what is happening, in the same units as the bar.
    let detail = match (loading.preparing(), loading.fraction().is_some()) {
        // Reading is over. The bar sweeps instead of claiming a fraction of
        // work nobody measured.
        (true, _) => format!("{} read · building the buffer…", human_bytes(loading.total)),
        (false, true) => format!(
            "{} of {}",
            human_bytes(loading.done()),
            human_bytes(loading.total)
        ),
        (false, false) => "reading…".to_string(),
    };
    buf.set_string(
        inner.x,
        inner.y + 2,
        fit(&detail, width),
        Style::default().fg(theme.dim),
    );
}

/// What the bar can honestly say.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bar {
    /// A measured fraction of the file.
    Fraction(f32),
    /// No fraction is available: either the size is unknown or the work left
    /// is a phase that reports nothing.
    Indeterminate,
}

/// Which cells of a `width`-wide bar are filled.
///
/// Separated from the painting so the arithmetic — rounding, the empty bar, the
/// full bar, the sweep — can be tested without a terminal.
fn bar_cells(bar: Bar, width: usize, frame: u64) -> Vec<bool> {
    match bar {
        Bar::Fraction(f) => {
            let filled = (f.clamp(0.0, 1.0) * width as f32).round() as usize;
            (0..width).map(|i| i < filled).collect()
        }
        Bar::Indeterminate => {
            // A short block that sweeps in from the left and out to the right.
            // It says "alive, unknown duration" without ever implying a
            // proportion, which is the one thing a stalled bar cannot do.
            const BLOCK: usize = 3;
            let head = (frame as usize * 2) % (width + BLOCK).max(1);
            (0..width).map(|i| i < head && i + BLOCK >= head).collect()
        }
    }
}

fn percent(fraction: f32) -> u32 {
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// A byte count as a short human string: `12 B`, `640 KiB`, `2.0 GiB`.
fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let n = n as f64;
    if n < KIB {
        return format!("{n:.0} B");
    }
    if n < KIB * KIB {
        return format!("{:.0} KiB", n / KIB);
    }
    if n < KIB * KIB * KIB {
        return format!("{:.1} MiB", n / (KIB * KIB));
    }
    format!("{:.1} GiB", n / (KIB * KIB * KIB))
}

/// A fixed-size rectangle in the middle of `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_fills_from_the_left_in_proportion() {
        assert_eq!(bar_cells(Bar::Fraction(0.0), 4, 0), vec![false; 4]);
        assert_eq!(bar_cells(Bar::Fraction(1.0), 4, 0), vec![true; 4]);
        assert_eq!(
            bar_cells(Bar::Fraction(0.5), 4, 0),
            vec![true, true, false, false]
        );
        // Rounds rather than truncating: half a cell is not a cell.
        let cells = bar_cells(Bar::Fraction(0.26), 10, 0);
        assert_eq!(cells.iter().filter(|c| **c).count(), 3);
    }

    #[test]
    fn fractions_are_clamped_before_they_reach_the_screen() {
        // A file that grew while it was being read reports more bytes than its
        // metadata said; the bar must not overflow the panel.
        assert_eq!(bar_cells(Bar::Fraction(1.4), 4, 0), vec![true; 4]);
        assert_eq!(percent(1.4), 100);
        assert_eq!(percent(-0.2), 0);
    }

    #[test]
    fn the_indeterminate_bar_sweeps_without_claiming_a_fraction() {
        let cells = |frame| bar_cells(Bar::Indeterminate, 10, frame);
        let filled = |frame| cells(frame).iter().filter(|c| **c).count();

        assert_eq!(filled(0), 0, "starts off the left edge");
        assert_eq!(filled(3), 3, "fully inside");
        assert!(filled(6) < 3, "on its way out");
        // Never wider than the block, and never outside the track.
        for frame in 0..40 {
            let row = cells(frame);
            assert_eq!(row.len(), 10);
            assert!(row.iter().filter(|c| **c).count() <= 3);
        }
    }

    #[test]
    fn the_sweep_never_stalls() {
        // Two consecutive frames must differ somewhere: a bar that stops moving
        // is indistinguishable from a hung editor.
        let a = bar_cells(Bar::Indeterminate, 12, 4);
        let b = bar_cells(Bar::Indeterminate, 12, 5);
        assert_ne!(a, b);
    }

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1 KiB");
        assert_eq!(human_bytes(1536 * 1024), "1.5 MiB");
        assert_eq!(human_bytes(2052 * 1024 * 1024), "2.0 GiB");
    }
}
