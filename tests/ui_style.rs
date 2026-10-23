//! The two drawing styles.
//!
//! `pixel` is the default: borders and dividers are hairlines drawn on the edge
//! of a cell, so a rule costs a fraction of a row instead of a whole one.
//! `cell` is the original block-glyph look, kept behind `ui.style` — these
//! tests exist so that keeping it costs nothing: every assertion about *what*
//! the UI says is made against both styles, and only the assertions about *how*
//! it is drawn are style specific.

mod common;

use common::*;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use vela::commands::Command;
use vela::config::UiStyle;

fn pixel(dir: &std::path::Path) -> vela::app::App {
    editor_with(dir, |c| c.ui.style = "pixel".to_string())
}

fn cell(dir: &std::path::Path) -> vela::app::App {
    editor_with(dir, |c| c.ui.style = "cell".to_string())
}

/// Render one frame and hand back the cells, for assertions that need colours
/// or modifiers rather than text.
fn cells(app: &mut vela::app::App, width: u16, height: u16) -> Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| vela::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn dump(app: &mut vela::app::App, width: u16, height: u16) {
    for (i, row) in screen(app, width, height).iter().enumerate() {
        println!("{i:>2}|{row}|");
    }
}

/// The row carrying the cursor readout — the status bar, wherever it is.
fn status_index(rows: &[String]) -> usize {
    rows.iter()
        .position(|r| r.contains(", Col "))
        .expect("a status bar")
}

#[test]
fn pixel_is_the_default_and_the_old_style_is_opt_in() {
    let scratch = Scratch::new();
    assert_eq!(vela::config::Config::default().ui_style(), UiStyle::Pixel);
    let app = editor(scratch.path());
    assert_eq!(app.config.ui_style(), UiStyle::Pixel);

    let app = cell(scratch.path());
    assert_eq!(app.config.ui_style(), UiStyle::Cell);
}

#[test]
fn the_toggle_command_switches_styles_and_says_so() {
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    assert_eq!(app.config.ui_style(), UiStyle::Pixel);

    app.execute(Command::ToggleUiStyle);
    assert_eq!(app.config.ui_style(), UiStyle::Cell);
    assert!(
        screen(&mut app, 80, 24)
            .iter()
            .any(|r| r.contains("UI style: cell")),
        "the change is announced"
    );

    app.execute(Command::ToggleUiStyle);
    assert_eq!(app.config.ui_style(), UiStyle::Pixel);
}

#[test]
fn both_styles_say_the_same_things() {
    let scratch = Scratch::new();
    let path = scratch.file("hello.txt", "hello world\n");
    for mut app in [pixel(scratch.path()), cell(scratch.path())] {
        open(&mut app, &path);
        let rows = screen(&mut app, 80, 24);
        dump(&mut app, 80, 24);
        let all = rows.join("\n");
        for needle in ["VELA", "hello.txt", "hello world", "Ln 1, Col 1", "UTF-8"] {
            assert!(all.contains(needle), "{needle} missing:\n{all}");
        }
    }
}

#[test]
fn the_pixel_style_draws_hairlines_where_the_cell_style_draws_boxes() {
    let scratch = Scratch::new();

    let mut app = pixel(scratch.path());
    app.open_palette(vela::ui::PaletteMode::Commands);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    let all = rows.join("\n");
    assert!(all.contains('▔'), "the panel has a hairline lid:\n{all}");
    assert!(all.contains('▁'), "and a hairline floor:\n{all}");
    assert!(!all.contains('┌'), "no box corners:\n{all}");
    assert!(!all.contains('│'), "no box sides:\n{all}");

    let mut app = cell(scratch.path());
    app.open_palette(vela::ui::PaletteMode::Commands);
    let rows = screen(&mut app, 80, 24);
    let all = rows.join("\n");
    assert!(all.contains('┌') && all.contains('│'), "{all}");
    assert!(!all.contains('▔'), "the cell style keeps its boxes:\n{all}");
}

#[test]
fn a_panel_title_breaks_the_rule_instead_of_sitting_on_it() {
    let scratch = Scratch::new();
    let mut app = pixel(scratch.path());
    app.open_palette(vela::ui::PaletteMode::Commands);
    let buf = cells(&mut app, 80, 24);

    let panel = vela::ui::chrome::centered(ratatui::layout::Rect::new(0, 0, 80, 24), 80, 60);
    let top: String = (panel.x..panel.x + panel.width)
        .map(|x| buf[(x, panel.y)].symbol())
        .collect();

    // `▔ Commands ▔▔▔…`: the corner keeps the rule, the title interrupts it,
    // and the rule picks up again immediately after.
    assert!(top.starts_with('▔'), "top: {top:?}");
    assert!(top.contains("Commands"), "top: {top:?}");
    let after = top.find("Commands").unwrap() + "Commands".len();
    assert_eq!(
        top.chars().nth(after),
        Some('▔'),
        "the rule resumes after the title: {top:?}"
    );
    assert_eq!(
        top.chars().last(),
        Some('▔'),
        "and reaches the far corner: {top:?}"
    );
}

#[test]
fn the_status_bar_sits_under_a_hairline_in_the_pixel_style_only() {
    let scratch = Scratch::new();
    let path = scratch.file("a.txt", "one\ntwo\n");

    let mut app = pixel(scratch.path());
    open(&mut app, &path);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    let y = status_index(&rows);
    let rule = &rows[y - 1];
    assert!(
        rule.chars().all(|c| c == '▔' || c == ' '),
        "the row above the status bar is a rule: {rule:?}"
    );
    assert!(
        rule.chars().filter(|c| *c == '▔').count() >= 70,
        "and it spans the window: {rule:?}"
    );

    let mut app = cell(scratch.path());
    open(&mut app, &path);
    let rows = screen(&mut app, 80, 24);
    let y = status_index(&rows);
    assert!(
        !rows[y - 1].contains('▔'),
        "the cell style spends no row on a rule"
    );
}

#[test]
fn the_rule_between_bars_is_a_one_pixel_underline() {
    // The bar rows are full of text, and a cell holds one glyph, so the rule
    // there cannot be a dot. It is the terminal's underline attribute instead:
    // painted at the baseline, in the hairline colour, costing no row.
    let scratch = Scratch::new();
    let mut app = pixel(scratch.path());
    let buf = cells(&mut app, 80, 24);

    for (row, what) in [
        (0u16, "top bar"),
        (status_index(&screen(&mut app, 80, 24)) as u16, "status bar"),
    ] {
        let cell = &buf[(2, row)];
        assert!(
            cell.modifier.contains(Modifier::UNDERLINED),
            "the {what} is underlined"
        );
        assert_eq!(
            cell.underline_color, app.theme.line,
            "the {what}'s rule is the hairline colour, not the text colour"
        );
    }

    // And the cell style does not underline anything.
    let mut app = cell(scratch.path());
    let buf = cells(&mut app, 80, 24);
    assert!(!buf[(2, 0)].modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn the_tab_strip_is_one_row_and_tabs_are_split_by_a_hairline() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");

    let mut app = pixel(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);
    let tabs = app.tabs_area();
    assert_eq!(tabs.height, 1, "one row, always");

    let strip = &rows[tabs.y as usize];
    assert!(
        strip.contains("a.txt") && strip.contains("b.txt"),
        "{strip:?}"
    );
    assert!(
        strip.contains('▏'),
        "the tabs are split by a hairline: {strip:?}"
    );
    // The active tab is told apart by weight and colour, not by a second row.
    assert!(!strip.contains('─'), "no rule row inside the strip");

    let mut app = cell(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    let rows = screen(&mut app, 80, 24);
    let tabs = app.tabs_area();
    assert_eq!(tabs.height, 1);
    assert!(
        !rows[tabs.y as usize].contains('▏'),
        "the cell style has no divider column"
    );
}

#[test]
fn the_separator_column_belongs_to_the_tab_it_precedes() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");
    let mut app = pixel(scratch.path());
    open(&mut app, &a);
    open(&mut app, &b);
    let _ = screen(&mut app, 80, 24);

    let tabs = app.tabs_area();
    let spans = vela::ui::chrome::tab_spans(&app, tabs);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].x, 0, "the first tab starts at the edge");
    assert_eq!(
        spans[1].x, spans[0].width,
        "the second tab owns the column after the first"
    );

    // Clicking the hairline itself selects the tab it belongs to, which is what
    // a pointer anywhere inside an outline is expected to do.
    app.handle_event(crossterm::event::Event::Mouse(
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: spans[1].x,
            row: tabs.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
    ));
    assert_eq!(app.active, 1, "the separator belongs to its tab");
}

#[test]
fn the_cell_style_still_renders_every_screen() {
    // The old style is kept, not retired: every surface it draws must still be
    // reachable and intact.
    let scratch = Scratch::new();
    let mut app = cell(scratch.path());
    type_str(&mut app, "unsaved");
    ctrl(&mut app, 'q');
    let rows = screen(&mut app, 90, 30);
    dump(&mut app, 90, 30);
    let all = rows.join("\n");
    assert!(all.contains("┌ Confirm"), "{all}");
    assert!(all.contains("Discard"), "{all}");
}

#[test]
fn tiny_terminals_survive_both_styles() {
    let scratch = Scratch::new();
    for mut app in [pixel(scratch.path()), cell(scratch.path())] {
        type_str(&mut app, "some text here\nand more");
        for (w, h) in [(1, 1), (2, 2), (5, 3), (10, 4), (20, 6), (80, 1), (3, 2)] {
            let rows = screen(&mut app, w, h);
            assert_eq!(
                rows.len(),
                h as usize,
                "{w}x{h} in {:?}",
                app.config.ui_style()
            );
        }
    }
}
