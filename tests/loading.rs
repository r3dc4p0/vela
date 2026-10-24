//! The loader shown while a large file is being opened.
//!
//! These tests are about what is on screen *while* work is in flight, so they
//! stop between the request and the result instead of letting it land: the
//! worker only publishes through `poll_jobs`, which the test calls by hand.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use vela::app::Loading;

/// Comfortably over the async threshold, and large enough that opening it is
/// not instantaneous even on a warm page cache.
const BIG_BYTES: usize = 6 * 1024 * 1024;

/// The loader is deliberately held back for a moment so that fast opens do not
/// flash a panel. Tests wait it out rather than asserting on a race.
const PAST_THE_DELAY: Duration = Duration::from_millis(150);

fn big_file(scratch: &Scratch, name: &str) -> PathBuf {
    let line = "the quick brown fox jumps over the lazy dog\n";
    let mut body = String::with_capacity(BIG_BYTES + line.len());
    while body.len() < BIG_BYTES {
        body.push_str(line);
    }
    scratch.file(name, &body)
}

/// Run the event loop's job pump until the open lands.
fn wait_for_open(app: &mut vela::app::App) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.loading.is_some() {
        assert!(Instant::now() < deadline, "the open never finished");
        app.poll_jobs();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn has_panel(rows: &[String]) -> bool {
    // The toast says "Opening big.txt (6.0 MiB)…" too, so match the panel's
    // top border rather than the word. Either style's border will do: the pixel
    // style cuts the title into a hairline, the cell style into a box.
    rows.iter()
        .any(|r| r.contains("▔ Opening") || r.contains("┌ Opening"))
}

/// Just the panel's own rows, from its title row to the row that closes it.
///
/// The status bar carries a percentage of its own ("Ln 1, Col 1   100%"), so
/// assertions about the bar have to look inside the box or they pass on the
/// wrong text. Picking rows by their border characters is not enough in the
/// pixel style, where the status bar also contains `▏`.
fn panel_text(rows: &[String]) -> String {
    let Some(top) = rows.iter().position(|r| r.contains(" Opening ")) else {
        return String::new();
    };
    let bottom = rows[top..]
        .iter()
        .position(|r| r.contains('└') || r.contains('▁'))
        .map_or(rows.len() - 1, |i| top + i);
    rows[top..=bottom].join("\n")
}

/// How many cells of the progress bar are actually painted.
///
/// The cell style shades the empty track with `░`; the pixel style paints it as
/// a surface, which no text dump can see, so this counts cells instead of
/// looking for characters. Either way the count is never zero while the panel
/// is up, whichever phase the worker happens to be in.
fn painted_bar(app: &mut vela::app::App, width: u16, height: u16) -> usize {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let track = app.theme.border;
    let mut n = 0;
    for y in 0..height {
        for x in 0..width {
            let cell = &buf[(x, y)];
            let filled = matches!(cell.symbol(), "█" | "░");
            let empty_track = cell.symbol() == " " && cell.bg == track;
            if filled || empty_track {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_big_file_shows_a_loader_until_it_lands() {
    let scratch = Scratch::new();
    let path = big_file(&scratch, "big.txt");
    let mut app = editor(scratch.path());

    open(&mut app, &path);
    assert!(app.loading.is_some(), "the open is on a worker");

    std::thread::sleep(PAST_THE_DELAY);
    let rows = screen(&mut app, 80, 24);
    let panel = panel_text(&rows);
    assert!(has_panel(&rows), "no loader while opening:\n{rows:#?}");
    assert!(
        panel.contains("big.txt"),
        "the panel names the file:\n{rows:#?}"
    );
    // The bar's track is always drawn; whether it carries a percentage depends
    // on which phase the worker happens to be in, which a test cannot pin down.
    assert!(
        painted_bar(&mut app, 80, 24) >= 8,
        "the panel has a bar:\n{rows:#?}"
    );

    wait_for_open(&mut app);
    assert!(app.loading.is_none());
    let expected = std::fs::metadata(&path).expect("fixture").len() as usize;
    assert_eq!(text(&app).len(), expected, "the whole file is open");
    let rows = screen(&mut app, 80, 24);
    assert!(!has_panel(&rows), "the loader is gone:\n{rows:#?}");
}

#[test]
fn a_small_file_never_shows_a_loader() {
    let scratch = Scratch::new();
    let path = scratch.file("small.txt", "hello\n");
    let mut app = editor(scratch.path());

    open(&mut app, &path);
    // Small files are read inline: there is no window in which a panel could
    // be useful, and showing one for a frame would read as a glitch.
    assert!(app.loading.is_none());
    std::thread::sleep(PAST_THE_DELAY);
    let rows = screen(&mut app, 80, 24);
    assert!(!has_panel(&rows), "{rows:#?}");
    assert_eq!(text(&app), "hello\n");
}

#[test]
fn the_bar_tracks_bytes_actually_read() {
    let scratch = Scratch::new();
    let path = scratch.file("partial.txt", "x\n");
    let mut app = editor(scratch.path());

    let loading = Arc::new(Loading::new(path, 2048));
    loading.set_done(1024);
    app.loading = Some(Arc::clone(&loading));
    std::thread::sleep(PAST_THE_DELAY);

    let rows = screen(&mut app, 80, 24);
    let panel = panel_text(&rows);
    assert!(panel.contains(" 50%"), "{rows:#?}");
    assert!(
        panel.contains("1 KiB of 2 KiB"),
        "the detail line is in the same units as the bar:\n{rows:#?}"
    );

    // Reading finished; the buffer is being built. The bar stops claiming a
    // fraction it cannot measure and sweeps instead, so 100% is never shown
    // while there is still work left.
    loading.set_done(2048);
    loading.start_preparing();
    let rows = screen(&mut app, 80, 24);
    let panel = panel_text(&rows);
    assert!(
        !panel.contains('%'),
        "no percentage once the measurable phase is over:\n{rows:#?}"
    );
    assert!(panel.contains("building the buffer"), "{rows:#?}");
}

#[test]
fn the_spinner_advances_between_frames() {
    let scratch = Scratch::new();
    let path = scratch.file("spin.txt", "x\n");
    let mut app = editor(scratch.path());

    app.loading = Some(Arc::new(Loading::new(path, 0)));
    std::thread::sleep(PAST_THE_DELAY);

    app.frame = 0;
    let first = screen(&mut app, 80, 24);
    app.frame = 2;
    let second = screen(&mut app, 80, 24);
    assert_ne!(first, second, "the spinner is what says 'still working'");
}

#[test]
fn a_second_open_supersedes_the_first() {
    let scratch = Scratch::new();
    let first = big_file(&scratch, "first.txt");
    let second = big_file(&scratch, "second.txt");
    let mut app = editor(scratch.path());

    open(&mut app, &first);
    let in_flight = app.loading.clone().expect("first open in flight");
    open(&mut app, &second);
    let current = app.loading.clone().expect("second open in flight");
    assert!(
        !Arc::ptr_eq(&in_flight, &current),
        "the loader follows the newest open"
    );

    wait_for_open(&mut app);
    // The superseded result arrives too; it must be dropped rather than
    // installed behind the user's back.
    std::thread::sleep(Duration::from_millis(50));
    app.poll_jobs();
    assert_eq!(
        app.active_doc().unwrap().path(),
        Some(second.as_path()),
        "the second file wins"
    );
}
