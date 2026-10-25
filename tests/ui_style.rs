//! The two drawing styles.
//!
//! `pixel` is the default: surfaces carry the structure, hairlines are spent
//! where they cost a column (dividers) or a panel's own border rows, and no row
//! is spent on a rule. `cell` is the original block-glyph chrome, kept behind
//! `ui.style` so that keeping it costs nothing.
//!
//! Every assertion about *what* the UI says is made against both styles; only
//! the assertions about *how* it is drawn are style specific.

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
    assert_eq!(editor(scratch.path()).config.ui_style(), UiStyle::Pixel);
    assert_eq!(cell(scratch.path()).config.ui_style(), UiStyle::Cell);
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
        for needle in [
            "VELA",
            "hello.txt",
            "hello world",
            "Ln 1, Col 1",
            "UTF-8",
            "Ctrl+s",
        ] {
            assert!(all.contains(needle), "{needle} missing:\n{all}");
        }
    }
}

#[test]
fn the_pixel_style_frames_panels_with_hairlines_where_the_cell_style_boxes_them() {
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
    // and the rule picks up again where the title ends.
    assert!(top.starts_with('▔'), "top: {top:?}");
    assert!(top.contains("Commands"), "top: {top:?}");
    let after = top.find("Commands").unwrap() + "Commands".len() + 1;
    assert_eq!(
        top.chars().nth(after),
        Some('▔'),
        "the rule resumes after the title's padding: {top:?}"
    );
    assert_eq!(
        top.chars().last(),
        Some('▔'),
        "and reaches the far corner: {top:?}"
    );
}

#[test]
fn no_row_is_spent_on_a_rule_and_no_bar_is_underlined() {
    // The two things the first attempt got wrong: a full row that painted only
    // its top eighth, and a bar whose text was underlined to fake a border.
    let scratch = Scratch::new();
    let path = scratch.file("a.txt", "one\ntwo\n");
    let mut app = pixel(scratch.path());
    open(&mut app, &path);
    let rows = screen(&mut app, 80, 24);
    dump(&mut app, 80, 24);

    let editor = app.editor_area();
    let status = status_index(&rows);
    assert_eq!(
        editor.y + editor.height,
        status as u16,
        "the status bar starts where the editor ends: no row in between"
    );
    assert_eq!(
        app.tabs_area().height,
        0,
        "one buffer, so no tab strip either"
    );

    let buf = cells(&mut app, 80, 24);
    for y in 0..24 {
        for x in 0..80 {
            assert!(
                !buf[(x, y)].modifier.contains(Modifier::UNDERLINED),
                "nothing is underlined, at ({x}, {y})"
            );
        }
    }
}

#[test]
fn the_tab_strip_is_one_row_with_a_divider_before_every_tab() {
    let scratch = Scratch::new();
    let a = scratch.file("a.txt", "a\n");
    let b = scratch.file("b.txt", "b\n");

    // Both styles draw the divider: a tab is a container, and the design draws
    // its edge as a rule. Only the panels and the bars differ between them.
    for mut app in [pixel(scratch.path()), cell(scratch.path())] {
        open(&mut app, &a);
        open(&mut app, &b);
        let rows = screen(&mut app, 80, 24);
        let tabs = app.tabs_area();
        assert_eq!(tabs.height, 1, "one row, always");

        let strip = &rows[tabs.y as usize];
        assert!(
            strip.contains("a.txt") && strip.contains("b.txt"),
            "{strip:?}"
        );
        assert_eq!(
            strip.matches('▏').count(),
            2,
            "every tab, including the first, owns a divider: {strip:?}"
        );
        assert!(!strip.contains('─'), "no rule row inside the strip");
        assert!(!strip.contains('│'), "and no cell of line either");
    }
}

#[test]
fn the_divider_belongs_to_the_tab_it_precedes() {
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

    // Clicking the divider selects the tab it belongs to, which is what a
    // pointer anywhere inside an outline is expected to do.
    app.handle_event(crossterm::event::Event::Mouse(
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: spans[1].x,
            row: tabs.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
    ));
    assert_eq!(app.active(), 1, "the divider belongs to its tab");
}

#[test]
fn the_gutter_rule_is_paid_for_by_the_pixel_style_alone() {
    // The pixel style spends one more column on the gutter than the block style:
    // the rule, and the air after it. The block style is left exactly as wide as
    // it was, which is the whole point of keeping it.
    let scratch = Scratch::new();
    let path = scratch.file("a.txt", "one\ntwo\nthree\n");

    let mut widths = Vec::new();
    for mut app in [pixel(scratch.path()), cell(scratch.path())] {
        open(&mut app, &path);
        let rows = screen(&mut app, 80, 24);
        dump(&mut app, 80, 24);
        let row = rows.iter().find(|r| r.contains("one")).expect("text");
        // `find` returns a byte offset, and the gutter rule is three bytes wide
        // in UTF-8, so the column has to be counted in characters.
        let byte = row.find("one").expect("the text starts somewhere");
        let col = row[..byte].chars().count() as u16;
        let gutter = {
            let doc = app.active_doc().expect("a buffer");
            vela::ui::editor::gutter_width(doc.line_count(), &app.config)
        };
        assert_eq!(
            app.editor_area().x + gutter,
            col,
            "the text starts exactly where the gutter ends"
        );
        widths.push(col);
    }
    assert_eq!(
        widths[0],
        widths[1] + 2,
        "the rule and the air after it are the only extra columns"
    );
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
