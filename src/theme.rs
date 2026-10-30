//! Colour themes.
//!
//! Themes are plain data: the UI reads colours from here and never hardcodes
//! them. Two built-ins ship (dark and light), and every colour can be overridden
//! from the configuration file with a `#rrggbb` value, so users are not forced
//! to fork the editor to adjust a shade.

use std::collections::BTreeMap;

use ratatui::style::Color;

use crate::syntax::TokenKind;

/// Colours for syntax tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxColors {
    pub keyword: Color,
    pub type_: Color,
    pub constant: Color,
    pub function: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub operator: Color,
    pub punctuation: Color,
    pub attribute: Color,
    pub heading: Color,
}

impl SyntaxColors {
    pub fn get(&self, kind: TokenKind) -> Color {
        match kind {
            TokenKind::Keyword => self.keyword,
            TokenKind::Type => self.type_,
            TokenKind::Constant => self.constant,
            TokenKind::Function => self.function,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Comment => self.comment,
            TokenKind::Operator => self.operator,
            TokenKind::Punctuation => self.punctuation,
            TokenKind::Attribute => self.attribute,
            TokenKind::Heading => self.heading,
        }
    }
}

/// A complete palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub dark: bool,

    pub bg: Color,
    pub fg: Color,
    pub dim: Color,

    pub gutter_fg: Color,
    pub gutter_active_fg: Color,
    pub gutter_bg: Color,

    pub topbar_bg: Color,
    pub topbar_fg: Color,
    pub topbar_accent: Color,

    pub tab_active_bg: Color,
    pub tab_active_fg: Color,
    pub tab_inactive_bg: Color,
    pub tab_inactive_fg: Color,

    pub statusbar_bg: Color,
    pub statusbar_fg: Color,
    pub statusbar_accent: Color,
    /// Used for the "the cursor is not on screen" readout, so it reads as a
    /// remark about the view rather than as another coordinate.
    pub statusbar_away: Color,
    /// Behind a badge in a bar — the percentage readout. A shade off the bar it
    /// sits on, which is all a pill is in a cell grid.
    pub pill_bg: Color,

    pub hint_bg: Color,
    pub hint_fg: Color,

    pub selection_bg: Color,
    pub selection_fg: Color,
    pub cursor_line_bg: Color,
    pub cursor_fg: Color,
    pub cursor_bg: Color,

    pub match_bg: Color,
    pub match_fg: Color,
    pub current_match_bg: Color,

    pub border: Color,
    /// Hairlines: the dividers inside a bar and the frame of a panel.
    ///
    /// Dimmer than [`Theme::border`] on purpose. A panel frame is a container
    /// and should be seen; a divider between two tabs is a seam and should only
    /// be inferred.
    pub line: Color,
    /// The accent that marks *this one*: the wordmark, the modified dot, the
    /// active tab's edge, the language in the status bar.
    pub mark: Color,
    pub overlay_bg: Color,
    pub overlay_sel_bg: Color,
    pub overlay_sel_fg: Color,
    pub overlay_title: Color,

    pub error: Color,
    pub warn: Color,
    pub info: Color,
    pub success: Color,

    pub bracket_match: Color,
    pub indent_guide: Color,

    /// Background of a line that exists only on the right of a diff.
    pub diff_add_bg: Color,
    /// Background of a line that exists only on the left.
    pub diff_del_bg: Color,
    /// Background of a row where one side of the diff has nothing.
    ///
    /// Darker than the document, so a gap in one column reads as absence
    /// rather than as a blank line that happens to be there.
    pub diff_filler_bg: Color,

    pub syntax: SyntaxColors,
}

fn rgb(hex: &str) -> Option<Color> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// Parse a colour from configuration. Accepts `#rrggbb` and a few names.
pub fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim();
    if let Some(c) = rgb(v) {
        return Some(c);
    }
    match v.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        "reset" | "default" => Some(Color::Reset),
        _ => None,
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The default dark palette: deep blue-grey surfaces, warm accents.
    pub fn dark() -> Self {
        Self {
            name: "vibe-dark".to_string(),
            dark: true,

            bg: Color::Rgb(0x14, 0x17, 0x1e),
            fg: Color::Rgb(0xd7, 0xdb, 0xe3),
            dim: Color::Rgb(0x7c, 0x86, 0x96),

            gutter_fg: Color::Rgb(0x4a, 0x51, 0x60),
            gutter_active_fg: Color::Rgb(0xc8, 0xa0, 0x5a),
            gutter_bg: Color::Rgb(0x14, 0x17, 0x1e),

            topbar_bg: Color::Rgb(0x10, 0x13, 0x1a),
            topbar_fg: Color::Rgb(0xa8, 0xb0, 0xc0),
            topbar_accent: Color::Rgb(0x7a, 0xb8, 0xd8),

            // Every bar is its own shade, and the steps are what draws the
            // separations a pixel design would draw with a line: the tab strip
            // is a shade under the top bar, the hint bar a shade under the
            // status bar, and the active tab is the document's own surface, so
            // it opens into the text instead of sitting on it.
            tab_active_bg: Color::Rgb(0x14, 0x17, 0x1e),
            tab_active_fg: Color::Rgb(0xe8, 0xec, 0xf4),
            tab_inactive_bg: Color::Rgb(0x0f, 0x12, 0x18),
            tab_inactive_fg: Color::Rgb(0x7c, 0x86, 0x96),

            statusbar_bg: Color::Rgb(0x10, 0x13, 0x1a),
            statusbar_fg: Color::Rgb(0x9a, 0xa3, 0xb5),
            statusbar_accent: Color::Rgb(0x7a, 0xb8, 0xd8),
            statusbar_away: Color::Rgb(0xd8, 0xa8, 0x4f),
            pill_bg: Color::Rgb(0x1e, 0x24, 0x30),

            hint_bg: Color::Rgb(0x0d, 0x10, 0x16),
            hint_fg: Color::Rgb(0x9a, 0xa3, 0xb2),

            selection_bg: Color::Rgb(0x2d, 0x3b, 0x54),
            selection_fg: Color::Rgb(0xe8, 0xec, 0xf4),
            cursor_line_bg: Color::Rgb(0x1a, 0x1f, 0x29),
            cursor_fg: Color::Rgb(0x14, 0x17, 0x1e),
            cursor_bg: Color::Rgb(0xc8, 0xa0, 0x5a),

            match_bg: Color::Rgb(0x3a, 0x35, 0x22),
            match_fg: Color::Rgb(0xe8, 0xec, 0xf4),
            current_match_bg: Color::Rgb(0x8a, 0x6d, 0x2c),

            border: Color::Rgb(0x33, 0x3a, 0x48),
            line: Color::Rgb(0x24, 0x2b, 0x36),
            mark: Color::Rgb(0xc8, 0xa0, 0x5a),
            overlay_bg: Color::Rgb(0x1c, 0x21, 0x2b),
            overlay_sel_bg: Color::Rgb(0x2d, 0x3b, 0x54),
            overlay_sel_fg: Color::Rgb(0xff, 0xff, 0xff),
            overlay_title: Color::Rgb(0x7a, 0xb8, 0xd8),

            error: Color::Rgb(0xe0, 0x6c, 0x75),
            warn: Color::Rgb(0xd8, 0xa8, 0x4f),
            info: Color::Rgb(0x7a, 0xb8, 0xd8),
            success: Color::Rgb(0x8f, 0xbc, 0x8f),

            bracket_match: Color::Rgb(0xc8, 0xa0, 0x5a),
            indent_guide: Color::Rgb(0x3a, 0x44, 0x52),

            // Tints rather than colours: the text keeps its own foreground, and
            // the row says what happened to it. Far enough from the background
            // to be seen at a glance, dim enough that a screen of them is still
            // readable.
            diff_add_bg: Color::Rgb(0x16, 0x28, 0x1c),
            diff_del_bg: Color::Rgb(0x2e, 0x1a, 0x1e),
            diff_filler_bg: Color::Rgb(0x0f, 0x12, 0x18),

            syntax: SyntaxColors {
                keyword: Color::Rgb(0xc6, 0x8f, 0xd8),
                type_: Color::Rgb(0x7a, 0xb8, 0xd8),
                constant: Color::Rgb(0xd8, 0xa8, 0x4f),
                function: Color::Rgb(0x8f, 0xbc, 0xc8),
                string: Color::Rgb(0x9e, 0xc4, 0x7a),
                number: Color::Rgb(0xd8, 0xa8, 0x4f),
                comment: Color::Rgb(0x5c, 0x67, 0x78),
                operator: Color::Rgb(0xa8, 0xb0, 0xc0),
                punctuation: Color::Rgb(0x8b, 0x93, 0xa5),
                attribute: Color::Rgb(0xd7, 0xdb, 0xe3),
                heading: Color::Rgb(0x7a, 0xb8, 0xd8),
            },
        }
    }

    /// A light palette for bright terminals.
    pub fn light() -> Self {
        Self {
            name: "vibe-light".to_string(),
            dark: false,

            bg: Color::Rgb(0xfa, 0xfa, 0xf7),
            fg: Color::Rgb(0x2b, 0x30, 0x38),
            dim: Color::Rgb(0x8a, 0x90, 0x9a),

            gutter_fg: Color::Rgb(0xb2, 0xb8, 0xc2),
            gutter_active_fg: Color::Rgb(0x9a, 0x6b, 0x1e),
            gutter_bg: Color::Rgb(0xfa, 0xfa, 0xf7),

            topbar_bg: Color::Rgb(0xe9, 0xe9, 0xe3),
            topbar_fg: Color::Rgb(0x4a, 0x50, 0x5a),
            topbar_accent: Color::Rgb(0x1f, 0x6f, 0x96),

            tab_active_bg: Color::Rgb(0xfa, 0xfa, 0xf7),
            tab_active_fg: Color::Rgb(0x1a, 0x1d, 0x22),
            tab_inactive_bg: Color::Rgb(0xe2, 0xe2, 0xdc),
            tab_inactive_fg: Color::Rgb(0x8a, 0x90, 0x9a),

            statusbar_bg: Color::Rgb(0xe9, 0xe9, 0xe3),
            statusbar_fg: Color::Rgb(0x4a, 0x50, 0x5a),
            statusbar_accent: Color::Rgb(0x1f, 0x6f, 0x96),
            statusbar_away: Color::Rgb(0x9a, 0x6b, 0x1e),
            pill_bg: Color::Rgb(0xe2, 0xe2, 0xdb),

            hint_bg: Color::Rgb(0xdc, 0xdc, 0xd6),
            hint_fg: Color::Rgb(0x6b, 0x72, 0x80),

            selection_bg: Color::Rgb(0xd6, 0xe4, 0xf5),
            selection_fg: Color::Rgb(0x1a, 0x1d, 0x22),
            cursor_line_bg: Color::Rgb(0xf2, 0xf2, 0xed),
            cursor_fg: Color::Rgb(0xfa, 0xfa, 0xf7),
            cursor_bg: Color::Rgb(0x1f, 0x6f, 0x96),

            match_bg: Color::Rgb(0xff, 0xef, 0xb8),
            match_fg: Color::Rgb(0x2b, 0x30, 0x38),
            current_match_bg: Color::Rgb(0xff, 0xd0, 0x54),

            border: Color::Rgb(0xd0, 0xd0, 0xc8),
            line: Color::Rgb(0xdc, 0xdc, 0xd4),
            mark: Color::Rgb(0x9a, 0x6b, 0x1e),
            overlay_bg: Color::Rgb(0xef, 0xef, 0xea),
            overlay_sel_bg: Color::Rgb(0xd6, 0xe4, 0xf5),
            overlay_sel_fg: Color::Rgb(0x00, 0x00, 0x00),
            overlay_title: Color::Rgb(0x1f, 0x6f, 0x96),

            error: Color::Rgb(0xb3, 0x2d, 0x3a),
            warn: Color::Rgb(0x9a, 0x6b, 0x1e),
            info: Color::Rgb(0x1f, 0x6f, 0x96),
            success: Color::Rgb(0x2f, 0x7a, 0x4f),

            bracket_match: Color::Rgb(0x9a, 0x6b, 0x1e),
            indent_guide: Color::Rgb(0xd0, 0xd0, 0xc6),

            diff_add_bg: Color::Rgb(0xdc, 0xef, 0xe0),
            diff_del_bg: Color::Rgb(0xf6, 0xdc, 0xdf),
            diff_filler_bg: Color::Rgb(0xe9, 0xe9, 0xe4),

            syntax: SyntaxColors {
                keyword: Color::Rgb(0x8a, 0x3f, 0xb0),
                type_: Color::Rgb(0x1f, 0x6f, 0x96),
                constant: Color::Rgb(0x9a, 0x6b, 0x1e),
                function: Color::Rgb(0x2f, 0x6f, 0x7a),
                string: Color::Rgb(0x3f, 0x7a, 0x2f),
                number: Color::Rgb(0x9a, 0x6b, 0x1e),
                comment: Color::Rgb(0x8a, 0x90, 0x9a),
                operator: Color::Rgb(0x4a, 0x50, 0x5a),
                punctuation: Color::Rgb(0x6a, 0x70, 0x7a),
                attribute: Color::Rgb(0x2b, 0x30, 0x38),
                heading: Color::Rgb(0x1f, 0x6f, 0x96),
            },
        }
    }

    /// Look a theme up by name.
    pub fn by_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "dark" | "vibe-dark" | "default" => Some(Self::dark()),
            "light" | "vibe-light" => Some(Self::light()),
            // The names from before the rename. The configuration file moves
            // with the directory, so it still says `theme = "vela-dark"`, and
            // an unknown name would fall back to the default and silently turn
            // a light setup dark.
            "vela-dark" => Some(Self::dark()),
            "vela-light" => Some(Self::light()),
            _ => None,
        }
    }

    /// Names of the built-in themes.
    pub fn builtin_names() -> &'static [&'static str] {
        &["vibe-dark", "vibe-light"]
    }

    /// Apply `key = "#rrggbb"` overrides from configuration.
    ///
    /// Unknown keys are returned so the caller can warn instead of silently
    /// ignoring a typo.
    pub fn apply_overrides(&mut self, overrides: &BTreeMap<String, String>) -> Vec<String> {
        let mut unknown = Vec::new();
        for (key, value) in overrides {
            let Some(color) = parse_color(value) else {
                unknown.push(format!("{key}: invalid colour '{value}'"));
                continue;
            };
            if !self.set_color(key, color) {
                unknown.push(format!("{key}: unknown theme key"));
            }
        }
        unknown
    }

    fn set_color(&mut self, key: &str, c: Color) -> bool {
        let normalized = key.trim().to_ascii_lowercase().replace('-', "_");
        match normalized.as_str() {
            "bg" | "background" => self.bg = c,
            "fg" | "foreground" => self.fg = c,
            "dim" => self.dim = c,
            "gutter_fg" => self.gutter_fg = c,
            "gutter_active_fg" => self.gutter_active_fg = c,
            "gutter_bg" => self.gutter_bg = c,
            "topbar_bg" => self.topbar_bg = c,
            "topbar_fg" => self.topbar_fg = c,
            "topbar_accent" => self.topbar_accent = c,
            "tab_active_bg" => self.tab_active_bg = c,
            "tab_active_fg" => self.tab_active_fg = c,
            "tab_inactive_bg" => self.tab_inactive_bg = c,
            "tab_inactive_fg" => self.tab_inactive_fg = c,
            "statusbar_bg" => self.statusbar_bg = c,
            "statusbar_fg" => self.statusbar_fg = c,
            "statusbar_accent" => self.statusbar_accent = c,
            "statusbar_away" => self.statusbar_away = c,
            "pill" | "pill_bg" => self.pill_bg = c,
            "hint_bg" => self.hint_bg = c,
            "hint_fg" => self.hint_fg = c,
            "selection_bg" => self.selection_bg = c,
            "selection_fg" => self.selection_fg = c,
            "cursor_line_bg" => self.cursor_line_bg = c,
            "cursor_fg" => self.cursor_fg = c,
            "cursor_bg" => self.cursor_bg = c,
            "match_bg" => self.match_bg = c,
            "match_fg" => self.match_fg = c,
            "current_match_bg" => self.current_match_bg = c,
            "border" => self.border = c,
            "line" | "divider" | "hairline" => self.line = c,
            "mark" | "accent_gold" => self.mark = c,
            "overlay_bg" => self.overlay_bg = c,
            "overlay_sel_bg" => self.overlay_sel_bg = c,
            "overlay_sel_fg" => self.overlay_sel_fg = c,
            "overlay_title" => self.overlay_title = c,
            "error" => self.error = c,
            "accent" => {
                self.topbar_accent = c;
                self.statusbar_accent = c;
            }
            "warn" | "warning" => self.warn = c,
            "info" => self.info = c,
            "success" => self.success = c,
            "bracket_match" => self.bracket_match = c,
            "indent_guide" => self.indent_guide = c,
            "diff_add_bg" => self.diff_add_bg = c,
            "diff_del_bg" => self.diff_del_bg = c,
            "diff_filler_bg" => self.diff_filler_bg = c,
            "syntax_keyword" => self.syntax.keyword = c,
            "syntax_type" => self.syntax.type_ = c,
            "syntax_constant" => self.syntax.constant = c,
            "syntax_function" => self.syntax.function = c,
            "syntax_string" => self.syntax.string = c,
            "syntax_number" => self.syntax.number = c,
            "syntax_comment" => self.syntax.comment = c,
            "syntax_operator" => self.syntax.operator = c,
            "syntax_punctuation" => self.syntax.punctuation = c,
            "syntax_attribute" => self.syntax.attribute = c,
            "syntax_heading" => self.syntax.heading = c,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_from_before_the_rename_still_resolve() {
        // A configuration file that moved with the directory still says
        // `vela-dark`; falling back to the default would turn a light setup
        // dark without saying so.
        assert_eq!(
            Theme::by_name("vela-dark").map(|t| t.name),
            Some("vibe-dark".into())
        );
        assert_eq!(
            Theme::by_name("vela-light").map(|t| t.name),
            Some("vibe-light".into())
        );
        assert!(Theme::by_name("vibe-dark").is_some());
        assert!(Theme::by_name("nonsense").is_none());
    }

    #[test]
    fn parses_hex_colours() {
        assert_eq!(parse_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color("00ff00"), Some(Color::Rgb(0, 255, 0)));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("nonsense"), None);
        assert_eq!(parse_color("red"), Some(Color::Red));
    }

    #[test]
    fn overrides_apply_and_report_unknown() {
        let mut t = Theme::dark();
        let mut map = BTreeMap::new();
        map.insert("accent".to_string(), "#010203".to_string());
        map.insert("bg".to_string(), "#0a0b0c".to_string());
        map.insert("nope".to_string(), "#ffffff".to_string());
        map.insert("fg".to_string(), "notacolor".to_string());
        let unknown = t.apply_overrides(&map);
        assert_eq!(t.bg, Color::Rgb(0x0a, 0x0b, 0x0c));
        assert_eq!(unknown.len(), 2);
    }

    #[test]
    fn themes_are_distinct() {
        let dark = Theme::dark();
        let light = Theme::light();
        assert_ne!(dark.bg, light.bg);
        assert!(dark.dark && !light.dark);
        assert!(Theme::by_name("light").is_some());
        assert!(Theme::by_name("missing").is_none());
    }
}
