//! Indentation as the file itself declares it.
//!
//! A global `use_spaces` / `tab_width` pair is wrong the moment you open
//! somebody else's file: a tab-indented Go file edited with four spaces gets the
//! wrong indent on every Enter, silently. The file in front of you is better
//! evidence than a setting, so its own indentation is read once, at load, and
//! the configuration becomes the fallback for files that say nothing — and for
//! buffers that have no file yet.
//!
//! What is detected is the *indent unit*: whether a level is a tab or a run of
//! spaces, and how many spaces. The width a tab is *drawn* at stays a view
//! setting, because that is what it is: a file written with tabs is correct at
//! any display width, and changing the display width would move every column on
//! screen for a reason that has nothing to do with the file.

/// How one buffer is indented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentStyle {
    /// Indent with spaces rather than tabs.
    pub use_spaces: bool,
    /// Columns per indent level. Meaningful only with spaces: a tab is one
    /// character, and how wide it looks is the terminal's business.
    pub width: usize,
}

impl IndentStyle {
    /// The text one level of indentation inserts.
    pub fn unit(&self) -> String {
        if self.use_spaces {
            " ".repeat(self.width)
        } else {
            "\t".to_string()
        }
    }

    /// How the status bar names it: `Spaces: 2`, `Tabs`.
    pub fn label(&self) -> String {
        if self.use_spaces {
            format!("Spaces: {}", self.width)
        } else {
            "Tabs".to_string()
        }
    }
}

/// What the loader needs to know to detect indentation.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Look at the file at all.
    pub detect: bool,
    /// Width to assume when the file indents with tabs, which carries no width
    /// of its own.
    pub fallback_width: usize,
}

/// Deeper than this and the evidence is more likely to be a continuation line
/// or a pasted block than one indent level.
const MAX_WIDTH: usize = 8;

/// How much of a file is worth reading to decide.
///
/// The answer is in the first screenful of any real file, and the loader is on
/// the path that has to stay fast for a two-gigabyte one, so this stops early
/// rather than scanning to the end.
const MAX_LINES: usize = 2_000;
const MAX_BYTES: usize = 512 * 1024;

/// Read a file's indentation from its text.
///
/// Returns `None` when the file has nothing to say — no indented line at all —
/// which is the caller's cue to keep the configured style.
pub fn detect(text: &str, options: Options) -> Option<IndentStyle> {
    if !options.detect {
        return None;
    }

    let mut tab_lines = 0usize;
    let mut space_lines = 0usize;
    // Indentation of every line with content, in columns of spaces. Lines that
    // are not indented count as zero, because the step from zero is evidence
    // too.
    let mut widths: Vec<usize> = Vec::new();
    let mut bytes = 0usize;

    for (seen, line) in text.lines().enumerate() {
        if seen >= MAX_LINES || bytes >= MAX_BYTES {
            break;
        }
        bytes += line.len() + 1;

        let mut first: Option<char> = None;
        let mut content = false;
        for c in line.chars() {
            match c {
                ' ' | '\t' => {
                    first.get_or_insert(c);
                }
                _ => {
                    content = true;
                    break;
                }
            }
        }
        if !content {
            // A blank line is indented however the last one was; it says
            // nothing either way.
            continue;
        }

        match first {
            None => widths.push(0),
            Some('\t') => tab_lines += 1,
            Some(' ') => {
                space_lines += 1;
                widths.push(line.chars().take_while(|c| *c == ' ').count());
            }
            _ => {}
        }
    }

    if tab_lines == 0 && space_lines == 0 {
        return None;
    }
    // Majority wins, and a tie goes to spaces: a file with an even split is
    // more likely a space file with a stray tab in it than the reverse.
    let use_spaces = space_lines >= tab_lines;
    let width = if use_spaces {
        // A width that is not an indent level means the evidence is not an
        // indent level either — a pasted block, a continuation line — so the
        // reading is not good enough to override the settings with.
        width_from(&widths)?
    } else {
        // A tab file has no width of its own to read; the fallback only ever
        // reaches `dedent`, which removes one tab regardless.
        options.fallback_width
    };
    Some(IndentStyle { use_spaces, width })
}

/// The indent width the indentation levels imply, if they imply one.
///
/// The width shows up as the *step* between nesting depths, not as the absolute
/// indentation of any one line: a body at 4 says four, whatever depth it
/// happens to sit at. When nothing ever steps deeper — a file with one level,
/// or none — the shallowest indentation is the only evidence there is.
fn width_from(widths: &[usize]) -> Option<usize> {
    let mut steps: Vec<usize> = Vec::new();
    for pair in widths.windows(2) {
        if pair[1] > pair[0] {
            steps.push(pair[1] - pair[0]);
        }
    }
    if let Some(step) = most_common(&steps) {
        if (1..=MAX_WIDTH).contains(&step) {
            return Some(step);
        }
    }
    let shallowest = widths.iter().copied().filter(|w| *w > 0).min()?;
    (1..=MAX_WIDTH).contains(&shallowest).then_some(shallowest)
}

/// The most frequent value, with ties going to the smaller one — of two equally
/// common steps, the smaller is the one that is a unit rather than a multiple.
fn most_common(values: &[usize]) -> Option<usize> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mut best = (sorted[0], 0usize);
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j < sorted.len() && sorted[j] == sorted[i] {
            j += 1;
        }
        if j - i > best.1 {
            best = (sorted[i], j - i);
        }
        i = j;
    }
    Some(best.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Options = Options {
        detect: true,
        fallback_width: 4,
    };

    fn style(text: &str) -> Option<IndentStyle> {
        detect(text, ON)
    }

    #[test]
    fn four_spaces_is_four() {
        let text = "fn main() {\n    let x = 1;\n    if x > 0 {\n        go();\n    }\n}\n";
        assert_eq!(
            style(text),
            Some(IndentStyle {
                use_spaces: true,
                width: 4
            })
        );
    }

    #[test]
    fn two_spaces_is_two() {
        let text = "def f():\n  if x:\n    return 1\n  return 0\n";
        assert_eq!(
            style(text),
            Some(IndentStyle {
                use_spaces: true,
                width: 2
            })
        );
    }

    #[test]
    fn a_tab_file_uses_tabs() {
        let text = "func main() {\n\tif x {\n\t\tgo()\n\t}\n}\n";
        assert_eq!(
            style(text),
            Some(IndentStyle {
                use_spaces: false,
                width: 4
            }),
            "the width is the fallback: a tab has none of its own"
        );
    }

    #[test]
    fn a_file_with_no_indentation_says_nothing() {
        assert_eq!(style("one\ntwo\nthree\n"), None);
        assert_eq!(style(""), None);
        assert_eq!(style("\n\n\n"), None, "blank lines are not evidence");
    }

    #[test]
    fn a_single_level_is_read_from_its_own_indentation() {
        let text = "{\n  \"a\": 1,\n  \"b\": 2\n}\n";
        assert_eq!(style(text).map(|s| s.width), Some(2));
    }

    #[test]
    fn a_deeper_first_block_does_not_mistake_the_depth_for_the_width() {
        // The body sits at 8 because it is two levels in; the step that matters
        // is the 4 from the line before it.
        let text = "impl X {\n    fn f() {\n        body();\n    }\n}\n";
        assert_eq!(style(text).map(|s| s.width), Some(4));
    }

    #[test]
    fn spaces_win_when_they_are_the_majority() {
        let text = "a\n  b\n  c\n  d\n\te\n";
        assert_eq!(style(text).map(|s| s.use_spaces), Some(true));
    }

    #[test]
    fn tabs_win_when_they_are_the_majority() {
        let text = "a\n\tb\n\tc\n\td\n  e\n";
        assert_eq!(style(text).map(|s| s.use_spaces), Some(false));
    }

    #[test]
    fn a_tie_goes_to_spaces() {
        let text = "a\n\tb\n  c\n";
        assert_eq!(style(text).map(|s| s.use_spaces), Some(true));
    }

    #[test]
    fn an_alignment_run_after_a_tab_is_still_a_tab_line() {
        let text = "fn f() {\n\tlet x = 1;\n\tlet y = 2;\n}\n";
        assert_eq!(style(text).map(|s| s.use_spaces), Some(false));
    }

    #[test]
    fn absurd_widths_are_not_evidence() {
        // Sixteen columns is a continuation or a paste, not an indent level.
        let text = "{\n                \"a\": 1\n}\n";
        assert_eq!(style(text), None, "no usable width, no style");
    }

    #[test]
    fn detection_can_be_turned_off() {
        let text = "fn main() {\n    body();\n}\n";
        assert_eq!(
            detect(
                text,
                Options {
                    detect: false,
                    fallback_width: 2
                }
            ),
            None
        );
    }

    #[test]
    fn the_unit_is_what_typing_one_level_inserts() {
        let spaces = IndentStyle {
            use_spaces: true,
            width: 2,
        };
        assert_eq!(spaces.unit(), "  ");
        assert_eq!(spaces.label(), "Spaces: 2");

        let tabs = IndentStyle {
            use_spaces: false,
            width: 4,
        };
        assert_eq!(tabs.unit(), "\t");
        assert_eq!(tabs.label(), "Tabs");
    }

    #[test]
    fn a_huge_file_is_read_only_at_the_start() {
        // One line, far longer than the byte cap, with its indent at the front:
        // the scan must not walk the whole thing.
        let mut text = String::from("    x");
        text.push_str(&"y".repeat(MAX_BYTES + 1_000));
        assert_eq!(style(&text).map(|s| s.width), Some(4));
    }
}
