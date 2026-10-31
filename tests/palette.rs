//! The command palette is a menu with sections.
//!
//! Grouping is only useful if it is navigable: a heading that can be selected
//! or run is worse than no heading at all, so these check the walking as much
//! as the drawing.

mod common;

use common::*;
use crossterm::event::KeyCode;
use vibe::app::App;
use vibe::ui::{Overlay, PaletteAction, PaletteMode};

/// An editor with the command palette open.
fn palette(scratch: &Scratch) -> App {
    let mut app = editor(scratch.path());
    app.open_palette(PaletteMode::Commands);
    app
}

/// Rows that are a group separator.
///
/// The panel's own frame uses `▔`/`▁` and the field's rule is a single row of
/// `▁`, so three `─` in a row only ever come from a heading.
fn headings(rows: &[String]) -> Vec<&str> {
    rows.iter()
        .map(|r| r.as_str())
        .filter(|r| r.contains("───"))
        .collect()
}

#[test]
fn the_commands_are_grouped_by_category() {
    // Asserted on the list rather than on the screen: the whole list is taller
    // than any panel, so a screenful can only ever show the first few sections.
    let scratch = Scratch::new();
    let mut app = palette(&scratch);

    let Overlay::Palette(p) = &app.ui.overlay else {
        panic!("the palette is open");
    };
    let mut sections: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for item in &p.items {
        match item {
            vibe::ui::PaletteItem::Group(name) => {
                sections.push(name.clone());
                current = Some(name.clone());
            }
            vibe::ui::PaletteItem::Entry { action, .. } => {
                let PaletteAction::Run(cmd) = action else {
                    panic!("the command list runs commands");
                };
                assert_eq!(
                    current.as_deref(),
                    Some(cmd.category().label()),
                    "{} is under its own section",
                    cmd.title()
                );
            }
        }
    }
    assert_eq!(
        sections,
        vec!["File", "Edit", "Navigate", "Search", "View", "Help"],
        "one section per category, in the declared order"
    );

    // And the first of them is drawn as a heading.
    let rows = screen(&mut app, 100, 40);
    assert!(
        headings(&rows).iter().any(|r| r.contains(" File ")),
        "the File heading is on screen: {:#?}",
        headings(&rows)
    );
}

#[test]
fn a_heading_is_a_rule_with_the_name_in_the_middle() {
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    let rows = screen(&mut app, 100, 40);

    let row = headings(&rows)
        .into_iter()
        .find(|r| r.contains(" File "))
        .expect("a File heading");
    let name_at = row.find(" File ").expect("the name");
    let before = &row[..name_at];
    let after = &row[name_at + " File ".len()..];
    assert!(
        before.chars().filter(|c| *c == '─').count() > 10,
        "a rule before the name: {row:?}"
    );
    assert!(
        after.chars().filter(|c| *c == '─').count() > 10,
        "and after it: {row:?}"
    );
    // Centred, within a column: the two sides are the same length.
    let left = before.chars().filter(|c| *c == '─').count();
    let right = after.chars().filter(|c| *c == '─').count();
    assert!(
        left.abs_diff(right) <= 1,
        "the name sits in the middle: {left} / {right}"
    );
}

#[test]
fn the_selection_opens_on_a_command_not_a_heading() {
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    screen(&mut app, 100, 40);

    let Overlay::Palette(p) = &app.ui.overlay else {
        panic!("the palette is open");
    };
    assert!(!p.items[p.selected].is_group(), "a heading is not a choice");
    assert_eq!(p.selected, 1, "the first command, after the heading");
}

#[test]
fn stepping_down_walks_over_the_headings() {
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    screen(&mut app, 100, 40);

    // Walk to the last command of the first section: the one whose successor
    // is a heading. Bounded, so a palette with no headings fails here instead
    // of walking forever.
    let mut reached = false;
    for _ in 0..200 {
        let Overlay::Palette(p) = &app.ui.overlay else {
            panic!("the palette is open");
        };
        assert!(
            !p.items[p.selected].is_group(),
            "the selection landed on a heading at {}",
            p.selected
        );
        if p.items.get(p.selected + 1).is_some_and(|i| i.is_group()) {
            reached = true;
            break;
        }
        plain(&mut app, KeyCode::Down);
    }
    assert!(reached, "the first section ended in a heading");

    // One more step crosses the "Edit" heading and lands on Undo.
    plain(&mut app, KeyCode::Down);
    let Overlay::Palette(p) = &app.ui.overlay else {
        panic!("the palette is open");
    };
    assert_eq!(
        p.items[p.selected].action(),
        Some(&vibe::ui::PaletteAction::Run(vibe::commands::Command::Undo)),
        "the first command of the next section"
    );
}

#[test]
fn the_selection_never_rests_on_a_heading() {
    // Walking the whole list both ways, the selection is always something that
    // can be run.
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    screen(&mut app, 100, 40);

    for _ in 0..120 {
        let Overlay::Palette(p) = &app.ui.overlay else {
            panic!("the palette is open");
        };
        assert!(
            !p.items[p.selected].is_group(),
            "the selection is on a heading at {}",
            p.selected
        );
        plain(&mut app, KeyCode::Down);
    }
    for _ in 0..120 {
        let Overlay::Palette(p) = &app.ui.overlay else {
            panic!("the palette is open");
        };
        assert!(!p.items[p.selected].is_group());
        plain(&mut app, KeyCode::Up);
    }
}

#[test]
fn enter_on_a_heading_runs_nothing() {
    // Reachable only by putting the selection there deliberately, but it must
    // not run a command or close the palette.
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    screen(&mut app, 100, 40);

    if let Overlay::Palette(p) = &mut app.ui.overlay {
        p.selected = 0;
        assert!(p.items[0].is_group());
    }
    plain(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.ui.overlay, Overlay::Palette(_)),
        "the palette is still open"
    );
    assert_eq!(text(&app), "", "and nothing ran");
}

#[test]
fn typing_flattens_the_list() {
    // A search ranks by match, and a list in score order has no runs to group.
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    screen(&mut app, 100, 40);

    type_str(&mut app, "toggle");
    let rows = screen(&mut app, 100, 40);
    assert!(
        headings(&rows).is_empty(),
        "no sections while searching: {:#?}",
        headings(&rows)
    );
    assert!(
        rows.iter().any(|r| r.contains("Toggle Word Wrap")),
        "the matches are still there"
    );

    // Clearing the query brings them back.
    for _ in 0..6 {
        plain(&mut app, KeyCode::Backspace);
    }
    let rows = screen(&mut app, 100, 40);
    assert!(!headings(&rows).is_empty(), "the sections return");
}

#[test]
fn the_footer_counts_commands_not_headings() {
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    let rows = screen(&mut app, 100, 40);
    assert!(
        rows.iter().any(|r| r.contains("1/64")),
        "the first of sixty-four commands:\n{}",
        rows.join("\n")
    );

    for _ in 0..10 {
        plain(&mut app, KeyCode::PageDown);
    }
    let rows = screen(&mut app, 100, 40);
    assert!(
        rows.iter().any(|r| r.contains("64/64")),
        "and the last is still sixty-four, with the headings uncounted"
    );
}

#[test]
fn the_separator_is_visible_against_the_panel() {
    // The indent guides shipped once in a colour 65 units from the background
    // and the user reported that they did not exist. A separator has to be
    // seen, and it has to match the panel's own frame rather than whisper.
    let scratch = Scratch::new();
    let mut app = palette(&scratch);
    let (width, height) = (100u16, 40u16);
    let buf = {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| vibe::ui::draw(f, &mut app)).unwrap();
        terminal.backend().buffer().clone()
    };
    let theme = vibe::theme::Theme::dark();

    // Find the row and a column that holds the rule rather than the name.
    let mut checked = false;
    for y in 0..height {
        let row: String = (0..width).map(|x| buf[(x, y)].symbol()).collect();
        if !row.contains(" File ") || !row.contains('─') {
            continue;
        }
        let x = row.find('─').expect("a rule") as u16;
        assert_eq!(
            buf[(x, y)].fg,
            theme.border,
            "the rule is drawn in the panel's own border colour"
        );
        let (fr, fg_, fb) = rgb(theme.border);
        let (br, bg_, bb) = rgb(theme.overlay_bg);
        let distance: i32 = (fr as i32 - br as i32).abs()
            + (fg_ as i32 - bg_ as i32).abs()
            + (fb as i32 - bb as i32).abs();
        assert!(
            distance >= 60,
            "the rule is {distance} from the panel it sits on"
        );
        checked = true;
        break;
    }
    assert!(checked, "a File heading was drawn");
}

fn rgb(colour: ratatui::style::Color) -> (u8, u8, u8) {
    match colour {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        other => panic!("the theme uses literal colours, not {other:?}"),
    }
}

#[test]
fn the_file_picker_has_no_sections() {
    // Only the command list is a menu; a list of files has no categories to
    // divide it into.
    let scratch = Scratch::new();
    scratch.file("one.txt", "a");
    scratch.file("two.txt", "b");
    let mut app = editor(scratch.path());
    app.open_palette(PaletteMode::Files);
    // The index is built on a worker.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.file_index.is_none() {
        assert!(std::time::Instant::now() < deadline, "index never arrived");
        app.poll_jobs();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.refresh_palette();
    let rows = screen(&mut app, 100, 40);
    assert!(
        headings(&rows).is_empty(),
        "no separators in the file picker: {:#?}",
        headings(&rows)
    );
    assert!(
        rows.iter().any(|r| r.contains("one.txt")),
        "and the files are listed:\n{}",
        rows.join("\n")
    );
}
